//! Rule-based asset cooking, driven from build scripts.
//!
//! # Responsibilities
//!
//! - Find each rule's inputs by glob under a pipeline's root, rebuild the
//!   stale outputs, and report what was found, rebuilt and skipped.
//! - Stamp an output that a rule left unchanged with its newest input's time,
//!   so the next run treats it as fresh.
//! - Write a project's `res` and the roots its modules ship as one asset pack
//!   ([`write_asset_pack_with_roots`]), which a shipping build embeds and the
//!   engine's asset store reads.
//! - Move a source asset together with its `.meta` file ([`move_asset`]).
//!
//! # Design
//!
//! The crate has no dependencies beyond `pill_asset_format`'s constants, like
//! `pill_hot_scan`, because it runs from build scripts, where a dependency is
//! paid on every clean build. Globbing and timestamps come from `std::fs`.
//!
//! Shader cooking is not optional: [`default_rules`] always carries
//! [`HlslToWgsl`], because a shader that was never cooked does not exist.
//! Cooking meshes and textures is opt-in through [`Pipeline::with_manifest`],
//! so a project that decodes those at runtime pays neither the cook time nor
//! the decoder's dependencies.

// The workspace bans `std::time::Instant`, and this crate is allowed to use
// it: cooking happens in build scripts and the development host, never in a
// game, and this crate does not depend on `pill_core`, whose platform clock
// the ban points to.
#![allow(clippy::disallowed_types)]

// Standard library
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// External crates
use pill_asset_format::{ASSET_PACK_MAGIC, ASSET_PACK_VERSION};

/// The rules this crate ships: the shader rule, and the [`default_rules`] set
/// that every pipeline runs.
pub mod rules;

pub use rules::{default_rules, HlslToWgsl};

/// Cooking a crate's whole shader tree: the routine a build script and the
/// development shader watcher share.
pub mod shader_tree;

/// Moving a source asset and its metadata file as one operation.
pub mod asset_move;

pub use asset_move::{metadata_path_of, move_asset, MoveAssetError};

pub use shader_tree::{cook_shader_tree, shader_tree_layout, ShaderTreeLayout, ShaderTreeReport};

// =============================================================================
// Errors
// =============================================================================

/// Failure to discover inputs, cook an output, read a manifest or write an
/// asset pack.
#[derive(Debug)]
pub enum CookError {
    /// The rule's input glob is not a shape this pipeline supports.
    BadGlob {
        /// The pattern that could not be expanded.
        pattern: String,
    },
    /// A filesystem operation failed.
    Io {
        /// Path the operation touched.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A rule could not turn its input into its output, or the pack writer
    /// could not name a file.
    Rule {
        /// The rule that failed, or `asset_pack` when a path cannot be keyed.
        rule: &'static str,
        /// Input it was working on.
        input: PathBuf,
        /// The failure text the rule or the pack writer reported.
        detail: String,
    },
    /// The manifest names a rule the caller did not supply.
    UnknownRule {
        /// The name found in the manifest.
        name: String,
    },
}

impl fmt::Display for CookError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadGlob { pattern } => {
                write!(formatter, "unsupported input glob: {pattern}")
            }
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Rule {
                rule,
                input,
                detail,
            } => {
                write!(
                    formatter,
                    "rule {rule} failed on {}: {detail}",
                    input.display()
                )
            }
            Self::UnknownRule { name } => write!(
                formatter,
                "the manifest names a rule this pipeline does not have: {name}"
            ),
        }
    }
}

impl std::error::Error for CookError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

// =============================================================================
// Rule
// =============================================================================

/// One transformation applied to every file its glob matches.
pub trait Rule {
    /// The name reported in errors and matched against a manifest entry.
    fn name(&self) -> &'static str;

    /// The glob, relative to the pipeline root, that selects this rule's inputs.
    ///
    /// Only two shapes are supported: `directory/*.extension`, or
    /// `*.extension` for a root whose inputs sit directly under it. The rules
    /// need nothing more, and a full glob engine would be a dependency every
    /// build script pays for.
    fn input_glob(&self) -> &'static str;

    /// The output that `input` produces. Its timestamp decides staleness.
    fn output_for(&self, input: &Path) -> PathBuf;

