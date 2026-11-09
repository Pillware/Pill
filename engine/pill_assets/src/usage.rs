//! Best-effort static scan of a crate's source for how it *consumes* assets, so
//! the DAG report can overlay "which module uses which asset, and which build
//! artifact that module ends up in" on top of the cooking graph.
//!
//! Every reference is recorded as an [`AssetRef`]: the asset, the source file
//! (module) that references it, and how. Modes:
//!   - [`Usage::Embedded`] — `include_bytes!` / `include_str!` (compile-time), and
//!     `res/config.ini` for crates that invoke `create_game!` (the macro embeds it).
//!   - [`Usage::Loaded`]   — runtime path loads: `ResourceLoader::Path("…")`,
//!     `Mesh::new`/`Sound::new`, `read_asset("…")`, `resources_dir().join("…")`,
//!     and any use of a `concat!(env!("CARGO_MANIFEST_DIR"), "/res/…")` const.
//!   - [`Usage::Fetched`]  — `build.rs` download destinations under `res/`.
//!
//! Which artifacts a module is compiled into comes from the `#[cfg(...)]` on its
//! `mod` declaration chain (from `src/lib.rs`/`src/main.rs`), evaluated for three
//! builds: native release, web (wasm32) release and the editor (debug/hot-reload).
//! A `#[cfg]` directly on the referencing statement or its enclosing `fn` narrows
//! the reference's own scope further.
//!
//! Caveat: this is a *textual* scan — dynamically computed paths (built at runtime
//! from variables) can't be resolved and are simply not reported.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

/// Synthetic parent dir for `OUT_DIR`-generated artifacts (not real on-disk paths).
pub const GENERATED_DIR: &str = "(generated)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    Embedded,
    Loaded,
    Fetched,
}

impl Usage {
    /// The lower-case label shown in the DAG legend/tooltips for this mode.
    pub fn label(self) -> &'static str {
        match self {
            Usage::Embedded => "embedded",
            Usage::Loaded => "loaded",
            Usage::Fetched => "fetched",
        }
    }
}

/// Which builds a piece of code is compiled into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Scope {
    pub native: bool,
    pub web: bool,
    pub editor: bool,
}

impl Scope {
    pub const ALL: Scope = Scope {
        native: true,
        web: true,
        editor: true,
    };
    pub const EDITOR: Scope = Scope {
        native: false,
        web: false,
        editor: true,
    };

    pub fn and(self, other: Scope) -> Scope {
        Scope {
            native: self.native && other.native,
            web: self.web && other.web,
            editor: self.editor && other.editor,
        }
    }

    pub fn is_empty(self) -> bool {
        !(self.native || self.web || self.editor)
    }

