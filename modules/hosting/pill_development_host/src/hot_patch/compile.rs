//! Capturing and replaying the exact `rustc` invocation cargo uses.
//!
//! # Responsibilities
//!
//! - Ask cargo once, in verbose mode, for the command it builds a crate with.
//! - Replay that command with a different input file and output path.
//! - Cache the result, invalidated by the build command and the inputs'
//!   modification times.
//!
//! # Design
//!
//! Working out `--extern` and `-L native` by hand is the most fragile part of
//! a patch pipeline. It means naming one rlib metadata hash and every registry
//! path, so it breaks whenever a dependency changes and cannot run on another
//! machine. Cargo already knows the right answer, so this asks it instead of
//! guessing. A patch then links exactly the artifacts the running module
//! linked, which is what keeps type layout AND `TypeId` identical across the
//! boundary.
//!
//! ```ignore
//! let line = CargoRustcLine::capture(&workspace, "project", &lib_rs, &build_command)?;
//! let output = line
//!     .replay(&patch_source, &patch_dll, "pill_hotpatch_7", &[project_rlib], None)
//!     .output()?;
//! ```

// Standard library
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

// =============================================================================
// Constants
// =============================================================================

/// The version of the tokenizer that wrote a cached compiler line.
///
/// Increase this whenever [`tokenize`] changes, so a cache written by an
/// earlier tokenizer is captured again instead of replayed. Nothing else in the
/// cache key can notice the change: fixing the parser leaves the manifests, the
/// lockfile and the build command exactly as they were.
const CACHE_FORMAT_VERSION: &str = "pill-hotpatch-flags-v3";

/// The longest time one flag capture may take before it gives up.
///
/// This is the same limit the module build's watchdog uses, because the capture
/// builds the same workspace. If a real build holds the package lock that long,
/// something is wrong, and the edit falls back to a full reload either way.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(120);

/// How often the capture checks whether its cargo has finished.
const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Flags whose value is the next token (`-C` `opt-level=0`), so the parser must
/// take two tokens to keep the flag and its value together.
const TWO_TOKEN_FLAGS: &[&str] = &[
    "-C",
    "-L",
    "-l",
    "-Z",
    "--extern",
    "--crate-name",
    "--crate-type",
    "--out-dir",
    "--target",
    "--check-cfg",
    "--cfg",
    "--edition",
    "--error-format",
    "--emit",
    "--sysroot",
];

/// `-C` values that are dropped when a patch is built.
///
/// - `metadata` and `extra-filename` describe the original crate, so keeping
///   them would make the patch collide with it.
/// - `incremental` makes a single-file patch slower. The patch is one codegen
///   unit that changes on every save, so the incremental cache never matches
///   and only adds bookkeeping.
const DROPPED_CODEGEN_VALUES: &[&str] = &["metadata=", "extra-filename=", "incremental="];

/// Whole flags that are dropped when a patch is built, together with their
/// value token when they have one.
///
/// The diagnostics flags go so `rustc` prints normal, readable errors into the
/// host console. `--emit` and `--out-dir` are replaced by a plain `-o`. The
/// crate identity flags are replaced by the patch's own.
const DROPPED_FLAGS: &[&str] = &[
    "--error-format",
    "--json",
    "--emit",
    "--out-dir",
    "--crate-name",
    "--crate-type",
];

/// The linker [`patch_linker_value`] uses in place of a recorded linker that
/// is not LLD.
///
/// rustc finds `rust-lld` in the active toolchain's sysroot
/// (`lib/rustlib/<target>/bin`), so nothing has to be installed and a
/// toolchain switch is followed. It is also the linker `modules/.cargo/config.toml`
/// asks every non-dx build for the MSVC target in this repository to use.
const PATCH_LINKER: &str = "rust-lld";

// =============================================================================
// CargoRustcLine
// =============================================================================

/// One captured `rustc` command line, ready to be replayed for a patch build.
#[derive(Debug, Clone, PartialEq)]
pub struct CargoRustcLine {
    /// Absolute path to the `rustc` cargo actually used, so a toolchain switch
    /// is followed rather than guessed.
    pub program: String,
    /// Every argument after the compiler, including the original source path.
    /// A replay drops that path and appends the patch's own source file.
    pub args: Vec<String>,
}

impl CargoRustcLine {
    // -------------------------------------------------------------------------
    // Capture
    // -------------------------------------------------------------------------

    /// Asks cargo for the `rustc` line it uses to build `package`'s library.
    ///
    /// Cargo prints the command only when it compiles the crate. So an
    /// up-to-date crate root is touched to force one rebuild, and the build is
    /// tried again. `build_command` MUST be the host's own command for this
    /// crate, feature flags included: a bare `cargo build -p <package>` line
    /// links a differently-configured `pill_engine`, which gives every patch
    /// `TypeId` a different value.
    ///
    /// # Errors
    ///
    /// Returns a message when cargo cannot run, the build fails, the crate root
    /// cannot be touched, or no invocation for `package` appears after forcing
    /// a rebuild.
    pub fn capture(
        workspace_dir: &Path,
        package: &str,
        crate_root: &Path,
        build_command: &[String],
    ) -> Result<Self, String> {
        // Step 1: Ask cargo for an invocation without changing anything.
        if let Some(line) = Self::try_capture(workspace_dir, package, build_command)? {
            return Ok(line);
        }

        // Step 2: Cargo stayed silent because the crate was fresh. Touch the
        // root source to force exactly one rebuild of this crate, then ask
        // again.
        touch(crate_root)?;
        Self::try_capture(workspace_dir, package, build_command)?.ok_or_else(|| {
            format!(
                "cargo -v printed no rustc invocation for `{package}` even after \
                 forcing a rebuild of {}",
                crate_root.display()
            )
        })
    }

    /// Runs one verbose build and returns the invocation, if cargo printed one.
    fn try_capture(
        workspace_dir: &Path,
        package: &str,
        build_command: &[String],
    ) -> Result<Option<Self>, String> {
        Self::try_capture_within(workspace_dir, package, build_command, CAPTURE_TIMEOUT)
    }