    /// The side outputs written in addition to [`Rule::output_for`].
    fn extra_outputs(&self, _input: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    /// The inputs besides `input` that the rule's build reads, if any.
    ///
    /// These are used only for staleness: an output is stale when any of them
    /// is newer than it. The shader rule needs this for the headers its
    /// sources `#include`. A build script can also report those headers to
    /// cargo, but without this list the per-file check would see the output as
    /// fresh and keep a stale cook.
    ///
    /// # Errors
    ///
    /// Returns [`CookError::Io`] when the extra inputs cannot be listed.
    fn extra_inputs(&self, _input: &Path) -> Result<Vec<PathBuf>, CookError> {
        Ok(Vec::new())
    }

    /// Turn `input` into `output`. Called only while `output` is stale.
    ///
    /// # Errors
    ///
    /// Returns [`CookError::Rule`] when the underlying tool fails. `detail`
    /// carries the tool's own output, so a build script's failure can be read
    /// without rerunning the tool by hand.
    fn build(&self, input: &Path, output: &Path) -> Result<(), CookError>;
}

// =============================================================================
// PipelineStats
// =============================================================================

/// The files one pass of [`Pipeline::run`] or [`Pipeline::plan`] found, and
/// what it did with them.
#[derive(Debug, Default)]
pub struct PipelineStats {
    /// Every input that any rule matched. Build scripts print these as
    /// `cargo:rerun-if-changed` lines.
    pub discovered_inputs: Vec<PathBuf>,
    /// The outputs written by this pass.
    pub rebuilt_outputs: Vec<PathBuf>,
    /// The outputs that were already newer than their input and extra inputs.
    pub skipped_outputs: Vec<PathBuf>,
    /// One `(input, output, rule)` per output a rule can produce, whether it
    /// was planned, rebuilt or skipped.
    pub cook_edges: Vec<(PathBuf, PathBuf, &'static str)>,
    /// How long each rebuilt output took.
    pub cook_times: Vec<(PathBuf, Duration)>,
}

// =============================================================================
// Pipeline
// =============================================================================

/// A set of rules applied under one root directory.
pub struct Pipeline {
    /// Directory the rules' globs resolve against.
    pub root: PathBuf,
    /// Rules to apply, in order.
    pub rules: Vec<Box<dyn Rule>>,
}

impl Pipeline {
    /// Create a pipeline over `root` with the rules every pipeline runs
    /// ([`default_rules`]).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            rules: default_rules(),
        }
    }

    /// Create a pipeline over `root` that runs `rules` instead of
    /// [`default_rules`].
    ///
    /// Use this when a rule needs configuring first, for example the shader
    /// rule's include path.
    pub fn with_rules(root: impl Into<PathBuf>, rules: Vec<Box<dyn Rule>>) -> Self {
        Self {
            root: root.into(),
            rules,
        }
    }

    /// Add the optional rules that `manifest` names, looked up through
    /// `resolve_rule`.
    ///
    /// The manifest is plain text: one rule name per line, where `#` starts a
    /// comment and blank lines are skipped. This is how a project opts into
    /// cooking meshes and textures; without a manifest the optional rules stay
    /// off. `resolve_rule` is the caller's registry, so this crate does not
    /// carry the decoders.
    ///
    /// # Errors
    ///
    /// Returns [`CookError::Io`] when the manifest cannot be read. Returns
    /// [`CookError::UnknownRule`] when the manifest names a rule that
    /// `resolve_rule` does not supply.
    pub fn with_manifest(
        mut self,
        manifest: &Path,
        resolve_rule: impl Fn(&str) -> Option<Box<dyn Rule>>,
    ) -> Result<Self, CookError> {
        let text = match fs::read_to_string(manifest) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(self),
            Err(source) => {
                return Err(CookError::Io {
                    path: manifest.to_owned(),
                    source,
                })
            }
        };
        for line in text.lines() {
            let name = line.split('#').next().unwrap_or_default().trim();
            if name.is_empty() {
                continue;
            }
            match resolve_rule(name) {
                Some(rule) => self.rules.push(rule),
                None => {
                    return Err(CookError::UnknownRule {
                        name: name.to_owned(),
                    })
                }
            }
        }
        Ok(self)
    }

    /// Cook every stale output.
    ///
    /// # Errors
    ///
    /// Returns the first failure and leaves the outputs this pass already wrote
    /// in place. An output is only replaced by a successful build of its own
    /// rule.
    ///
    /// # Examples
    ///
    /// A build script reports every discovered input back to cargo:
    ///
    /// ```no_run
    /// let stats = pill_assets::Pipeline::new("src").run()?;
    /// for input in &stats.discovered_inputs {
    ///     println!("cargo:rerun-if-changed={}", input.display());
    /// }
    /// # Ok::<(), pill_assets::CookError>(())
    /// ```
    pub fn run(&self) -> Result<PipelineStats, CookError> {
        self.walk(true)
    }

    /// List what a run would discover, without cooking anything.
    ///
    /// `discovered_inputs` and `cook_edges` are filled in; `rebuilt_outputs`,
    /// `skipped_outputs` and `cook_times` stay empty because nothing is built
    /// or timed.
    ///
    /// # Errors
    ///
    /// Returns [`CookError::BadGlob`] or [`CookError::Io`], the same failures
    /// as [`Pipeline::run`].
    pub fn plan(&self) -> Result<PipelineStats, CookError> {
        self.walk(false)
    }

    /// Run one pass over every rule, cooking only when `should_cook` is set.
    ///
    /// # Errors
    ///
    /// Returns the first failure, as [`Pipeline::run`] does.
    fn walk(&self, should_cook: bool) -> Result<PipelineStats, CookError> {
        // Step 1: Record every output the rules can produce.
        let mut stats = PipelineStats::default();
        for rule in &self.rules {
            for input in expand(&self.root, rule.input_glob())? {
                let output = rule.output_for(&input);
                stats.discovered_inputs.push(input.clone());
                stats
                    .cook_edges
                    .push((input.clone(), output.clone(), rule.name()));
                for extra_output in rule.extra_outputs(&input) {
                    stats
                        .cook_edges
                        .push((input.clone(), extra_output, rule.name()));
                }
                // Step 2: Stop here when the caller only wants the plan.
                if !should_cook {
                    continue;
                }
                // Step 3: An output is stale when it is older than the input
                // or older than any extra input.
                let extra_inputs = rule.extra_inputs(&input)?;
                let mut stale = is_stale(&input, &output)?;
                for extra_input in &extra_inputs {
                    if is_stale(extra_input, &output)? {
                        stale = true;
                        break;
                    }
                }
                if !stale {
                    stats.skipped_outputs.push(output);
                    continue;
                }
                // Step 4: Build the output and stamp it with its newest
                // input's time.
                let started_at = Instant::now();
                rule.build(&input, &output)?;
                stamp_output(&output, std::iter::once(&input).chain(&extra_inputs))?;
                stats
                    .cook_times
                    .push((output.clone(), started_at.elapsed()));
                stats.rebuilt_outputs.push(output);
            }
        }
        Ok(stats)
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Return every file below `directory`, recursively, sorted by path.
///
/// A shader `#include`s headers that the rule's own glob does not match, so a
/// build script reports them to cargo by hand through this function.
///
/// # Errors
///
/// Returns [`CookError::Io`] when a directory cannot be read. A directory that
/// does not exist is not an error and yields nothing.
pub fn walk_files(directory: &Path) -> Result<Vec<PathBuf>, CookError> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(source) => {
            return Err(CookError::Io {
                path: directory.to_owned(),
                source,
            })
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| CookError::Io {
            path: directory.to_owned(),
            source,
        })?;
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk_files(&path)?);
        } else if path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Write every file below `source` into one asset pack at `pack_path`, and