    /// Short human label, e.g. "native+web", "editor".
    pub fn label(self) -> String {
        let parts: Vec<&str> = [
            (self.native, "native"),
            (self.web, "web"),
            (self.editor, "editor"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| *name)
        .collect();
        if parts.is_empty() {
            "none".into()
        } else {
            parts.join("+")
        }
    }

    /// Evaluate a `cfg(...)` predicate (the text inside the parens) for each build.
    fn from_cfg(predicate: &str, features: &HashSet<String>) -> Scope {
        let builds = [
            BuildCfg {
                debug: false,
                wasm: false,
            },
            BuildCfg {
                debug: false,
                wasm: true,
            },
            BuildCfg {
                debug: true,
                wasm: false,
            },
        ];
        let [native, web, editor] =
            builds.map(|build| eval_cfg(predicate.trim(), build, features).unwrap_or(true));
        Scope {
            native,
            web,
            editor,
        }
    }
}

/// One reference from a source file to an asset.
#[derive(Clone, Debug)]
pub struct AssetRef {
    /// Normalized asset path.
    pub asset: PathBuf,
    /// The referencing source file (`src/**/*.rs` or `build.rs`).
    pub module: PathBuf,
    pub mode: Usage,
    /// Builds this reference is compiled into: the module's scope narrowed by any
    /// `#[cfg]` on the statement or its enclosing `fn`.
    pub scope: Scope,
}

#[derive(Clone, Debug, Default)]
pub struct UsageScan {
    pub refs: Vec<AssetRef>,
    /// Builds each referencing module is compiled into.
    pub modules: HashMap<PathBuf, Scope>,
}

impl UsageScan {
    /// Normalized paths of every referenced asset.
    pub fn assets(&self) -> HashSet<PathBuf> {
        self.refs
            .iter()
            .map(|asset_ref| asset_ref.asset.clone())
            .collect()
    }
}

/// Scan `crate_directory` (a CARGO_MANIFEST_DIR) for asset references.
/// `features` = the cargo features the crate is built with (for `cfg(feature = "…")`).
/// Missing/unreadable sources yield an empty scan.
pub fn scan_usage(crate_directory: &Path, features: &HashSet<String>) -> UsageScan {
    let mut scan = UsageScan::default();
    let resource_dir = crate_directory.join("res");

    let module_scopes = module_scopes(crate_directory, features);

    // Read each source once; the const pre-pass and the scan both reuse it.
    let source_files: Vec<(PathBuf, String)> = rust_sources(crate_directory)
        .into_iter()
        .filter_map(|file_path| {
            std::fs::read_to_string(&file_path)
                .ok()
                .map(|text| (file_path, text))
        })
        .collect();

    // Pre-pass: `const NAME: &str = concat!(env!("CARGO_MANIFEST_DIR"), "<LIT>")`.
    let mut manifest_consts: HashMap<String, PathBuf> = HashMap::new();
    for (_, text) in &source_files {
        collect_manifest_consts(text, crate_directory, &mut manifest_consts);
    }

    for (file_path, text) in &source_files {
        // Files not reached from lib.rs/main.rs (e.g. bins, tests) count as everywhere.
        let module_scope = module_scopes.get(file_path).copied().unwrap_or(Scope::ALL);
        let file_directory = file_path.parent().unwrap_or(crate_directory);
        let mut found: Vec<(PathBuf, Usage, usize)> = Vec::new();
        scan_source(
            text,
            crate_directory,
            file_directory,
            &resource_dir,
            &manifest_consts,
            &mut found,
        );
        if found.is_empty() {
            continue;
        }
        scan.modules.insert(file_path.clone(), module_scope);
        for (asset, mode, position) in found {
            let item_scope = item_scope(text, position, features);
            scan.refs.push(AssetRef {
                asset,
                module: file_path.clone(),
                mode,
                scope: module_scope.and(item_scope),
            });
        }
    }

    let build_rs = crate_directory.join("build.rs");
    if let Ok(text) = std::fs::read_to_string(&build_rs) {
        let mut found = Vec::new();
        scan_build_rs(&text, crate_directory, &mut found);
        if !found.is_empty() {
            // A build script runs on the dev machine only — tooling, not a shipped artifact.
            scan.modules.insert(build_rs.clone(), Scope::EDITOR);
            for asset in found {
                scan.refs.push(AssetRef {
                    asset,
                    module: build_rs.clone(),
                    mode: Usage::Fetched,
                    scope: Scope::EDITOR,
                });
            }
        }
    }

    // De-duplicate identical (asset, module, mode) refs, widening the scope.
    let mut merged: Vec<AssetRef> = Vec::new();
    for asset_ref in scan.refs {
        match merged.iter_mut().find(|existing| {
            existing.asset == asset_ref.asset
                && existing.module == asset_ref.module
                && existing.mode == asset_ref.mode
        }) {
            Some(existing) => {
                existing.scope = Scope {
                    native: existing.scope.native || asset_ref.scope.native,
                    web: existing.scope.web || asset_ref.scope.web,
                    editor: existing.scope.editor || asset_ref.scope.editor,
                }
            }
            None => merged.push(asset_ref),
        }
    }
    scan.refs = merged;
    scan
}

/// Lexically normalize a path (collapse `.` and `..`) without touching the filesystem,
/// so scanned references match the DAG's `root.join(rel)` node paths.
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Every `.rs` file under `<crate>/src`.
fn rust_sources(crate_directory: &Path) -> Vec<PathBuf> {
    let pattern = crate_directory.join("src/**/*.rs");
    let Some(pattern) = pattern.to_str() else {
        return Vec::new();
    };
    match glob::glob(pattern) {
        Ok(paths) => paths.flatten().collect(),
        Err(_) => Vec::new(),
    }
}

// --- module tree + cfg evaluation ---

#[derive(Clone, Copy)]
struct BuildCfg {
    debug: bool,
    wasm: bool,
}

/// Walk `mod name;` declarations from the crate roots, combining each declaration's
/// `#[cfg(...)]` with its parent's scope.
fn module_scopes(crate_directory: &Path, features: &HashSet<String>) -> HashMap<PathBuf, Scope> {
    let mut scopes: HashMap<PathBuf, Scope> = HashMap::new();
    let mut stack: Vec<(PathBuf, Scope)> = ["src/lib.rs", "src/main.rs"]
        .iter()
        .map(|root| crate_directory.join(root))
        .filter(|root| root.is_file())
        .map(|root| (root, Scope::ALL))
        .collect();
    while let Some((file, scope)) = stack.pop() {
        if scopes.contains_key(&file) {
            continue;
        }
        scopes.insert(file.clone(), scope);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            let Some(name) = mod_decl_name(line) else {
                continue;
            };
            let decl_scope = scope.and(attribute_scope(&lines, index, features));
            if let Some(child) = child_module_path(&file, name) {
                stack.push((child, decl_scope));
            }
        }
    }
    scopes
}