    /// [`Self::try_capture`] with an explicit deadline, so a test can watch one
    /// expire without waiting two minutes for it.
    fn try_capture_within(
        workspace_dir: &Path,
        package: &str,
        build_command: &[String],
        timeout: Duration,
    ) -> Result<Option<Self>, String> {
        // Step 1: Reconstruct the host's own command, environment included.
        let (program, arguments) = build_command
            .split_first()
            .ok_or_else(|| "the build command is empty".to_string())?;

        let mut command = Command::new(program);
        command.args(arguments).current_dir(workspace_dir);
        // Both the full reload path and this flag capture run the module's
        // configured cargo command, and both must match the host binary that is
        // running now. So re-apply everything the reload path applies: the
        // spawned-build environment (profile-driven `RUSTFLAGS` handling, and
        // the mirror of dx's `RUSTC_WORKSPACE_WRAPPER` under the dioxus CLI),
        // plus the shared overrides (the private target directory, the host's
        // explicit engine features and the custom profile definition).
        //
        // The profile definition matters: without it cargo rejects a profile
        // the launcher injected, such as `desktop-dev`. The rest matters
        // because a different environment compiles the crate with different
        // codegen flags or a different wrapper hash than the module had. That
        // changes every metadata hash, so the captured `--extern` paths no
        // longer match the staged rlib, rustc reports `error[E0463]`, and every
        // edit falls back to a full reload.
        if program == "cargo" {
            command.envs(crate::config::spawned_build_environment());
            crate::build_runner::apply_cargo_host_overrides(&mut command, workspace_dir);
        }
        // Step 2: Start cargo and read its stderr while the poll loop runs.
        //
        // This code runs on the frame thread, so the child is watched, not
        // waited on. A cargo stuck behind another build's package lock must not
        // freeze the editor for as long as that build takes. A separate thread
        // reads stderr because a pipe nobody reads fills up and blocks the
        // compiler.
        crate::build_runner::prepare_process_tree(&mut command);
        let process_tree = crate::build_runner::create_process_tree();
        let mut child = command
            .arg("-v")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot run cargo in {}: {error}", workspace_dir.display()))?;
        crate::build_runner::join_process_tree(process_tree.as_ref(), &child);
        let mut stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| "cargo's stderr was not captured".to_string())?;
        let stderr_reader = std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = stderr_pipe.read_to_end(&mut text);
            text
        });

        // Step 3: Poll until the child exits or the deadline expires.
        let deadline = pill_core::platform::Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|error| {
                format!(
                    "cannot wait for cargo in {}: {error}",
                    workspace_dir.display()
                )
            })? {
                break status;
            }
            if pill_core::platform::Instant::now() >= deadline {
                // Stop the whole process tree. Cargo's children keep the
                // stderr pipe open, so joining the reader before them would
                // wait for the very build this deadline is meant to stop.
                crate::build_runner::stop_process_tree(process_tree.as_ref(), &mut child);
                let _ = stderr_reader.join();
                return Err(format!(
                    "cargo build -p {package} did not finish within {}s, so its compiler flags could not be captured",
                    timeout.as_secs()
                ));
            }
            std::thread::sleep(CAPTURE_POLL_INTERVAL);
        };
        let stderr = stderr_reader
            .join()
            .map_err(|_| "the cargo stderr reader panicked".to_string())?;

        // Step 4: Pick this package's invocation out of cargo's stderr.
        //
        // Cargo writes the `Running` lines to stderr.
        let stderr = String::from_utf8_lossy(&stderr);
        if !status.success() {
            return Err(format!("cargo build -p {package} failed:\n{stderr}"));
        }

        // Several crates may be rebuilt; take the one that names this package as
        // its crate. `--crate-name <package>` identifies it unambiguously
        // because cargo builds one library target per package here.
        Ok(stderr
            .lines()
            .find_map(|raw| parse_rustc_line(raw, package)))
    }

    // -------------------------------------------------------------------------
    // Replay
    // -------------------------------------------------------------------------

    /// Builds the `rustc` command that compiles `input` into a patch cdylib at
    /// `output`, reusing every dependency flag cargo resolved.
    ///
    /// The captured line's crate identity, diagnostics format, emit list and
    /// output directory are replaced. Everything that describes the dependency
    /// graph (`--extern`, `-L`, `--cfg`, `-C prefer-dynamic`) is carried over
    /// unchanged; the linker value is the one exception, and
    /// [`patch_linker_value`] explains why. Keeping the dependency graph
    /// identical is what gives the patch the same view of every type - layout
    /// AND `TypeId` - as the running host.
    ///
    /// `extra_externs` are additional `name=path` entries appended as
    /// `--extern`. A patch needs at least the patched crate's own rlib, so its
    /// generated source can `use project::*` and get the identical types the
    /// running module holds.
    pub fn replay(
        &self,
        input: &Path,
        output: &Path,
        crate_name: &str,
        extra_externs: &[String],
        staged_dependencies: Option<&Path>,
    ) -> Command {
        let mut command = Command::new(&self.program);
        command.args(self.replay_args(
            input,
            output,
            crate_name,
            extra_externs,
            staged_dependencies,
        ));
        command
    }

    /// Returns the arguments `replay` passes to `rustc`. Tests use it, and it
    /// can be logged to show the exact command line.
    ///
    /// When `staged_dependencies` is given, every `--extern` entry that names a
    /// shared per-crate slot is pointed at the copy staged there, when that
    /// copy exists. Any build of the same crate name overwrites those shared
    /// slots in place, whatever its feature set, so linking one directly can
    /// make a patch fail with `error[E0463]` after an unrelated `cargo build`.
    /// Hash-qualified paths already name one exact configuration and are left
    /// alone. A `.rmeta` entry is first pointed at the `.rlib` beside it; see
    /// [`redirect_extern`].
    pub fn replay_args(
        &self,
        input: &Path,
        output: &Path,
        crate_name: &str,
        extra_externs: &[String],
        staged_dependencies: Option<&Path>,
    ) -> Vec<String> {
        let mut arguments = Vec::with_capacity(self.args.len() + 8);
        let mut index = 0;

        // Step 1: Put the staged dependency directory ahead of cargo's own.
        //
        // This must come before every path cargo recorded, so rustc finds the
        // staged copies first. It is a different job from redirecting
        // `--extern` below, and both are needed. `--extern` only covers crates
        // the patch source names itself. The module rlib's own dependencies are
        // found by searching the `-L` paths for a crate with the right name and
        // metadata hash. If the shared `deps` directory stays first, rustc
        // finds whichever variant was written there last, rejects it, and
        // reports `error[E0463]: can't find crate for <the module>`. That blames
        // the module instead of the dependency that actually moved.
        if let Some(directory) = staged_dependencies {
            arguments.push("-L".to_string());
            arguments.push(format!("dependency={}", directory.display()));
        }

        // Step 2: Replay the recorded tokens, dropping what is replaced below.
        while index < self.args.len() {
            let token = &self.args[index];
            let has_separate_value =
                TWO_TOKEN_FLAGS.contains(&token.as_str()) && index + 1 < self.args.len();

            // Drop the flags whose replacements are appended below. Both the
            // `--flag value` and `--flag=value` spellings appear in cargo output.
            let flag_without_value = token.split('=').next().unwrap_or(token);
            if DROPPED_FLAGS.contains(&flag_without_value) {
                index += if has_separate_value { 2 } else { 1 };
                continue;
            }

            // `-C metadata=...`, `-C extra-filename=...`, `-C incremental=...`
            if token == "-C" && has_separate_value {
                let flag_value = &self.args[index + 1];
                if DROPPED_CODEGEN_VALUES
                    .iter()
                    .any(|prefix| flag_value.starts_with(prefix))
                {
                    index += 2;
                    continue;
                }
            }

            // The original crate's source file is a positional argument; the
            // patch supplies its own.
            if !token.starts_with('-') && token.ends_with(".rs") {
                index += 1;
                continue;
            }

            arguments.push(token.clone());
            if has_separate_value {
                let flag_value = &self.args[index + 1];
                arguments.push(match (token.as_str(), staged_dependencies) {
                    ("--extern", Some(directory)) => redirect_extern(flag_value, directory),
                    // The patch is linked by the toolchain's own LLD even when
                    // cargo recorded a different linker; see
                    // [`patch_linker_value`].
                    ("-C", _) => patch_linker_value(flag_value),
                    _ => flag_value.clone(),
                });
            }
            index += if has_separate_value { 2 } else { 1 };
        }

        // Step 3: Append anything the patch needs beyond what the original
        // crate linked - principally that crate's own rlib.
        for extern_entry in extra_externs {
            arguments.push("--extern".to_string());
            arguments.push(extern_entry.clone());
        }

        // Step 4: Append the patch's own build settings and output.
        //
        // No PDB for the patch.
        //
        // Linking takes most of a patch's wall time, because the linker reads
        // the whole engine closure - tens of megabytes of `pill_engine` and
        // `pill_core` - to emit one function. Most of that bulk is debug info,
        // and building a PDB writes it back to disk on every save.
        // `-C debuginfo=0` does NOT avoid that: the debug info lives in the
        // inputs, not in the patch, so only telling the linker to skip the PDB
        // entirely helps.
        //
        // The trade is deliberate: patched code has no debugger symbols. A
        // patch is generated code that exists for a few seconds, and a
        // prologue-patched function already defeats breakpoints set on it.
        //
        // This applies to MSVC only, because `/DEBUG:NONE` is a link.exe and
        // lld-link flag. Other targets keep whatever cargo recorded.
        #[cfg(target_env = "msvc")]
        {
            arguments.push("-C".to_string());
            arguments.push("link-arg=/DEBUG:NONE".to_string());
        }

        // The patch's own identity and output.
        arguments.push("--crate-name".to_string());
        arguments.push(crate_name.to_string());
        arguments.push("--crate-type".to_string());
        arguments.push("cdylib".to_string());
        arguments.push("-o".to_string());
        arguments.push(output.display().to_string());
        arguments.push(input.display().to_string());
        arguments
    }

    // -------------------------------------------------------------------------
    // Cache
    // -------------------------------------------------------------------------

    /// Saves the captured line, so a later host process can skip the cargo
    /// probe.
    ///
    /// The file holds the format version on one line, the build command on the
    /// next, the program on the next, and then one argument per line. No token
    /// contains a newline (they are paths and flags), so nothing needs
    /// escaping.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the cache directory cannot be created or the
    /// file cannot be written.
    pub fn save(&self, path: &Path, build_command: &[String]) -> std::io::Result<()> {
        let mut text = String::with_capacity(4096);
        // Records which tokenizer wrote this file. A cache written by an older
        // tokenizer holds tokens split the wrong way, and nothing else in the
        // key would notice: fixing the parser leaves the manifests, the
        // lockfile and the build command unchanged. This version makes an old
        // cache be captured again instead of replayed.
        text.push_str(CACHE_FORMAT_VERSION);
        text.push('\n');
        // The command this was captured for, so a feature change invalidates it.
        text.push_str(&build_command.join("\u{1}"));
        text.push('\n');
        text.push_str(&self.program);
        text.push('\n');
        for argument in &self.args {
            text.push_str(argument);
            text.push('\n');
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)
    }

    /// Loads a line saved by [`Self::save`].
    ///
    /// Returns `None` when the cache is missing, when any input that could
    /// change the flags is newer than the cache or cannot be read, when the
    /// file was written by a different tokenizer, or when `build_command`
    /// differs from the one the cache was captured with.
    ///
    /// The build command is part of the key because feature flags live there
    /// and nowhere else. Changing them changes crate metadata and therefore
    /// every `TypeId`, while `Cargo.toml`, `Cargo.lock` and the sources stay
    /// untouched. Keying on modification times alone would happily replay flags
    /// for an engine built with different features.
    pub fn load_if_fresh(
        path: &Path,
        inputs: &[PathBuf],
        build_command: &[String],
    ) -> Option<Self> {
        let cached_at = std::fs::metadata(path).ok()?.modified().ok()?;
        for input in inputs {
            let modified_at = std::fs::metadata(input).ok()?.modified().ok()?;
            if modified_at > cached_at {
                return None;
            }
        }
        let text = std::fs::read_to_string(path).ok()?;
        let mut lines = text.lines();

        // Written by a different tokenizer, so its tokens cannot be trusted.
        if lines.next()? != CACHE_FORMAT_VERSION {
            return None;
        }
        // Then the command this was captured for.
        let recorded_command = lines.next()?;
        if recorded_command != build_command.join("\u{1}") {
            return None;
        }
        let program = lines.next()?.to_string();
        Some(Self {
            program,
            args: lines.map(|line| line.to_string()).collect(),
        })
    }
}