/// return the packed files for a build script to report to cargo.
///
/// This is a shorthand for [`write_asset_pack_with_roots`] with no module
/// roots: a project with no module resources beside it packs only its own
/// `res`.
///
/// # Errors
///
/// Returns the same errors as [`write_asset_pack_with_roots`].
pub fn write_asset_pack(source: &Path, pack_path: &Path) -> Result<Vec<PathBuf>, CookError> {
    write_asset_pack_with_roots(source, &[], pack_path)
}

/// Write every file below `source` and below each of `module_roots` into one
/// asset pack at `pack_path`, and return the packed files for a build script
/// to report to cargo.
///
/// The format belongs to `pill_engine::asset_store`, and the two change
/// together. A pack starts with `PILLPACK`, a `u32` version and a `u32` entry
/// count. Then comes one entry per file: a `u32` path length, the
/// `/`-separated path below its root, a `u64` offset and a `u64` length
/// measured from the start of the pack. Every number is little-endian. The
/// file bytes follow the index, sorted by path. A missing `source` or root
/// contributes no files.
///
/// A module's resources are named relative to its own root, so they pack
/// under that same name. A shipped build then reads them where a development
/// build finds them beside the module, and no directory has to be declared.
/// Names resolve the way `pill_engine::asset_store::read` resolves them: the
/// project's files first, then each root in order. The index is flat, so a
/// duplicate would replace the earlier entry silently. `res/editor/` is the
/// editor's own content and stays out of the project's part. A module root is
/// read-only content and keeps everything it ships.
///
/// # Errors
///
/// Returns [`CookError::Io`] when a file cannot be read or the pack cannot be
/// written, and [`CookError::Rule`] for a path that is not UTF-8.
pub fn write_asset_pack_with_roots(
    source: &Path,
    module_roots: &[PathBuf],
    pack_path: &Path,
) -> Result<Vec<PathBuf>, CookError> {
    // Step 1: Collect the project's own files, named by their `/`-separated
    // path below `source`.
    let mut files = Vec::new();
    for file in walk_files(source)? {
        let Some(key) = pack_key(source, &file)? else {
            continue;
        };
        // `res/editor/` holds the editor's own content: its theme layers and
        // the fonts they name. A game never loads it, so it never ships.
        if key.split('/').next() == Some("editor") {
            continue;
        }
        files.push((key, file));
    }
    // Step 2: Add each module root's files. A project file with a module
    // resource's name belongs to the project, as the store orders the two, so
    // a key the project claimed is not offered to a root.
    let mut claimed: BTreeSet<String> = files.iter().map(|(key, _)| key.clone()).collect();
    for root in module_roots {
        for file in walk_files(root)? {
            let Some(key) = pack_key(root, &file)? else {
                continue;
            };
            if claimed.insert(key.clone()) {
                files.push((key, file));
            }
        }
    }
    files.sort();

    // Step 3: Build the index and the file bytes. The index comes first, so
    // every offset is known before any bytes are written: the first file
    // starts after the header and the whole index.
    let index_length: usize = files.iter().map(|(key, _)| 4 + key.len() + 16).sum();
    let mut offset = (ASSET_PACK_MAGIC.len() + 4 + 4 + index_length) as u64;
    let mut file_contents = Vec::with_capacity(files.len());
    let mut pack = Vec::new();
    pack.extend_from_slice(ASSET_PACK_MAGIC);
    pack.extend_from_slice(&ASSET_PACK_VERSION.to_le_bytes());
    pack.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (key, file) in &files {
        let bytes = fs::read(file).map_err(|source| CookError::Io {
            path: file.clone(),
            source,
        })?;
        pack.extend_from_slice(&(key.len() as u32).to_le_bytes());
        pack.extend_from_slice(key.as_bytes());
        pack.extend_from_slice(&offset.to_le_bytes());
        pack.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        offset += bytes.len() as u64;
        file_contents.push(bytes);
    }
    for bytes in file_contents {
        pack.extend_from_slice(&bytes);
    }

    // Step 4: Replace the pack only when its bytes changed, so an unchanged
    // `res` does not relink the binary that embeds it.
    if fs::read(pack_path).ok().as_deref() != Some(pack.as_slice()) {
        if let Some(parent) = pack_path.parent() {
            fs::create_dir_all(parent).map_err(|source| CookError::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        fs::write(pack_path, &pack).map_err(|source| CookError::Io {
            path: pack_path.to_owned(),
            source,
        })?;
    }
    Ok(files.into_iter().map(|(_, file)| file).collect())
}

/// The `/`-separated key that `file` packs under below `root`, or `None` when
/// any part of the path starts with a dot.
///
/// A dot-named path is never an asset. `write_beside` writes a metadata update
/// as `.<file>.meta.<pid>-<seq>.tmp`, and a crash can leave one behind. The
/// engine's own walk of `res` skips dot-files, and the temporary file holds
/// the asset's own header, so indexing it would take that asset's guid.
///
/// # Errors
///
/// Returns [`CookError::Rule`] when the path is not UTF-8.
fn pack_key(root: &Path, file: &Path) -> Result<Option<String>, CookError> {
    let relative = file.strip_prefix(root).unwrap_or(file);
    let parts: Option<Vec<&str>> = relative.iter().map(|part| part.to_str()).collect();
    let key = parts.ok_or_else(|| CookError::Rule {
        rule: "asset_pack",
        input: file.to_owned(),
        detail: "the path is not UTF-8".to_owned(),
    })?;
    if key.iter().any(|part| part.starts_with('.')) {
        return Ok(None);
    }
    Ok(Some(key.join("/")))
}

/// Return the files that match one `directory/*.extension` pattern below
/// `root`.
///
/// # Errors
///
/// Returns [`CookError::BadGlob`] when the pattern has no `*`, and
/// [`CookError::Io`] when a directory cannot be read.
fn expand(root: &Path, pattern: &str) -> Result<Vec<PathBuf>, CookError> {
    let unsupported = || CookError::BadGlob {
        pattern: pattern.to_owned(),
    };
    let (directory, file_pattern) = match pattern.rsplit_once('/') {
        Some((directory, file_pattern)) => (root.join(directory), file_pattern),
        // No directory part: the inputs sit directly under the root. That is
        // how a tree that is itself the shaders directory is cooked.
        None => (root.to_owned(), pattern),
    };
    let (prefix, suffix) = file_pattern.split_once('*').ok_or_else(unsupported)?;

    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        // A root without this directory has no inputs for the rule. A crate
        // that cooks shaders but ships none of its own is not a failure.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(CookError::Io {
                path: directory,
                source,
            })
        }
    };

    let mut matched = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| CookError::Io {
            path: directory.clone(),
            source,
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with(prefix) && name.ends_with(suffix) && entry.path().is_file() {
            matched.push(entry.path());
        }
    }
    matched.sort();
    Ok(matched)
}