/// `mod name;` / `pub mod name;` / `pub(crate) mod name;` → `name` (not inline `mod x {`).
fn mod_decl_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let rest = trimmed
        .strip_prefix("pub(crate) mod ")
        .or_else(|| trimmed.strip_prefix("pub mod "))
        .or_else(|| trimmed.strip_prefix("mod "))?;
    let name = rest.strip_suffix(';')?.trim();
    (!name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')).then_some(name)
}

fn child_module_path(parent: &Path, name: &str) -> Option<PathBuf> {
    let directory = parent.parent()?;
    let file_name = parent.file_name()?.to_str()?;
    let base = if matches!(file_name, "lib.rs" | "main.rs" | "mod.rs") {
        directory.to_path_buf()
    } else {
        directory.join(parent.file_stem()?)
    };
    [
        base.join(format!("{name}.rs")),
        base.join(name).join("mod.rs"),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// Scope from the `#[cfg(...)]` attribute lines directly above `lines[index]`.
fn attribute_scope(lines: &[&str], index: usize, features: &HashSet<String>) -> Scope {
    let mut scope = Scope::ALL;
    let mut cursor = index;
    while cursor > 0 {
        cursor -= 1;
        let attribute = lines[cursor].trim();
        if !attribute.starts_with("#[") {
            break;
        }
        if let Some(predicate) = attribute
            .strip_prefix("#[cfg(")
            .and_then(|rest| rest.strip_suffix(")]"))
        {
            scope = scope.and(Scope::from_cfg(predicate, features));
        }
    }
    scope
}

/// Scope of the statement at byte `position`: its own attributes plus those of the
/// nearest enclosing `fn` (the closest less-indented `fn` signature above it).
fn item_scope(text: &str, position: usize, features: &HashSet<String>) -> Scope {
    let lines: Vec<&str> = text.lines().collect();
    let line_index = text[..position.min(text.len())].matches('\n').count();
    if line_index >= lines.len() {
        return Scope::ALL;
    }
    // The statement may span lines; its attributes sit above its first line, which is
    // the nearest line above that ends a previous statement/block.
    let mut statement_start = line_index;
    while statement_start > 0 {
        let previous = lines[statement_start - 1].trim_end();
        if previous.ends_with(';')
            || previous.ends_with('{')
            || previous.ends_with('}')
            || previous.ends_with(']')
            || previous.trim().is_empty()
        {
            break;
        }
        statement_start -= 1;
    }
    let mut scope = attribute_scope(&lines, statement_start, features);

    let indent = |line: &str| line.len() - line.trim_start().len();
    let own_indent = indent(lines[line_index]);
    for candidate in (0..line_index).rev() {
        let line = lines[candidate];
        let trimmed = line.trim_start();
        let is_fn = [
            "fn ",
            "pub fn ",
            "pub(crate) fn ",
            "async fn ",
            "pub async fn ",
        ]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix));
        if is_fn && indent(line) < own_indent {
            scope = scope.and(attribute_scope(&lines, candidate, features));
            break;
        }
    }
    scope
}