// =============================================================================
// Free functions
// =============================================================================

/// Returns where a package's captured flags are cached between processes.
///
/// The file lives in the workspace's private build tree, not in the system
/// temporary directory. The temporary directory is shared by every checkout of
/// this repository on the machine, so a cache there could be replayed against
/// another checkout's dependency graph. That is exactly the metadata and
/// `TypeId` drift the rest of this subsystem prevents. Keying on the workspace
/// root makes a foreign cache unreachable instead of merely detectable.
///
/// Two writers share this path: the fast-patch pipeline, which captures flags
/// when it needs them, and [`crate::build_runner::run_build_command`], which
/// takes the same line out of a build it runs anyway.
pub(crate) fn flags_cache_path(workspace_root: &Path, package: &str) -> PathBuf {
    workspace_root
        .join(crate::config::MODULE_BUILD_TARGET_DIRECTORY)
        .join("hotpatch-flags")
        .join(format!("{package}.flags"))
}

/// Returns the invocation for `crate_name` from one line of cargo's `-v`
/// output.
///
/// Returns `None` for every line that is not a `Running \`rustc ...\`` naming
/// that crate, so a caller can pass cargo's whole stderr stream one line at a
/// time. Any rustc wrapper cargo ran (sccache) is unwrapped, exactly as the
/// on-demand capture does. The patch replays the compiler directly, and the
/// wrapper's own CLI would reject rustc's arguments.
pub(crate) fn parse_rustc_line(
    cargo_output_line: &str,
    crate_name: &str,
) -> Option<CargoRustcLine> {
    let invocation = extract_backticked(cargo_output_line)?;
    if !invocation.contains("rustc") {
        return None;
    }
    let tokens = tokenize(&invocation);
    if tokens.len() < 2 {
        return None;
    }
    let names_this_crate = tokens
        .windows(2)
        .any(|pair| pair[0] == "--crate-name" && pair[1] == crate_name);
    if !names_this_crate {
        return None;
    }
    let compiler_index = compiler_token_index(&tokens);
    Some(CargoRustcLine {
        program: tokens[compiler_index].clone(),
        args: tokens[compiler_index + 1..].to_vec(),
    })
}