/// Mark `output` as cooked against `inputs`, whether or not the rule rewrote
/// it. An output the rule never produced is left for the next run to find
/// missing.
///
/// A tool may leave an output untouched when the bytes it would write are
/// identical - `slangc` does - so without a stamp an edit that changes
/// nothing in the output would leave it older than its input and re-cooked
/// every run. The stamp is the newest input's own timestamp, and it only
/// moves the output forward to it. A clock stamp would be wrong: file times
/// are coarser than `SystemTime::now()`, so an input saved just after the
/// cook can carry an earlier time and would never be cooked.
///
/// # Errors
///
/// Returns [`CookError::Io`] when an input's time or the output's time cannot
/// be read, or when the output cannot be stamped.
fn stamp_output<'a>(
    output: &Path,
    inputs: impl IntoIterator<Item = &'a PathBuf>,
) -> Result<(), CookError> {
    let modified_time_of = |path: &Path| {
        fs::metadata(path)
            .and_then(|meta| meta.modified())
            .map_err(|source| CookError::Io {
                path: path.to_owned(),
                source,
            })
    };
    let output_modified = match fs::metadata(output).and_then(|meta| meta.modified()) {
        Ok(time) => time,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(CookError::Io {
                path: output.to_owned(),
                source,
            })
        }
    };
    let mut newest_time = None;
    for input in inputs {
        let time = modified_time_of(input)?;
        newest_time =
            Some(newest_time.map_or(time, |newest: std::time::SystemTime| newest.max(time)));
    }
    let Some(newest_time) = newest_time.filter(|newest| *newest > output_modified) else {
        return Ok(());
    };
    fs::File::options()
        .write(true)
        .open(output)
        .and_then(|file| file.set_modified(newest_time))
        .map_err(|source| CookError::Io {
            path: output.to_owned(),
            source,
        })
}