/// Evaluate a cfg predicate. `None` = unknown construct (callers treat as true).
fn eval_cfg(predicate: &str, build: BuildCfg, features: &HashSet<String>) -> Option<bool> {
    let predicate = predicate.trim();
    for (prefix, combinator) in [("any(", 0u8), ("all(", 1), ("not(", 2)] {
        if let Some(inner) = predicate
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix(')'))
        {
            let parts = split_top_level(inner);
            let values: Vec<bool> = parts
                .iter()
                .map(|part| eval_cfg(part, build, features).unwrap_or(true))
                .collect();
            return Some(match combinator {
                0 => values.iter().any(|value| *value),
                1 => values.iter().all(|value| *value),
                _ => !values.first().copied().unwrap_or(true),
            });
        }
    }
    if predicate == "debug_assertions" {
        return Some(build.debug);
    }
    if predicate == "test" {
        return Some(false);
    }
    let (key, value) = predicate.split_once('=')?;
    let value = value.trim().trim_matches('"');
    match key.trim() {
        "target_arch" => Some((value == "wasm32") == build.wasm),
        "target_family" => Some(if build.wasm {
            value == "wasm"
        } else {
            value == "unix" || value == "windows"
        }),
        "target_os" => Some(!build.wasm && value == std::env::consts::OS),
        "feature" => Some(features.contains(value)),
        _ => None,
    }
}