/// Returns the linker value the patch is built with, given the one cargo
/// recorded.
///
/// Every `-C` value other than `linker=` is returned unchanged, and so is a
/// `linker=` that already names an LLD.
///
/// The workspace asks for `rust-lld` (`modules/.cargo/config.toml`), but the
/// editor launcher forces `link.exe` for the whole dx session, whose linker
/// proxy cannot run LLD. The patch is not linked by dx - the host runs `rustc`
/// itself - so the captured flag does not apply to it.
///
/// The choice only affects the ephemeral patch image. Both linkers consume the
/// same objects and import libraries, and a linker that fails refuses the patch
/// rather than producing a wrong one.
fn patch_linker_value(codegen_value: &str) -> String {
    let Some(linker) = codegen_value.strip_prefix("linker=") else {
        return codegen_value.to_string();
    };
    let file_name = Path::new(linker)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(linker)
        .to_ascii_lowercase();
    if file_name.contains("lld") {
        return codegen_value.to_string();
    }
    format!("linker={PATCH_LINKER}")
}

/// Points one `--extern name=path` entry at the artifact a patch can use.
///
/// First, a metadata-only entry (`.rmeta`) is pointed at the `.rlib` beside it:
/// a patch is linked, and rustc refuses to link against metadata alone. If the
/// sibling does not exist, the entry stays as it was.
///
/// Second, a shared per-crate slot is pointed at its staged copy, when that copy
/// exists. Only a `.rlib` whose filename has no `-<16 hex digits>` metadata
/// suffix counts as shared. Everything else is returned unchanged, including
/// entries this cannot parse: an entry it cannot read is linked from its
/// original path, which loses the redirect instead of breaking the command
/// line.
fn redirect_extern(extern_entry: &str, staged_dependencies: &Path) -> String {
    let Some((name, path)) = extern_entry.split_once('=') else {
        return extern_entry.to_string();
    };
    let path = match path.strip_suffix(".rmeta") {
        Some(stem) => {
            let sibling = format!("{stem}.rlib");
            if Path::new(&sibling).is_file() {
                sibling
            } else {
                path.to_string()
            }
        }
        None => path.to_string(),
    };
    let Some(file_name) = Path::new(&path).file_name().and_then(|name| name.to_str()) else {
        return extern_entry.to_string();
    };
    if !crate::build_runner::is_shared_slot_rlib(file_name) {
        return format!("{name}={path}");
    }
    let staged_path = staged_dependencies.join(file_name);
    if !staged_path.is_file() {
        return format!("{name}={path}");
    }
    format!("{name}={}", staged_path.display())
}

/// Updates a file's modification time without changing its contents.
///
/// Writing the same bytes again would also make cargo's mtime-based freshness
/// check see a newer source, but it means writing back bytes read a moment
/// earlier. A save that completes between the read and the write is then
/// silently undone, and the next build compiles source the person has already
/// replaced. Changing only the timestamp leaves the contents to whoever owns
/// them.
fn touch(path: &Path) -> Result<(), String> {
    let file_handle = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| format!("cannot open {} to touch it: {error}", path.display()))?;
    file_handle
        .set_modified(std::time::SystemTime::now())
        .map_err(|error| format!("cannot touch {}: {error}", path.display()))
}

/// Returns the command from cargo's ``Running `...` `` line.
fn extract_backticked(cargo_output_line: &str) -> Option<String> {
    let start_index = cargo_output_line.find('`')? + 1;
    let end_index = cargo_output_line.rfind('`')?;
    if end_index <= start_index {
        return None;
    }
    Some(cargo_output_line[start_index..end_index].to_string())
}

/// Splits a command line into tokens, following cargo's quote grouping.
///
/// Cargo quotes any argument that contains a space or a backslash, which on
/// Windows means most paths. Single quotes only group: cargo escapes nothing
/// inside them, and paths cannot contain a single quote. Double quotes carry
/// backslash escapes, which cargo uses for a value that itself contains a
/// double quote.
fn tokenize(command_line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current_token = String::new();
    // The quote character that opened the group being read, if any. Cargo uses
    // single quotes for most arguments, but switches to double quotes with
    // backslash escapes when the value itself contains a double quote, as
    // `--check-cfg 'cfg(feature, values("rendering"))'` does. If only single
    // quotes counted as grouping, that argument would be split in two; rustc
    // would read the tail as a second input file ("multiple input filenames
    // provided") and every patch of a crate with features would fall back to a
    // full reload.
    let mut opened_by: Option<char> = None;
    let mut escaped = false;
    let mut has_content = false;

    for character in command_line.chars() {
        // A backslash inside double quotes escapes the next character. Inside
        // single quotes cargo does not escape, so a backslash is literal, which
        // matters because every Windows path here is full of them.
        if escaped {
            current_token.push(character);
            has_content = true;
            escaped = false;
            continue;
        }
        if character == '\\' && opened_by == Some('"') {
            escaped = true;
            continue;
        }

        match character {
            '\'' | '"' => match opened_by {
                // Closing the group this character opened.
                Some(opener) if opener == character => opened_by = None,
                // Inside the other kind of quote, so it is literal content.
                Some(_) => {
                    current_token.push(character);
                    has_content = true;
                }
                // An empty '' is still a real (empty) argument.
                None => {
                    opened_by = Some(character);
                    has_content = true;
                }
            },
            character if character.is_whitespace() && opened_by.is_none() => {
                if has_content {
                    tokens.push(std::mem::take(&mut current_token));
                    has_content = false;
                }
            }
            character => {
                current_token.push(character);
                has_content = true;
            }
        }
    }
    if has_content {
        tokens.push(current_token);
    }
    tokens
}