/// Whether `output` is missing or older than `input`.
///
/// An output whose time cannot be read counts as stale, so the rule rebuilds
/// it.
///
/// # Errors
///
/// Returns [`CookError::Io`] when the input's time cannot be read.
fn is_stale(input: &Path, output: &Path) -> Result<bool, CookError> {
    let Ok(output_modified) = fs::metadata(output).and_then(|meta| meta.modified()) else {
        return Ok(true);
    };
    let input_modified = fs::metadata(input)
        .and_then(|meta| meta.modified())
        .map_err(|source| CookError::Io {
            path: input.to_owned(),
            source,
        })?;
    Ok(input_modified > output_modified)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_asset_pack_indexes_every_file_by_its_relative_path() {
        let directory = std::env::temp_dir()
            .join("pill_assets_tests")
            .join(format!("{}_asset_pack", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let source = directory.join("res");
        fs::create_dir_all(source.join("models")).expect("scratch directory");
        fs::write(source.join("models").join("a.obj"), b"mesh").expect("input");
        fs::write(source.join("b.png"), b"image").expect("input");
        // A metadata update that crashed part-way through leaves this behind.
        fs::write(
            source.join(".b.png.meta.4242-0.tmp"),
            b"{\"format_version\":1}",
        )
        .expect("input");
        let output = directory.join("out").join("assets.pillpack");

        let packed = write_asset_pack(&source, &output).expect("pack");
        let pack = fs::read(&output).expect("written");

        assert_eq!(packed.len(), 2);
        assert_eq!(&pack[..8], ASSET_PACK_MAGIC);
        assert_eq!(&pack[8..12], &ASSET_PACK_VERSION.to_le_bytes());
        assert_eq!(&pack[12..16], &2u32.to_le_bytes());
        // Sorted, so `b.png` comes before `models/a.obj`; the file bytes come
        // after the index.
        assert_eq!(&pack[16..20], &5u32.to_le_bytes());
        assert_eq!(&pack[20..25], b"b.png");
        assert!(pack.ends_with(b"imagemesh"));
        assert!(
            !pack.windows(7).any(|window| window == b"b.png.m"),
            "the leftover metadata temp file is not packed"
        );
        let _ = fs::remove_dir_all(&directory);
    }

    /// `res/editor/` is the editor's own content, and a game pack never
    /// carries it. A game file that only starts with the same letters
    /// ("editorial.txt") still packs.
    #[test]
    fn an_asset_pack_skips_the_editor_directory() {
        let directory = scratch("editor_directory");
        let source = directory.join("res");
        fs::create_dir_all(source.join("editor").join("fonts")).expect("scratch directory");
        fs::create_dir_all(source.join("ui")).expect("scratch directory");
        fs::write(source.join("editor").join("editor.theme"), b"{}").expect("input");
        fs::write(source.join("editor").join("fonts").join("a.ttf"), b"font").expect("input");
        fs::write(source.join("ui").join("accent.theme"), b"{}").expect("input");
        fs::write(source.join("editorial.txt"), b"prose").expect("input");
        let output = directory.join("assets.pillpack");

        let packed = write_asset_pack(&source, &output).expect("pack");
        let pack = fs::read(&output).expect("written");

        let keys: Vec<String> = packed
            .iter()
            .map(|path| {
                path.strip_prefix(&source)
                    .expect("below res")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(keys, ["editorial.txt", "ui/accent.theme"]);
        assert!(!pack.windows(12).any(|window| window == b"editor.theme"));
        let _ = fs::remove_dir_all(&directory);
    }

    /// A module ships its resources beside its code, not in the project's
    /// `res`. A packed build has no module directory to resolve them from, so
    /// the pack carries them under names relative to their own root.
    #[test]
    fn a_module_roots_files_are_packed_beside_the_projects() {
        let directory = scratch("module_roots");
        let source = directory.join("res");
        fs::create_dir_all(&source).expect("scratch directory");
        fs::write(source.join("project.txt"), b"project").expect("input");
        let resources = directory.join("module").join("res");
        fs::create_dir_all(resources.join("defaults")).expect("scratch directory");
        fs::write(
            resources.join("defaults").join("pill.pbr.material"),
            b"material",
        )
        .expect("input");
        let shaders = directory.join("module").join("shader_sources");
        fs::create_dir_all(shaders.join("pbr").join("shaders")).expect("scratch directory");
        fs::write(shaders.join("pbr").join("shaders").join("x.wgsl"), b"stage").expect("input");
        let output = directory.join("assets.pillpack");

        let packed =
            write_asset_pack_with_roots(&source, &[resources.clone(), shaders.clone()], &output)
                .expect("pack");
        let pack = fs::read(&output).expect("written");

        assert_eq!(packed.len(), 3);
        assert_eq!(&pack[12..16], &3u32.to_le_bytes());
        for key in [
            "project.txt",
            "defaults/pill.pbr.material",
            "pbr/shaders/x.wgsl",
        ] {
            assert!(
                pack.windows(key.len())
                    .any(|window| window == key.as_bytes()),
                "{key} is indexed"
            );
        }
        let _ = fs::remove_dir_all(&directory);
    }

    /// A project file with a module resource's name belongs to the project:
    /// the store tries the project's directory before a module root, and the
    /// flat index cannot hold both.
    #[test]
    fn a_project_file_shadows_a_module_resource_of_the_same_name() {
        let directory = scratch("shadowed_module_resource");
        let source = directory.join("res");
        fs::create_dir_all(source.join("defaults")).expect("scratch directory");
        fs::write(source.join("defaults").join("x.material"), b"project").expect("input");
        let resources = directory.join("module").join("res");
        fs::create_dir_all(resources.join("defaults")).expect("scratch directory");
        fs::write(resources.join("defaults").join("x.material"), b"module").expect("input");
        let output = directory.join("assets.pillpack");

        let packed = write_asset_pack_with_roots(&source, &[resources], &output).expect("pack");
        let pack = fs::read(&output).expect("written");

        assert_eq!(packed, [source.join("defaults").join("x.material")]);
        assert!(pack.windows(7).any(|window| window == b"project"));
        assert!(!pack.windows(6).any(|window| window == b"module"));
        let _ = fs::remove_dir_all(&directory);
    }

    /// When two roots ship one name, the first one keeps it, as the first
    /// declared root answers a lookup first.
    #[test]
    fn the_first_module_root_wins_a_name_two_roots_ship() {
        let directory = scratch("overlapping_module_roots");
        let first = directory.join("first");
        let second = directory.join("second");
        fs::create_dir_all(&first).expect("scratch directory");
        fs::create_dir_all(&second).expect("scratch directory");
        fs::write(first.join("shared.txt"), b"first").expect("input");
        fs::write(second.join("shared.txt"), b"second").expect("input");
        let output = directory.join("assets.pillpack");

        let packed = write_asset_pack_with_roots(
            &directory.join("missing_res"),
            &[first.clone(), second],
            &output,
        )
        .expect("pack");
        let pack = fs::read(&output).expect("written");

        assert_eq!(packed, [first.join("shared.txt")]);
        assert!(pack.windows(5).any(|window| window == b"first"));
        assert!(!pack.windows(6).any(|window| window == b"second"));
        let _ = fs::remove_dir_all(&directory);
    }

    /// A project with no `res` still ships what its modules declare.
    #[test]
    fn a_missing_project_directory_still_packs_module_roots() {
        let directory = scratch("module_only_pack");
        let resources = directory.join("module").join("res");
        fs::create_dir_all(&resources).expect("scratch directory");
        fs::write(resources.join("defaults.material"), b"material").expect("input");
        let output = directory.join("assets.pillpack");

        let packed = write_asset_pack_with_roots(
            &directory.join("res"),
            std::slice::from_ref(&resources),
            &output,
        )
        .expect("pack");
        let pack = fs::read(&output).expect("written");

        assert_eq!(packed, [resources.join("defaults.material")]);
        assert_eq!(&pack[12..16], &1u32.to_le_bytes());
        let _ = fs::remove_dir_all(&directory);
    }

    /// Creates an empty `<temp>/pill_assets_tests/<pid>_<name>` directory and
    /// returns it.
    fn scratch(name: &str) -> PathBuf {
        let directory = std::env::temp_dir()
            .join("pill_assets_tests")
            .join(format!("{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("scratch directory");
        directory
    }

    /// A rule that records the inputs it was asked to build.
    struct RecordingRule {
        built_inputs: std::cell::RefCell<Vec<PathBuf>>,
    }

    impl Rule for RecordingRule {
        fn name(&self) -> &'static str {
            "recording"
        }

        fn input_glob(&self) -> &'static str {
            "shaders/*.in"
        }

        fn output_for(&self, input: &Path) -> PathBuf {
            input.with_extension("out")
        }

        fn build(&self, input: &Path, output: &Path) -> Result<(), CookError> {
            fs::write(output, b"cooked").map_err(|source| CookError::Io {
                path: output.to_owned(),
                source,
            })?;
            self.built_inputs.borrow_mut().push(input.to_owned());
            Ok(())
        }
    }

    #[test]
    fn expand_matches_only_the_extension_and_the_named_directory() {
        let root = scratch("expand");
        fs::create_dir_all(root.join("shaders")).expect("shaders");
        fs::create_dir_all(root.join("elsewhere")).expect("elsewhere");
        fs::write(root.join("shaders/a.hlsl"), b"").expect("a");
        fs::write(root.join("shaders/b.hlsl"), b"").expect("b");
        fs::write(root.join("shaders/note.txt"), b"").expect("note");
        fs::write(root.join("elsewhere/c.hlsl"), b"").expect("c");

        let matched = expand(&root, "shaders/*.hlsl").expect("expansion");

        let names: Vec<_> = matched
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.hlsl", "b.hlsl"]);
    }

    #[test]
    fn a_pattern_without_a_directory_matches_the_root_itself() {
        let root = scratch("flat");
        fs::create_dir_all(root.join("include")).expect("include");
        fs::write(root.join("a.hlsl"), b"").expect("a");
        fs::write(root.join("include/header.hlsl"), b"").expect("header");

        let matched = expand(&root, "*.hlsl").expect("expansion");

        let names: Vec<_> = matched
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // The nested header is not a match: the pattern is one level deep even
        // when that level is the root.
        assert_eq!(names, ["a.hlsl"]);
    }

    #[test]
    fn a_missing_directory_is_not_a_failure() {
        let root = scratch("missing");

        assert!(expand(&root, "shaders/*.hlsl")
            .expect("expansion")
            .is_empty());
    }

    #[test]
    fn run_rebuilds_only_stale_outputs() {
        let root = scratch("stale");
        fs::create_dir_all(root.join("shaders")).expect("shaders");
        fs::write(root.join("shaders/triangle.in"), b"source").expect("input");
        let rule = RecordingRule {
            built_inputs: std::cell::RefCell::new(Vec::new()),
        };
        let pipeline = Pipeline {
            root: root.clone(),
            rules: vec![Box::new(rule)],
        };

        let first = pipeline.run().expect("first run");
        assert_eq!(first.rebuilt_outputs.len(), 1);
        assert!(first.skipped_outputs.is_empty());

        // Nothing changed, so the second run leaves the output alone.
        let second = pipeline.run().expect("second run");
        assert!(second.rebuilt_outputs.is_empty());
        assert_eq!(second.skipped_outputs.len(), 1);
        assert_eq!(second.discovered_inputs, first.discovered_inputs);
    }

    /// A rule whose tool leaves an identical output untouched must not leave
    /// it stale. The next run has to skip it instead of cooking it again.
    #[test]
    fn an_output_the_rule_left_untouched_is_fresh_afterwards() {
        /// Cooks `shaders/*.in`, writing the output only when it is missing.
        struct WritesOnlyWhenMissing;

        impl Rule for WritesOnlyWhenMissing {
            fn name(&self) -> &'static str {
                "writes_only_when_missing"
            }

            fn input_glob(&self) -> &'static str {
                "shaders/*.in"
            }

            fn output_for(&self, input: &Path) -> PathBuf {
                input.with_extension("out")
            }

            fn build(&self, _input: &Path, output: &Path) -> Result<(), CookError> {
                if !output.exists() {
                    fs::write(output, b"cooked").map_err(|source| CookError::Io {
                        path: output.to_owned(),
                        source,
                    })?;
                }
                Ok(())
            }
        }

        let root = scratch("untouched");
        fs::create_dir_all(root.join("shaders")).expect("shaders");
        let input = root.join("shaders/triangle.in");
        fs::write(&input, b"source").expect("input");
        let pipeline = Pipeline {
            root: root.clone(),
            rules: vec![Box::new(WritesOnlyWhenMissing)],
        };
        pipeline.run().expect("first run");

        // Make the output older than the input, as an edit to the input would.
        // The rule will see an existing output and leave it alone.
        let output = input.with_extension("out");
        let earlier = std::time::SystemTime::now() - Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(&output)
            .and_then(|file| file.set_modified(earlier))
            .expect("age the output");
        let second = pipeline.run().expect("second run");
        assert_eq!(second.rebuilt_outputs.len(), 1);

        // Without the stamp the output would still be older than the input and
        // this run would cook it again.
        let third = pipeline.run().expect("third run");
        assert!(third.rebuilt_outputs.is_empty());
        assert_eq!(third.skipped_outputs.len(), 1);
    }

    /// A rule can declare inputs its glob does not match - the shader rule's
    /// `#include` headers - and a newer one has to make the output stale.
    #[test]
    fn a_newer_extra_input_rebuilds_the_output() {
        /// Cooks `shaders/*.in`, re-cooked when `include/` changes.
        struct WithIncludes;

        impl Rule for WithIncludes {
            fn name(&self) -> &'static str {
                "with_includes"
            }

            fn input_glob(&self) -> &'static str {
                "shaders/*.in"
            }

            fn output_for(&self, input: &Path) -> PathBuf {
                input.with_extension("out")
            }

            fn extra_inputs(&self, input: &Path) -> Result<Vec<PathBuf>, CookError> {
                let parent = input.parent().expect("matched inputs live in a directory");
                walk_files(&parent.join("include"))
            }

            fn build(&self, input: &Path, output: &Path) -> Result<(), CookError> {
                fs::copy(input, output).map_err(|source| CookError::Io {
                    path: output.to_owned(),
                    source,
                })?;
                Ok(())
            }
        }

        let root = scratch("extra_inputs");
        fs::create_dir_all(root.join("shaders/include")).expect("shaders");
        fs::write(root.join("shaders/triangle.in"), b"source").expect("input");
        let header = root.join("shaders/include/common.h");
        fs::write(&header, b"header").expect("header");
        let pipeline = Pipeline {
            root: root.clone(),
            rules: vec![Box::new(WithIncludes)],
        };

        let first = pipeline.run().expect("first run");
        assert_eq!(first.rebuilt_outputs.len(), 1);

        // Nothing moved: the matched input is untouched, so the output stays.
        let second = pipeline.run().expect("second run");
        assert!(second.rebuilt_outputs.is_empty());
        assert_eq!(second.skipped_outputs.len(), 1);

        // The header the rule reads is newer than the output, so the next run
        // cooks again even though the matched input never moved. The timestamp
        // is set by hand: a file written a moment later can still carry the
        // same modification time as the output (file times are quantized), and
        // the staleness check is strict.
        fs::write(&header, b"header v2").expect("header edit");
        let newer = std::time::SystemTime::now() + Duration::from_secs(60);
        fs::File::options()
            .write(true)
            .open(&header)
            .and_then(|file| file.set_modified(newer))
            .expect("age the header");
        let third = pipeline.run().expect("third run");
        assert_eq!(third.rebuilt_outputs.len(), 1);
    }

    #[test]
    fn the_manifest_adds_only_the_rules_it_names() {
        let root = scratch("manifest");
        let manifest = root.join("cook.rules");
        fs::write(
            &manifest,
            "# meshes stay uncooked here\n\nobj_to_cooked_mesh  # and so does this one\n",
        )
        .expect("manifest");

        let pipeline = Pipeline::new(root.clone())
            .with_manifest(&manifest, |name| {
                (name == "obj_to_cooked_mesh").then(|| {
                    Box::new(RecordingRule {
                        built_inputs: std::cell::RefCell::new(Vec::new()),
                    }) as Box<dyn Rule>
                })
            })
            .expect("manifest resolves");

        let names: Vec<_> = pipeline.rules.iter().map(|rule| rule.name()).collect();
        assert_eq!(names, ["hlsl_to_wgsl", "recording"]);

        // An unknown name is a hard error rather than a silently skipped rule.
        fs::write(&manifest, "nonsense\n").expect("manifest");
        assert!(matches!(
            Pipeline::new(root).with_manifest(&manifest, |_| None),
            Err(CookError::UnknownRule { .. })
        ));
    }

    #[test]
    fn a_manifest_that_is_not_there_leaves_the_rules_alone() {
        let root = scratch("no_manifest");

        let pipeline = Pipeline::new(root.clone())
            .with_manifest(&root.join("absent.rules"), |_| None)
            .expect("absent manifest");

        assert_eq!(pipeline.rules.len(), default_rules().len());
    }
}