/// Split on commas not nested in parentheses.
fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (index, character) in text.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(text[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

// --- reference detection ---

/// Collect `const NAME: &str = concat!(env!("CARGO_MANIFEST_DIR"), "<LIT>")` → resolved path.
fn collect_manifest_consts(
    text: &str,
    crate_directory: &Path,
    manifest_consts: &mut HashMap<String, PathBuf>,
) {
    const MANIFEST_MARKER: &str = "CARGO_MANIFEST_DIR\"";
    for line in text.lines() {
        if !line.contains(MANIFEST_MARKER) || !line.contains("const ") {
            continue;
        }
        let Some(const_index) = line.find("const ") else {
            continue;
        };
        let const_name = line[const_index + 6..]
            .split([':', ' ', '='])
            .next()
            .unwrap_or("")
            .trim();
        let Some(marker_index) = line.find(MANIFEST_MARKER) else {
            continue;
        };
        if let Some((literal, _)) = next_str(line, marker_index + MANIFEST_MARKER.len()) {
            let resolved = normalize_path(&crate_directory.join(literal.trim_start_matches('/')));
            if !const_name.is_empty() {
                manifest_consts.insert(const_name.to_string(), resolved);
            }
        }
    }
}

/// Scan one source file for embedded + runtime-loaded asset references.
/// Pushes (asset, mode, byte position of the reference).
fn scan_source(
    text: &str,
    crate_directory: &Path,
    file_directory: &Path,
    resource_dir: &Path,
    manifest_consts: &HashMap<String, PathBuf>,
    found: &mut Vec<(PathBuf, Usage, usize)>,
) {
    // --- Embedded: include_bytes! / include_str! ---
    for marker in ["include_bytes!(", "include_str!("] {
        for position in occurrences(text, marker) {
            let remainder = text[position..].trim_start();
            if remainder.starts_with('"') {
                if let Some((literal, _)) = next_str(text, position) {
                    found.push((
                        normalize_path(&file_directory.join(&literal)),
                        Usage::Embedded,
                        position,
                    ));
                }
            } else if let Some(out_dir_offset) = text[position..]
                .find("OUT_DIR\"")
                .filter(|offset| *offset < 80)
            {
                // include_bytes!(concat!(env!("OUT_DIR"), "/<name>")) — generated artifact.
                let name_start = position + out_dir_offset + "OUT_DIR\"".len();
                if let Some((literal, _)) = next_str(text, name_start) {
                    let name = literal.trim_start_matches('/');
                    found.push((
                        crate_directory.join(GENERATED_DIR).join(name),
                        Usage::Embedded,
                        position,
                    ));
                }
            }
        }
    }

    // --- Embedded: create_game! embeds res/config.ini (pill_core::create_game) ---
    for position in occurrences(text, "create_game!(") {
        found.push((resource_dir.join("config.ini"), Usage::Embedded, position));
    }

    // --- Loaded: ResourceLoader::Path("rel") — texture loader, extension optional ---
    for position in occurrences(text, "ResourceLoader::Path(") {
        if !text[position..].trim_start().starts_with('"') {
            continue; // a variable, not a literal path
        }
        if let Some((literal, _)) = next_str(text, position) {
            found.push((
                resolve_resource(resource_dir, &literal, &["cooked_tex", "png"]),
                Usage::Loaded,
                position,
            ));
        }
    }

    // --- Loaded: Mesh::new(name, "rel") / Sound::new(name, "rel") (2nd literal = path) ---
    for (marker, extensions) in [
        ("Mesh::new(", &["cooked_mesh", "obj"][..]),
        ("Sound::new(", &["wav", "mp3"][..]),
    ] {
        for position in occurrences(text, marker) {
            let statement_end = text[position..]
                .find(';')
                .map(|offset| position + offset)
                .unwrap_or(text.len());
            let statement = &text[position..statement_end];
            if let Some((_name, after_name)) = next_str(statement, 0) {
                if let Some((path, _)) = next_str(statement, after_name) {
                    if !path.contains(' ') && (path.contains('/') || path.contains('.')) {
                        found.push((
                            resolve_resource(resource_dir, &path, extensions),
                            Usage::Loaded,
                            position,
                        ));
                    }
                }
            }
        }
    }

    // --- Loaded: read_asset("rel") / resources_dir().join("rel") ---
    for marker in ["read_asset(", "resources_dir().join("] {
        for position in occurrences(text, marker) {
            if text[position..].trim_start().starts_with('"') {
                if let Some((literal, _)) = next_str(text, position) {
                    found.push((
                        normalize_path(&resource_dir.join(&literal)),
                        Usage::Loaded,
                        position,
                    ));
                }
            }
        }
    }

    // --- Loaded: from_file("lit") / read_to_string("lit") ---
    for marker in ["from_file(", "read_to_string("] {
        for position in occurrences(text, marker) {
            if !text[position..].trim_start().starts_with('"') {
                continue;
            }
            if let Some((literal, _)) = next_str(text, position) {
                if literal.contains('/') {
                    let path = if literal.starts_with('/') {
                        PathBuf::from(&literal)
                    } else {
                        resource_dir.join(&literal)
                    };
                    found.push((normalize_path(&path), Usage::Loaded, position));
                }
            }
        }
    }

    // --- Loaded: a bare string literal naming an existing res/ file, e.g. a name passed
    //     through a helper (`load_level(engine, "hero.ron")`). Relative ("../") and
    //     absolute ("/res/…") literals are covered by the include/const rules above.
    let already: Vec<PathBuf> = found.iter().map(|(path, _, _)| path.clone()).collect();
    let mut cursor = 0;
    while let Some((literal, after)) = next_str(text, cursor) {
        let position = cursor;
        cursor = after;
        if literal.starts_with(['.', '/']) || !has_asset_ext(&literal) {
            continue;
        }
        let path = normalize_path(&resource_dir.join(&literal));
        if path.is_file() && !already.contains(&path) {
            found.push((path, Usage::Loaded, position));
        }
    }

    // --- Loaded: any use of a CARGO_MANIFEST_DIR path const (outside its definition) ---
    for (name, resolved) in manifest_consts {
        for position in occurrences(text, name) {
            let start = position - name.len();
            let before = text[..start].chars().next_back();
            let after = text[position..].chars().next();
            let is_token = !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                && !after.is_some_and(|c| c.is_alphanumeric() || c == '_');
            let line_start = text[..start].rfind('\n').map_or(0, |index| index + 1);
            let line_end = text[start..]
                .find('\n')
                .map_or(text.len(), |offset| start + offset);
            let line = &text[line_start..line_end];
            let is_definition = line.contains("const ") && line.contains("CARGO_MANIFEST_DIR");
            let is_import = line.trim_start().starts_with("use ");
            if is_token && !is_definition && !is_import {
                found.push((resolved.clone(), Usage::Loaded, start));
            }
        }
    }
}

/// A `res/`-relative path; when it has no extension, the first of `extensions` that
/// exists on disk (the engine resolves `models/x` → `models/x.cooked_mesh` etc.).
fn resolve_resource(resource_dir: &Path, literal: &str, extensions: &[&str]) -> PathBuf {
    let base = normalize_path(&resource_dir.join(literal));
    if base.extension().is_some() || extensions.is_empty() {
        return base;
    }
    extensions
        .iter()
        .map(|extension| base.with_extension(extension))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| base.with_extension(extensions[0]))
}

/// Scan a `build.rs` for fetched (downloaded) asset destinations under `res/`.
fn scan_build_rs(text: &str, crate_directory: &Path, found: &mut Vec<PathBuf>) {
    // Fetched: destination literals under res/ with an asset extension (e.g. the
    // `res/models/studio_small_08.hdr` written/declared by a download build script).
    let mut cursor = 0;
    while let Some((literal, after)) = next_str(text, cursor) {
        cursor = after;
        // Literal may carry a prefix (e.g. "cargo:rerun-if-changed=res/…"); slice from `res/`.
        if let Some(res_index) = literal.find("res/") {
            if has_asset_ext(&literal) {
                found.push(normalize_path(&crate_directory.join(&literal[res_index..])));
            }
        }
    }
}

// --- tiny textual helpers (no regex dependency) ---

/// Byte indices in `haystack` immediately *after* each occurrence of `needle`.
fn occurrences(haystack: &str, needle: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut cursor = 0;
    while let Some(found) = haystack[cursor..].find(needle) {
        let position = cursor + found + needle.len();
        positions.push(position);
        cursor = position;
    }
    positions
}

/// The next `"…"` string literal at/after `start`; returns (content, index just past closing quote).
fn next_str(text: &str, start: usize) -> Option<(String, usize)> {
    let open_quote = text[start..].find('"')? + start;
    let content_start = open_quote + 1;
    let close_offset = text[content_start..].find('"')?;
    let close = content_start + close_offset;
    Some((text[content_start..close].to_string(), close + 1))
}

/// Whether `literal` ends with a known source/cooked asset extension.
fn has_asset_ext(literal: &str) -> bool {
    const EXTENSIONS: &[&str] = &[
        ".wav",
        ".ron",
        ".cooked_mesh",
        ".cooked_tex",
        ".wgsl",
        ".hlsl",
        ".hdr",
        ".png",
        ".glb",
        ".obj",
        ".mp3",
        ".f32bin",
    ];
    EXTENSIONS
        .iter()
        .any(|extension| literal.ends_with(extension))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(text: &str, consts: &HashMap<String, PathBuf>) -> Vec<(PathBuf, Usage)> {
        let mut found = Vec::new();
        scan_source(
            text,
            Path::new("/g"),
            Path::new("/g/src"),
            Path::new("/g/res"),
            consts,
            &mut found,
        );
        found
            .into_iter()
            .map(|(path, mode, _)| (path, mode))
            .collect()
    }

    #[test]
    fn embedded_include_resolves_relative_to_file() {
        let found = scan(
            r#"let mesh = include_bytes!("../res/models/x.cooked_mesh");"#,
            &HashMap::new(),
        );
        assert_eq!(
            found,
            vec![(
                PathBuf::from("/g/res/models/x.cooked_mesh"),
                Usage::Embedded
            )]
        );
    }

    #[test]
    fn manifest_const_used_anywhere_is_loaded() {
        let mut consts = HashMap::new();
        collect_manifest_consts(
            r#"const LEVEL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/res/a.ron");"#,
            Path::new("/g"),
            &mut consts,
        );
        assert_eq!(consts.get("LEVEL"), Some(&PathBuf::from("/g/res/a.ron")));
        let found = scan(
            "const LEVEL: &str = concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/res/a.ron\");\nlet level = load_level(engine, LEVEL);",
            &consts,
        );
        assert_eq!(found, vec![(PathBuf::from("/g/res/a.ron"), Usage::Loaded)]);
    }

    #[test]
    fn extensionless_paths_resolve_to_cooked_kind() {
        let found = scan(
            "let m = Mesh::new(\n    \"mech\",\n    PathBuf::from(\"models/mech\"),\n);\nlet t = Texture::new(\"a\", TextureType::Color, ResourceLoader::Path(\"models/a\".into()));",
            &HashMap::new(),
        );
        assert!(found.contains(&(
            PathBuf::from("/g/res/models/mech.cooked_mesh"),
            Usage::Loaded
        )));
        assert!(found.contains(&(PathBuf::from("/g/res/models/a.cooked_tex"), Usage::Loaded)));
    }

    #[test]
    fn read_asset_and_resources_dir_join_are_loaded() {
        let found = scan(
            r#"let a = engine.read_asset("pill.f32bin"); let b = engine.resources_dir().join("pill.ron");"#,
            &HashMap::new(),
        );
        assert!(found.contains(&(PathBuf::from("/g/res/pill.f32bin"), Usage::Loaded)));
        assert!(found.contains(&(PathBuf::from("/g/res/pill.ron"), Usage::Loaded)));
    }

    #[test]
    fn create_game_embeds_config_ini() {
        let found = scan("create_game!(crate::G {}, PillGame);", &HashMap::new());
        assert_eq!(
            found,
            vec![(PathBuf::from("/g/res/config.ini"), Usage::Embedded)]
        );
    }

    #[test]
    fn out_dir_include_is_generated_embedded() {
        let found = scan(
            r#"let raw = include_bytes!(concat!(env!("OUT_DIR"), "/equirect.f32bin"));"#,
            &HashMap::new(),
        );
        assert_eq!(
            found,
            vec![(
                PathBuf::from("/g/(generated)/equirect.f32bin"),
                Usage::Embedded
            )]
        );
    }

    #[test]
    fn fetched_from_build_rs_res_literal() {
        let mut found = Vec::new();
        scan_build_rs(
            r#"println!("cargo:rerun-if-changed=res/models/studio_small_08.hdr");"#,
            Path::new("/g"),
            &mut found,
        );
        assert_eq!(
            found,
            vec![PathBuf::from("/g/res/models/studio_small_08.hdr")]
        );
    }

    #[test]
    fn cfg_predicates_map_to_builds() {
        let features: HashSet<String> = ["ui".to_string()].into_iter().collect();
        let scope = |predicate: &str| Scope::from_cfg(predicate, &features);
        assert_eq!(scope("debug_assertions"), Scope::EDITOR);
        assert_eq!(
            scope("not(target_arch = \"wasm32\")"),
            Scope {
                native: true,
                web: false,
                editor: true
            }
        );
        assert_eq!(
            scope("any(not(debug_assertions), target_arch = \"wasm32\")"),
            Scope {
                native: true,
                web: true,
                editor: false
            }
        );
        assert_eq!(scope("feature = \"ui\""), Scope::ALL);
        assert!(scope("feature = \"headless\"").is_empty());
    }

    #[test]
    fn statement_and_fn_attributes_narrow_scope() {
        let features = HashSet::new();
        let text = "#[cfg(debug_assertions)]\nfn tool() {\n    let x = engine.read_asset(\"a.bin\");\n}\nfn ship() {\n    #[cfg(not(debug_assertions))]\n    let y = engine.read_asset(\"b.bin\");\n}\n";
        let a = text.find("\"a.bin").unwrap();
        let b = text.find("\"b.bin").unwrap();
        assert_eq!(item_scope(text, a, &features), Scope::EDITOR);
        assert_eq!(
            item_scope(text, b, &features),
            Scope {
                native: true,
                web: true,
                editor: false
            }
        );
    }

    #[test]
    fn mod_decls_with_cfg_scope_child_files() {
        let root = std::env::temp_dir().join(format!("pill_usage_mods_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "mod game;\n#[cfg(debug_assertions)]\nmod tool;\n#[cfg(not(target_arch = \"wasm32\"))]\npub(crate) mod export;\n",
        )
        .unwrap();
        for name in ["game", "tool", "export"] {
            std::fs::write(root.join(format!("src/{name}.rs")), "").unwrap();
        }
        let scopes = module_scopes(&root, &HashSet::new());
        assert_eq!(scopes[&root.join("src/game.rs")], Scope::ALL);
        assert_eq!(scopes[&root.join("src/tool.rs")], Scope::EDITOR);
        assert_eq!(
            scopes[&root.join("src/export.rs")],
            Scope {
                native: true,
                web: false,
                editor: true
            }
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bare_literal_naming_existing_res_file_is_loaded() {
        let root = std::env::temp_dir().join(format!("pill_usage_lit_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("res")).unwrap();
        std::fs::write(root.join("res/hero.ron"), "()").unwrap();
        let mut found = Vec::new();
        scan_source(
            r#"let a = load_level(engine, "hero.ron"); let b = "missing.ron";"#,
            &root,
            &root.join("src"),
            &root.join("res"),
            &HashMap::new(),
            &mut found,
        );
        let paths: Vec<PathBuf> = found.into_iter().map(|(path, _, _)| path).collect();
        assert_eq!(paths, vec![root.join("res/hero.ron")]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