/// Returns the index of the actual `rustc` binary in a captured invocation.
///
/// Returns 0 when none of the tokens names a rustc binary, which keeps a plain
/// invocation - whose first token is the compiler - unchanged.
///
/// Cargo can run rustc through a wrapper (a `RUSTC_WRAPPER` such as sccache),
/// which makes a captured line read `sccache C:\...\rustc.exe --crate-name
/// ...`. The patch replays the captured program directly with rustc arguments,
/// and the wrapper's own CLI rejects those, so the wrapper token is dropped and
/// the first token that names a rustc binary becomes the program.
fn compiler_token_index(tokens: &[String]) -> usize {
    tokens
        .iter()
        .position(|token| {
            Path::new(token)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_ascii_lowercase().contains("rustc"))
        })
        .unwrap_or(0)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_runner::is_shared_slot_rlib;
    use crate::scratch::{date_file, ScratchDirectory};

    /// A trimmed but structurally faithful copy of a real cargo -v line.
    fn sample_cargo_line() -> &'static str {
        "     Running `C:\\rustc.exe --crate-name project --edition=2021 \
         'D:\\proj\\src\\lib.rs' --error-format=json \
         --json=diagnostic-rendered-ansi --crate-type cdylib --emit=dep-info,link \
         -C embed-bitcode=no -C debuginfo=2 --check-cfg 'cfg(docsrs,test)' \
         -C metadata=b74d3ef2a041325c --out-dir 'D:\\proj\\target\\debug\\deps' \
         -C linker=rust-lld -C 'incremental=D:\\proj\\target\\debug\\incremental' \
         -L 'dependency=D:\\proj\\target\\debug\\deps' \
         --extern 'pill_engine=D:\\proj\\target\\debug\\deps\\libpill_engine-319.rlib' \
         --extern 'serde=D:\\proj\\target\\debug\\deps\\libserde-1094916a36d6bb5e.rlib' \
         --extern 'pill_test=D:\\proj\\target\\debug\\deps\\libpill_test.rlib' \
         -C prefer-dynamic -L 'native=C:\\reg\\windows_x86_64_msvc-0.52.6\\lib'`"
    }

    /// A cargo `deps` directory spelled with the host's separators.
    ///
    /// `redirect_extern` reads a file name with the host's path rules, so a
    /// `D:\...` literal has no file name on Linux and would never be redirected.
    /// The directory is never created: only the path's shape matters.
    fn cargo_deps_directory() -> std::path::PathBuf {
        std::env::temp_dir()
            .join("proj")
            .join("target")
            .join("debug")
            .join("deps")
    }

    fn parsed_cargo_line() -> CargoRustcLine {
        let invocation = extract_backticked(sample_cargo_line()).expect("backticked command");
        let tokens = tokenize(&invocation);
        CargoRustcLine {
            program: tokens[0].clone(),
            args: tokens[1..].to_vec(),
        }
    }

    /// Cargo double-quotes an argument whose value contains a double quote and
    /// escapes the inner ones, which `--check-cfg` does for any crate that has
    /// features.
    ///
    /// If only single quotes grouped arguments, this one would be split and
    /// rustc would read the tail as a second input file. The error ("multiple
    /// input filenames provided") says nothing about quoting.
    #[test]
    fn tokenizer_handles_double_quoted_arguments_with_escapes() {
        let tokens =
            tokenize(r#"--check-cfg "cfg(feature, values(\"rendering\"))" --edition=2021"#);
        assert_eq!(
            tokens,
            vec![
                "--check-cfg".to_string(),
                r#"cfg(feature, values("rendering"))"#.to_string(),
                "--edition=2021".to_string(),
            ],
            "the check-cfg value must stay one argument"
        );
    }

    /// A backslash is literal inside single quotes, which every Windows path in
    /// this command line depends on.
    #[test]
    fn tokenizer_keeps_backslashes_in_single_quoted_paths() {
        let tokens = tokenize(r"'D:\\proj\\target\\debug\\deps' next");
        assert_eq!(
            tokens,
            vec![
                r"D:\\proj\\target\\debug\\deps".to_string(),
                "next".to_string()
            ]
        );
    }

    /// One kind of quote inside the other is literal content, not a delimiter.
    #[test]
    fn tokenizer_treats_the_other_quote_as_content() {
        assert_eq!(
            tokenize(r#"'it\"s one token' after"#),
            vec![r#"it\"s one token"#.to_string(), "after".to_string()]
        );
    }

    /// A rustc wrapper (sccache under `RUSTC_WRAPPER`) appears ahead of the
    /// real compiler in cargo's `Running` line. The patch replays the captured
    /// program with rustc arguments, so capture must drop the wrapper and keep
    /// the first token that names a rustc binary - which is the first token on
    /// a plain invocation, leaving that case unchanged.
    #[test]
    fn a_rustc_wrapper_is_unwrapped_to_the_real_compiler() {
        let plain_tokens = tokenize(r"C:\rustc.exe --crate-name project --edition=2021");
        assert_eq!(
            compiler_token_index(&plain_tokens),
            0,
            "a plain invocation already names rustc first"
        );

        let wrapped_tokens = tokenize(
            r"sccache C:\Users\me\.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\rustc.exe --crate-name project --edition=2021",
        );
        assert_eq!(
            compiler_token_index(&wrapped_tokens),
            1,
            "the wrapper token is dropped in favour of the real rustc"
        );

        // No token names rustc: fall back to the first token, which is what a
        // plain invocation already gives rather than a guess.
        let unrelated_tokens = tokenize(r"--crate-name project --edition=2021");
        assert_eq!(compiler_token_index(&unrelated_tokens), 0);
    }

    /// Quotes one argument the way cargo does, in either style.
    ///
    /// Cargo quotes an argument that contains whitespace or a backslash. It
    /// usually uses single quotes, but switches to double quotes with backslash
    /// escapes for some values; cargo emits both spellings for `--check-cfg`.
    /// The tokenizer has to read both, so the round-trip below drives both.
    fn quote_like_cargo(argument: &str, double: bool) -> String {
        if double {
            format!(
                "\"{}\"",
                argument.replace('\\', "\\\\").replace('"', "\\\"")
            )
        } else {
            format!("'{argument}'")
        }
    }

    /// Every argument shape this command line actually carries must survive a
    /// quote-then-tokenize round trip, in both quoting styles.
    ///
    /// A single mis-grouped argument is enough to break a patch: rustc takes the
    /// tail as a second input filename, and the failure never mentions quoting.
    /// A single hand-written case would not catch it, because the broken shape
    /// only appears for `--check-cfg` on a crate that has features - so the
    /// shapes are enumerated instead.
    #[test]
    fn every_argument_shape_survives_a_quoting_round_trip() {
        let arguments = [
            // The hard shape: nested double quotes inside the value.
            r#"cfg(feature, values("rendering"))"#,
            r#"cfg(feature, values("default", "test-hooks", "rendering"))"#,
            // No features, so no nested quotes: this shape stays one token in
            // either quoting style.
            "cfg(docsrs,test)",
            // Windows paths - backslashes everywhere, and spaces in some.
            r"D:\\proj\\target\\debug\\deps",
            r"dependency=D:\\proj\\target\\debug\\deps",
            r"C:\\Program Files\\rustc\\lib",
            r"incremental=D:\\proj\\target\\debug\\incremental",
            // Ordinary flags and values.
            "--edition=2021",
            "embed-bitcode=no",
            "pill_engine=D:\\proj\\libpill_engine-190d6c0e2d2eaf24.rlib",
            // A lone apostrophe inside a double-quoted value.
            r#"it's one argument"#,
        ];

        for argument in arguments {
            for double in [false, true] {
                // A single-quoted group cannot carry a single quote, and cargo
                // would not produce one - skip that impossible pairing.
                if !double && argument.contains('\'') {
                    continue;
                }
                let quoted_argument = quote_like_cargo(argument, double);
                let tokens = tokenize(&quoted_argument);
                assert_eq!(
                    tokens,
                    vec![argument.to_string()],
                    "round trip failed for {argument:?} quoted with {}",
                    if double {
                        "double quotes"
                    } else {
                        "single quotes"
                    }
                );
            }
        }
    }

    /// A whole command line tokenizes into flags and values, never into
    /// fragments.
    ///
    /// The failure this guards is specific and silent: a split argument leaves a
    /// token that is a piece of a value, and rustc treats a token it cannot
    /// recognize as an input filename. Asserting the count is what makes a split
    /// visible - a fragment is always one token too many.
    #[test]
    fn a_realistic_command_line_yields_no_fragments() {
        let command_line = concat!(
            r#"C:\rustc.exe --crate-name project --edition=2021 "#,
            r#"'D:\proj\src\lib.rs' --check-cfg 'cfg(docsrs,test)' "#,
            r#"--check-cfg "cfg(feature, values(\"rendering\"))" "#,
            r#"-C 'incremental=D:\proj\target\debug\incremental' "#,
            r#"-L 'dependency=D:\proj\target\debug\deps'"#,
        );
        let tokens = tokenize(command_line);

        assert_eq!(
            tokens.len(),
            13,
            "a fragment would show up as an extra token: {tokens:#?}"
        );
        // The value a single-quote-only tokenizer would split in half.
        assert!(
            tokens.contains(&r#"cfg(feature, values("rendering"))"#.to_string()),
            "the check-cfg value must be one token: {tokens:#?}"
        );
        // Nothing that rustc would mistake for an input file: exactly one token
        // may end in `.rs`, the real source path. A fragment of a split value
        // would show up as a second one.
        let source_paths: Vec<&String> = tokens
            .iter()
            .filter(|token| token.ends_with(".rs"))
            .collect();
        assert_eq!(
            source_paths.len(),
            1,
            "exactly one token may look like a source file: {source_paths:#?}"
        );
        // A leftover quote in any token means a group was not closed.
        for token in &tokens {
            assert!(
                !token.contains('\'') && !token.starts_with('"'),
                "token carries an unbalanced quote, so grouping went wrong: {token:?}"
            );
        }
    }

    /// The cache is rejected when it was written by a different tokenizer.
    ///
    /// The cache key is the manifests, the lockfile and the build command, and
    /// a parser fix changes none of them - so without the version marker an
    /// existing cache would replay the tokens an earlier parser produced.
    #[test]
    fn a_cache_from_a_different_tokenizer_is_rejected() {
        let directory = ScratchDirectory::new("flags_cache_version");
        let cache = directory.join("flags");
        let inputs: Vec<std::path::PathBuf> = Vec::new();
        let command = vec!["cargo".to_string(), "build".to_string()];

        // Written by the current tokenizer: accepted.
        parsed_cargo_line().save(&cache, &command).expect("save");
        assert!(CargoRustcLine::load_if_fresh(&cache, &inputs, &command).is_some());

        // The same content with an older version marker: refused.
        let text = std::fs::read_to_string(&cache).expect("read");
        let older_version_text = text.replacen(CACHE_FORMAT_VERSION, "pill-hotpatch-flags-v1", 1);
        std::fs::write(&cache, older_version_text).expect("write");
        assert!(
            CargoRustcLine::load_if_fresh(&cache, &inputs, &command).is_none(),
            "a cache from another tokenizer must be re-captured, not replayed"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn tokenizer_groups_single_quoted_paths() {
        let tokens = tokenize("a 'b c' 'd\\e' f");
        assert_eq!(tokens, vec!["a", "b c", "d\\e", "f"]);
    }

    #[test]
    fn capture_finds_program_and_arguments() {
        let line = parsed_cargo_line();
        assert_eq!(line.program, "C:\\rustc.exe");
        assert!(line.args.contains(&"--crate-name".to_string()));
        assert!(line.args.contains(&"-C".to_string()));
    }

    /// A shared per-crate slot is a file cargo overwrites in place. A
    /// hash-qualified name belongs to one exact configuration.
    ///
    /// This distinction decides which `--extern` entries are redirected to a
    /// staged copy. The staging side in `build_runner` must agree with it: if
    /// the two disagree, a file is either staged and never linked, or linked
    /// and never staged.
    #[test]
    fn shared_slots_are_told_apart_from_hash_qualified_artifacts() {
        // Workspace crates: one slot per crate name, overwritten by any build.
        assert!(is_shared_slot_rlib("libpill_test.rlib"));
        assert!(is_shared_slot_rlib("libpill_core.rlib"));
        // A crate name containing a hyphen is still a shared slot; only a
        // trailing 16-hex-digit suffix marks a configuration.
        assert!(is_shared_slot_rlib("libtrait_type-map.rlib"));
        // Hash-qualified, so it already names one configuration.
        assert!(!is_shared_slot_rlib("libpill_engine-190d6c0e2d2eaf24.rlib"));
        // A suffix of the wrong length or with non-hex digits is not a hash.
        assert!(is_shared_slot_rlib("libthing-190d6c0e2d2eaf2.rlib"));
        assert!(is_shared_slot_rlib("libthing-190d6c0e2d2eaf2z.rlib"));
        // Not an rlib at all.
        assert!(!is_shared_slot_rlib("pill_core.dll"));
    }

    /// Only a shared slot with a staged copy is redirected. Everything else is
    /// returned unchanged, so an unparseable entry is linked from its original
    /// path rather than producing a broken command line.
    #[test]
    fn only_staged_shared_slots_are_redirected() {
        let staged_directory = ScratchDirectory::new("staged_externs_test");
        let deps_directory = cargo_deps_directory();
        std::fs::write(staged_directory.join("libpill_test.rlib"), b"staged")
            .expect("write staged copy");

        // Staged shared slot: redirected.
        let redirected_entry = redirect_extern(
            &format!(
                "pill_test={}",
                deps_directory.join("libpill_test.rlib").display()
            ),
            &staged_directory,
        );
        assert_eq!(
            redirected_entry,
            format!(
                "pill_test={}",
                staged_directory.join("libpill_test.rlib").display()
            ),
            "must point into the staging directory"
        );

        // Shared slot with nothing staged: left alone.
        let unredirected_entry = format!(
            "pill_spline={}",
            deps_directory.join("libpill_spline.rlib").display()
        );
        assert_eq!(
            redirect_extern(&unredirected_entry, &staged_directory),
            unredirected_entry
        );

        // Hash-qualified: never redirected, even if a same-named file exists.
        std::fs::write(
            staged_directory.join("libpill_engine-190d6c0e2d2eaf24.rlib"),
            b"staged",
        )
        .expect("write same-named copy");
        let hash_qualified_entry = format!(
            "pill_engine={}",
            deps_directory
                .join("libpill_engine-190d6c0e2d2eaf24.rlib")
                .display()
        );
        assert_eq!(
            redirect_extern(&hash_qualified_entry, &staged_directory),
            hash_qualified_entry
        );

        // Unparseable entries are returned verbatim.
        assert_eq!(
            redirect_extern("no_equals_sign", &staged_directory),
            "no_equals_sign"
        );

        let _ = std::fs::remove_dir_all(&staged_directory);
    }

    /// A `.rmeta` entry is retargeted at the sibling rlib that can be linked,
    /// and only when that sibling exists.
    #[test]
    fn a_metadata_only_extern_prefers_the_rlib_beside_it() {
        let directory = ScratchDirectory::new("rmeta_extern_test");
        let staged_directory = directory.join("staged");
        std::fs::create_dir_all(&staged_directory).expect("create staging dir");
        std::fs::write(
            directory.join("libpill_engine-190d6c0e2d2eaf24.rlib"),
            b"rlib",
        )
        .expect("write sibling rlib");

        // The sibling exists: the entry now names the rlib.
        let retargeted_entry = redirect_extern(
            &format!(
                "pill_engine={}",
                directory
                    .join("libpill_engine-190d6c0e2d2eaf24.rmeta")
                    .display()
            ),
            &staged_directory,
        );
        assert!(
            retargeted_entry.ends_with("libpill_engine-190d6c0e2d2eaf24.rlib"),
            "got: {retargeted_entry}"
        );

        // No sibling: the entry is left exactly as it was.
        let orphan_entry = format!(
            "pill_engine={}",
            directory
                .join("libpill_missing-0000000000000000.rmeta")
                .display()
        );
        assert_eq!(
            redirect_extern(&orphan_entry, &staged_directory),
            orphan_entry
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The redirect reaches `replay_args`, which is where it has to happen.
    #[test]
    fn replay_redirects_a_staged_dependency() {
        let staged_directory = ScratchDirectory::new("staged_replay_test");
        std::fs::write(staged_directory.join("libpill_test.rlib"), b"staged")
            .expect("write staged copy");

        // The sample line records a Windows path, which has no file name on
        // other hosts; give the shared slot a host path so the redirect applies.
        let mut line = parsed_cargo_line();
        for argument in &mut line.args {
            if argument.starts_with("pill_test=") {
                *argument = format!(
                    "pill_test={}",
                    cargo_deps_directory().join("libpill_test.rlib").display()
                );
            }
        }
        let joined = line
            .replay_args(
                Path::new("patch.rs"),
                Path::new("patch.dll"),
                "pill_hotpatch_1",
                &[],
                Some(staged_directory.path()),
            )
            .join(" ");

        assert!(
            joined.contains(&format!(
                "pill_test={}",
                staged_directory.join("libpill_test.rlib").display()
            )),
            "the staged copy must be linked: {joined}"
        );
        // A genuinely hash-qualified dependency is untouched.
        assert!(
            joined.contains("serde=D:\\proj\\target\\debug\\deps\\libserde-1094916a36d6bb5e.rlib")
        );

        let _ = std::fs::remove_dir_all(&staged_directory);
    }

    #[test]
    fn replay_keeps_every_dependency_flag() {
        let line = parsed_cargo_line();
        let arguments = line.replay_args(
            Path::new("patch.rs"),
            Path::new("patch.dll"),
            "pill_hotpatch_1",
            &[],
            None,
        );
        let joined = arguments.join(" ");

        // The dependency graph must survive verbatim - this is what keeps
        // TypeId identical between host and patch.
        assert!(joined.contains("dependency=D:\\proj\\target\\debug\\deps"));
        assert!(
            joined.contains("pill_engine=D:\\proj\\target\\debug\\deps\\libpill_engine-319.rlib")
        );
        assert!(joined.contains("native=C:\\reg\\windows_x86_64_msvc-0.52.6\\lib"));
        assert!(joined.contains("prefer-dynamic"));
        assert!(joined.contains("linker=rust-lld"));
        assert!(joined.contains("--edition=2021"));
        assert!(joined.contains("cfg(docsrs,test)"));
    }

    /// The patch links through the toolchain's LLD whatever cargo recorded.
    ///
    /// The editor launcher forces `link.exe` on the whole dx session because
    /// dx's linker proxy cannot drive LLD, and that flag is captured with the
    /// rest of the command. The patch is not linked by dx - the host runs rustc
    /// itself - so the captured choice does not apply to it.
    #[test]
    fn the_patch_links_with_lld_even_when_cargo_recorded_link_exe() {
        assert_eq!(patch_linker_value("linker=link.exe"), "linker=rust-lld");
        assert_eq!(
            patch_linker_value(r"linker=C:\Program Files\...\link.exe"),
            "linker=rust-lld"
        );
        // Already an LLD: left exactly as cargo recorded it, so a workspace
        // that names a specific LLD build keeps it.
        assert_eq!(patch_linker_value("linker=rust-lld"), "linker=rust-lld");
        assert_eq!(
            patch_linker_value("linker=lld-link.exe"),
            "linker=lld-link.exe"
        );
        // Every other `-C` value passes through untouched.
        assert_eq!(patch_linker_value("prefer-dynamic"), "prefer-dynamic");
        assert_eq!(patch_linker_value("debuginfo=2"), "debuginfo=2");
    }

    /// A captured `link.exe` is rewritten inside a full replay, not only in
    /// isolation - and nothing else about the linker line moves.
    #[test]
    fn replay_rewrites_a_captured_link_exe() {
        let mut line = parsed_cargo_line();
        for argument in &mut line.args {
            if argument == "linker=rust-lld" {
                *argument = "linker=link.exe".to_string();
            }
        }
        let arguments = line.replay_args(
            Path::new("patch.rs"),
            Path::new("patch.dll"),
            "pill_hotpatch_1",
            &[],
            None,
        );
        let joined = arguments.join(" ");
        assert!(joined.contains("linker=rust-lld"), "{joined}");
        assert!(!joined.contains("link.exe"), "{joined}");
    }

    #[test]
    fn replay_drops_the_original_crate_identity_and_output() {
        let line = parsed_cargo_line();
        let arguments = line.replay_args(
            Path::new("patch.rs"),
            Path::new("patch.dll"),
            "pill_hotpatch_1",
            &[],
            None,
        );
        let joined = arguments.join(" ");

        assert!(!joined.contains("metadata=b74d3ef2a041325c"));
        assert!(!joined.contains("--out-dir"));
        assert!(!joined.contains("--emit"));
        assert!(!joined.contains("error-format"));
        assert!(!joined.contains("--json"));
        // Incremental is slower for a single-file patch, so it is dropped.
        assert!(!joined.contains("incremental="));
        // The original crate's source must not survive as a positional argument.
        assert!(!joined.contains("lib.rs"));
    }

    #[test]
    fn replay_installs_the_patch_identity_and_output() {
        let line = parsed_cargo_line();
        let arguments = line.replay_args(
            Path::new("patch.rs"),
            Path::new("out.dll"),
            "pill_hotpatch_7",
            &[],
            None,
        );

        let name_index = arguments.iter().position(|a| a == "--crate-name").unwrap();
        assert_eq!(arguments[name_index + 1], "pill_hotpatch_7");

        let type_index = arguments.iter().position(|a| a == "--crate-type").unwrap();
        assert_eq!(arguments[type_index + 1], "cdylib");

        let out_index = arguments.iter().position(|a| a == "-o").unwrap();
        assert_eq!(arguments[out_index + 1], "out.dll");

        assert_eq!(arguments.last().unwrap(), "patch.rs");
        // Exactly one crate identity survives.
        assert_eq!(arguments.iter().filter(|a| *a == "--crate-name").count(), 1);
        assert_eq!(arguments.iter().filter(|a| *a == "--crate-type").count(), 1);
    }

    #[test]
    fn save_and_load_round_trip() {
        let command = vec!["cargo".to_string(), "build".to_string()];
        let line = parsed_cargo_line();
        let directory = ScratchDirectory::new("cargo_line_test");
        let path = directory.join("line.txt");
        line.save(&path, &command).expect("save");
        let loaded = CargoRustcLine::load_if_fresh(&path, &[], &command).expect("load");
        assert_eq!(line, loaded);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// Two checkouts must not share one flags cache.
    ///
    /// A cache in the system temporary directory would name the same file for
    /// every clone of this repository on the machine. Scoping it to the
    /// workspace root is what makes a replay always describe the dependency
    /// graph the running module was built against.
    #[test]
    fn flags_cache_path_is_workspace_scoped() {
        let first_cache_path = flags_cache_path(Path::new("C:/work/one"), "project");
        let second_cache_path = flags_cache_path(Path::new("C:/work/two"), "project");

        assert_ne!(
            first_cache_path, second_cache_path,
            "two checkouts must not share one flags cache"
        );
        assert!(
            first_cache_path.starts_with(Path::new("C:/work/one")),
            "the cache lives inside its own workspace: {}",
            first_cache_path.display()
        );
        assert_eq!(
            first_cache_path.file_name().and_then(|name| name.to_str()),
            Some("project.flags")
        );
    }

    #[test]
    fn load_rejects_a_stale_cache() {
        let command = vec!["cargo".to_string(), "build".to_string()];
        let directory = ScratchDirectory::new("cargo_line_stale");
        let cache = directory.join("line.txt");
        let manifest = directory.join("Cargo.toml");

        parsed_cargo_line().save(&cache, &command).expect("save");
        // Everything below is written after the cache; dating the cache into
        // the past makes each of them provably newer without pausing between
        // the writes.
        date_file(
            &cache,
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000),
        );
        std::fs::write(&manifest, "[package]").expect("write manifest");

        assert!(CargoRustcLine::load_if_fresh(&cache, &[manifest], &command).is_none());

        // The cargo configuration and the pinned toolchain change what a
        // replay means without moving a manifest: a wrapper, a target
        // directory override or a toolchain switch lives there. Each is
        // written after the cache, so each input must be refused on its own.
        for relative in [".cargo/config.toml", "rust-toolchain.toml"] {
            let input = directory.join(relative);
            if let Some(parent) = input.parent() {
                std::fs::create_dir_all(parent).expect("create config directory");
            }
            std::fs::write(&input, "").expect("write config input");
            assert!(
                CargoRustcLine::load_if_fresh(&cache, &[input], &command).is_none(),
                "a newer {relative} must invalidate the cache"
            );
        }
    }

    /// A touch must never put back bytes it read before a save completed.
    ///
    /// The touch exists to update the crate root's modification time so cargo
    /// rebuilds it. Writing back the bytes it read would undo any save that
    /// completes between the read and the write, and the next reload would
    /// compile source the person has already replaced.
    ///
    /// A save replaces the file in one step while the touch runs, so the crate
    /// root holds either version cleanly. Reading a large file takes long enough
    /// that the save finishes inside the touch; the version on disk must then be
    /// the saved one, never the one the touch read.
    #[test]
    fn a_touch_never_reverts_a_save_that_lands_while_it_runs() {
        let filler =
            "// filler line, long enough that reading the file takes time\n".repeat(160_000);
        let mut reverted_at = None;
        for delay_milliseconds in 1..=12 {
            let directory = ScratchDirectory::new("touch_race");
            let path = directory.join("lib.rs");
            let staged_save_path = directory.join("lib.rs.saved");
            std::fs::write(&path, format!("{filler}// version 0\n")).expect("write the crate root");

            let touching_path = path.clone();
            let touch_thread = std::thread::spawn(move || touch(&touching_path));

            std::thread::sleep(std::time::Duration::from_millis(delay_milliseconds));
            // An editor's save: the new text replaces the file in one step.
            std::fs::write(&staged_save_path, format!("{filler}// version 1\n"))
                .expect("stage the save");
            std::fs::rename(&staged_save_path, &path).expect("replace the crate root");

            touch_thread
                .join()
                .expect("join the touch")
                .expect("touch the crate root");
            let observed_text = std::fs::read_to_string(&path).expect("read the crate root");
            let version = observed_text
                .rsplit("// version ")
                .next()
                .and_then(|tail| tail.trim().parse::<u64>().ok())
                .expect("the crate root carries its version");
            if version == 0 {
                reverted_at = Some(delay_milliseconds);
                break;
            }
        }
        assert_eq!(
            reverted_at, None,
            "the touch rewrote the crate root with bytes it read before a save landed"
        );
    }

    /// A capture whose cargo never returns must give up on its own deadline.
    ///
    /// The capture runs on the frame thread, so a cargo blocked on another
    /// build's package lock would freeze the editor for as long as that build
    /// takes - with no deadline, no cancellation and no way for the person to
    /// see why. The command here never finishes on its own, which is what a
    /// lock wait looks like from the host's side.
    #[test]
    fn a_capture_that_never_returns_is_given_up_on_its_deadline() {
        // A script rather than an inline command: the capture appends `-v` to
        // whatever it is given, which a real cargo accepts and a bare `ping`
        // rejects. A script ignores it.
        let directory = ScratchDirectory::new("capture_deadline");
        #[cfg(windows)]
        let (script, body) = (directory.join("slow.cmd"), "ping -n 30 127.0.0.1 >nul\n");
        #[cfg(not(windows))]
        let (script, body) = (directory.join("slow.sh"), "sleep 30\n");
        std::fs::write(&script, body).expect("write the slow command");
        let script_path = script.display().to_string();
        #[cfg(windows)]
        let command: Vec<String> = vec!["cmd".to_string(), "/c".to_string(), script_path];
        #[cfg(not(windows))]
        let command: Vec<String> = vec!["sh".to_string(), script_path];

        let started_at = pill_core::platform::Instant::now();
        let outcome = CargoRustcLine::try_capture_within(
            Path::new("."),
            "a_locked_package",
            &command,
            Duration::from_millis(300),
        );
        let elapsed = started_at.elapsed();

        let error = outcome.expect_err("a capture that never returns must fail");
        assert!(
            error.contains("did not finish"),
            "the failure must name the deadline it ran out of: {error}"
        );
        assert!(
            elapsed < Duration::from_secs(20),
            "the capture waited {elapsed:?} instead of giving up on its deadline"
        );
    }
}
