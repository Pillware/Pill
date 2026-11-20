//! The imports a generated patch carries, rooted where the patch resolves
//! them.
//!
//! # Responsibilities
//!
//! - Name the changed file's own public items that the copied body uses, so an
//!   item declared beside the patched function is in scope.
//! - Keep the file's top-level imports that the copied body can name, and
//!   leave out the rest.
//! - Root every kept import at a path the patch crate resolves: `crate`,
//!   `self`, `super` and a bare module name are rewritten to the patched
//!   crate's own paths.
//!
//! # Design
//!
//! A patch compiles as its own crate linked against the patched crate's rlib,
//! so a path in it resolves through `std`, an `--extern` crate, the prelude,
//! or a public item of the patched crate - nothing else. An item declared in
//! the changed file is therefore imported by its full path, but only when the
//! crate root does not already provide the name: `use <package>::*` in the
//! generated source reaches every name the crate root makes public, and an
//! extra path through a private module would fail where the crate-root one
//! resolves. For the same reason an import the patch cannot resolve is left
//! out when the body never names what it binds - an unused import fails the
//! whole compile, and that costs a full reload.

// Standard library
use std::path::Path;

// Current crate
use super::source;

// =============================================================================
// Types
// =============================================================================

/// The module context one generated patch is rooted in.
pub(super) struct PatchScope<'a> {
    /// The patched package, which takes the place of `crate`.
    pub(super) package: &'a str,
    /// The patched crate's source directory, where its modules' files live.
    pub(super) source_root: &'a Path,
    /// The patched crate's root file, whose public names the generated
    /// `use <package>::*` already reaches.
    pub(super) crate_root: &'a Path,
    /// The changed file's module path, the package first.
    pub(super) module_path: &'a [String],
}

impl PatchScope<'_> {
    /// The `use` statements a patch for one body needs: the changed file's own
    /// public items the body names, then the file's imports the body can name.
    ///
    /// A glob is kept whatever the body says, because the names it brings in
    /// cannot be listed here.
    pub(super) fn imports_for(
        &self,
        file_source: &str,
        statements: &[String],
        body: &str,
    ) -> Vec<String> {
        // A file below the crate root imports its own public items by their
        // full path. The crate root needs none of that, because the generated
        // source imports the package already.
        let own_item_names = if self.module_path.len() >= 2 {
            public_item_names(file_source)
        } else {
            Vec::new()
        };
        let mut imports = self.own_items_used_by(&own_item_names, body);
        for statement in statements {
            let Some(path) = use_path(statement) else {
                continue;
            };
            let names = bound_names(path);
            let body_may_use_it =
                imports_a_glob(path) || names.iter().any(|name| names_identifier(body, name));
            if !body_may_use_it {
                continue;
            }
            // A `pub use` re-export is one of the file's public items and one
            // of its top-level `use` statements at once, so it binds a name the
            // item import above already bound. The item import wins and the
            // statement goes: `use path::name;` brings every namespace of that
            // item into scope, so the second binding collides even where one is
            // a module and the other a function of the same name. A glob binds
            // no listable name and so can never collide, which is why it is
            // kept whatever the body says.
            if !imports_a_glob(path) && names.iter().all(|name| own_item_names.contains(name)) {
                continue;
            }
            imports.push(self.rooted(statement, path));
        }
        imports
    }

    /// Imports for the changed file's own public items that the body names and
    /// the crate root does not provide.
    ///
    /// The crate root contributes nothing here: the generated source already
    /// imports the package, so its own public items are in scope.
    fn own_items_used_by(&self, own_item_names: &[String], body: &str) -> Vec<String> {
        if own_item_names.is_empty() {
            return Vec::new();
        }
        // The patch defines this function itself, and its text names it, so
        // importing it too would define the name twice.
        let defined_name = declared_item_name(body.trim_start()).unwrap_or_default();
        // A name the crate root already provides comes in through
        // `use <package>::*`, and that path always resolves. Naming the
        // module's own path instead would fail whenever the module itself is
        // private, which is how most project modules are declared.
        let crate_root_source = std::fs::read_to_string(self.crate_root).unwrap_or_default();
        let crate_root_names = public_item_names(&crate_root_source);
        let module_path = self.module_path.join("::");
        let mut imports: Vec<String> = Vec::new();
        for name in own_item_names {
            if *name == defined_name
                || crate_root_names.contains(name)
                || !names_identifier(body, name)
            {
                continue;
            }
            let statement = format!("use {module_path}::{name};");
            if !imports.contains(&statement) {
                imports.push(statement);
            }
        }
        imports
    }

    /// The statement with its first path segment rewritten so the patch crate
    /// resolves it, or the statement unchanged when it already names a crate
    /// the patch links.
    fn rooted(&self, statement: &str, path: &str) -> String {
        // The module path always starts with the package, which is the root
        // every rewritten path is built from.
        let mut base: Vec<&str> = vec![self.package];
        base.extend(self.module_path.iter().skip(1).map(String::as_str));
        let mut remaining = path;
        let mut is_first_segment = true;
        loop {
            let (root, rest) = split_first_segment(remaining);
            let rewritten = match root {
                "crate" if is_first_segment => {
                    base.truncate(1);
                    true
                }
                "self" if is_first_segment => true,
                // Each `super` steps one module up from the file's own module.
                "super" => {
                    if base.len() > 1 {
                        base.pop();
                    }
                    true
                }
                // A bare name is a module of the file's own module, which is
                // how Rust reads the path. An extern crate is not a module of
                // this crate, so it stays as written.
                module if is_first_segment && self.declares_module(module) => {
                    base.push(module);
                    true
                }
                _ => false,
            };
            if !rewritten {
                break;
            }
            remaining = rest;
            is_first_segment = false;
        }
        if is_first_segment {
            // Nothing was rewritten, so the path names a crate the patch
            // already links.
            return statement.to_string();
        }
        format!("use {}{remaining};", base.join("::"))
    }

    /// Whether the changed file's module declares a child module called
    /// `name`.
    ///
    /// The answer is read from the module's folder, which is the only place
    /// the scanner can see it: `src/<name>.rs` for a module of the crate root,
    /// and `src/<module>/<name>.rs` below it.
    fn declares_module(&self, name: &str) -> bool {
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_alphanumeric() || character == '_')
        {
            return false;
        }
        let mut module_directory = self.source_root.to_path_buf();
        for segment in &self.module_path[1..] {
            module_directory.push(segment);
        }
        module_directory.join(format!("{name}.rs")).is_file()
            || module_directory.join(name).join("mod.rs").is_file()
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The path a `use` statement names, without the keyword and the semicolon.
fn use_path(statement: &str) -> Option<&str> {
    Some(
        statement
            .trim()
            .strip_prefix("use ")?
            .strip_suffix(';')?
            .trim(),
    )
}

/// Whether the path imports a glob, whose names cannot be listed.
fn imports_a_glob(path: &str) -> bool {
    path.contains('*')
}

/// Every name a file makes public at its top level: declared items and
/// `pub use` re-exports.
///
/// Only the file's own top level counts, so a method inside an `impl` block
/// and a name reached through a nested module are both left out.
fn public_item_names(source: &str) -> Vec<String> {
    let code = source::code_mask(source);
    let bytes = source.as_bytes();
    let mut names = Vec::new();
    let mut depth = 0i32;
    for (index, byte) in bytes.iter().enumerate() {
        if !code[index] {
            continue;
        }
        match *byte {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b'p' if depth == 0 && source[index..].starts_with("pub ") => {
                let declaration = &source[index + "pub ".len()..];
                if let Some(re_export) = declaration.strip_prefix("use ") {
                    if let Some(end) = re_export.find(';') {
                        names.extend(bound_names(&re_export[..end]));
                    }
                } else if let Some(name) = declared_item_name(declaration) {
                    names.push(name);
                }
            }
            _ => {}
        }
    }
    names
}

/// The name a `pub <keyword> name` declaration introduces.
fn declared_item_name(declaration: &str) -> Option<String> {
    const DECLARATION_KEYWORDS: [&str; 9] = [
        "struct ", "enum ", "union ", "type ", "trait ", "const ", "static ", "fn ", "mod ",
    ];
    let after_keyword = DECLARATION_KEYWORDS
        .iter()
        .find_map(|keyword| declaration.strip_prefix(keyword))?;
    let name: String = after_keyword
        .chars()
        .take_while(|character| character.is_alphanumeric() || *character == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// The first path segment, and the rest of the path starting at the `::` that
/// follows it.
fn split_first_segment(path: &str) -> (&str, &str) {
    match path.find("::") {
        Some(separator) => (path[..separator].trim(), &path[separator..]),
        None => (path.trim(), ""),
    }
}

/// Every name the path binds, so a caller can ask whether the body uses one.
///
/// An `as _` alias binds nothing and is not listed.
fn bound_names(path: &str) -> Vec<String> {
    let mut names = Vec::new();
    collect_bound_names(path.trim(), &mut names);
    names
}

/// Adds the names one path binds, descending into its brace groups.
fn collect_bound_names(path: &str, names: &mut Vec<String>) {
    let path = path.trim();
    if path.is_empty() || path.ends_with('*') {
        return;
    }
    if let Some(open) = path.find('{') {
        let prefix = path[..open]
            .trim_end_matches("::")
            .trim_end_matches(':')
            .trim();
        let close = closing_brace(path, open).unwrap_or(path.len() - 1);
        for item in split_top_level_items(&path[open + 1..close]) {
            let item = item.trim();
            if item == "self" {
                // `use a::{self, b};` binds `a` itself.
                if let Some(last_segment) = prefix.rsplit("::").next() {
                    names.push(last_segment.to_string());
                }
            } else {
                collect_bound_names(item, names);
            }
        }
        return;
    }
    if let Some((_, alias)) = path.rsplit_once(" as ") {
        let alias = alias.trim();
        if !alias.is_empty() && alias != "_" {
            names.push(alias.to_string());
        }
        return;
    }
    if let Some(last_segment) = path.rsplit("::").next() {
        if !matches!(last_segment, "self" | "super" | "crate") {
            names.push(last_segment.to_string());
        }
    }
}

/// The offset of the `}` that closes the `{` at `open`.
fn closing_brace(path: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (offset, character) in path[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// The comma-separated items of a brace group, ignoring commas inside nested
/// groups.
fn split_top_level_items(group: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut depth = 0i32;
    let mut item_start = 0usize;
    for (offset, character) in group.char_indices() {
        match character {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                items.push(&group[item_start..offset]);
                item_start = offset + character.len_utf8();
            }
            _ => {}
        }
    }
    items.push(&group[item_start..]);
    items
}

/// Whether `body` names `name` as a whole identifier of its code.
///
/// Comments and string literals are not code, so a name written only in one is
/// not a use. The shell mentions its own `tests` module in a comment, and
/// reading that as a use made the patch import a module that is declared in
/// test builds only, which does not compile.
fn names_identifier(body: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let code = source::code_mask(body);
    let mut search_from = 0;
    while let Some(offset) = body[search_from..].find(name) {
        let start = search_from + offset;
        let end = start + name.len();
        let before_is_identifier = body[..start]
            .chars()
            .next_back()
            .is_some_and(is_identifier_character);
        let after_is_identifier = body[end..]
            .chars()
            .next()
            .is_some_and(is_identifier_character);
        if !before_is_identifier
            && !after_is_identifier
            && code[start..end].iter().all(|flag| *flag)
        {
            return true;
        }
        search_from = end;
    }
    false
}

/// Whether `character` can be part of an identifier, so a match next to it is
/// part of a longer name.
fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // Current crate
    use super::*;
    use crate::scratch::ScratchDirectory;

    /// A scope for a module file of a crate whose source directory holds the
    /// given files. The crate root is not on disk, so it contributes no names.
    fn module_scope<'a>(package: &'a str, module_path: &'a [String]) -> PatchScope<'a> {
        PatchScope {
            package,
            source_root: Path::new("unused"),
            crate_root: Path::new("unused"),
            module_path,
        }
    }

    /// A public type declared beside the function is imported by its full
    /// path, because the crate root does not re-export it.
    #[test]
    fn a_module_file_imports_its_own_public_item() {
        let module_path = vec![String::from("pill_audio"), String::from("audio_manager")];
        let scope = module_scope("pill_audio", &module_path);
        let file_source = "pub struct AudioSettings { pub ear_separation: f32 }\n";

        let imports = scope.imports_for(
            file_source,
            &[],
            "fn applier(&mut self, settings: &AudioSettings) {\
             let next = AudioSettings::default(); let _ = (settings, next); }",
        );

        assert_eq!(
            imports,
            vec!["use pill_audio::audio_manager::AudioSettings;"]
        );
    }

    /// The function the patch defines is never imported: its own text names
    /// it, and an import would define the name twice.
    #[test]
    fn the_function_the_patch_defines_is_not_imported() {
        let module_path = vec![String::from("pill_audio"), String::from("audio_manager")];
        let scope = module_scope("pill_audio", &module_path);
        let file_source = "pub fn ambient_system() {}\n";

        let imports = scope.imports_for(
            file_source,
            &[],
            "fn ambient_system() { ambient_system(); }",
        );

        assert!(imports.is_empty(), "{imports:?}");
    }

    /// A `pub use` re-export is a public item of the file and one of its
    /// top-level `use` statements at once, so a body naming it reaches the name
    /// twice. Only one import may be emitted: two explicit imports of one name
    /// do not compile, and a patch that does not compile costs the fast path.
    #[test]
    fn a_re_export_the_body_names_is_imported_once() {
        let module_path = vec![String::from("pill_editor_shell"), String::from("shell")];
        let scope = module_scope("pill_editor_shell", &module_path);
        let file_source = "pub mod claim;\n\npub use claim::claim_shell_context;\n";

        let imports = scope.imports_for(
            file_source,
            &[String::from("use claim::claim_shell_context;")],
            "fn run_frame(context: u8) { claim_shell_context(context); }",
        );

        assert_eq!(
            imports,
            vec!["use pill_editor_shell::shell::claim_shell_context;"]
        );
    }

    /// The case that broke `shell::run_frame`: the file declares a module and
    /// re-exports a function under that same name. `use path::panel_windows;`
    /// brings every namespace of the name into scope, so the re-export
    /// statement binding the function collides with it and has to go.
    #[test]
    fn a_re_export_of_a_module_of_its_own_name_is_imported_once() {
        let module_path = vec![String::from("pill_editor_shell"), String::from("shell")];
        let scope = module_scope("pill_editor_shell", &module_path);
        let file_source =
            "pub mod panel_windows;\n\npub use panel_windows::{panel_windows, PanelFeed};\n";

        let imports = scope.imports_for(
            file_source,
            &[String::from(
                "use panel_windows::{panel_windows, PanelFeed};",
            )],
            "fn run_frame(feed: &mut PanelFeed) { panel_windows(feed); }",
        );

        assert_eq!(
            imports,
            vec![
                "use pill_editor_shell::shell::panel_windows;",
                "use pill_editor_shell::shell::PanelFeed;",
            ]
        );
    }

    /// The crate root needs no import of its own: the generated source imports
    /// the package already.
    #[test]
    fn the_crate_root_adds_no_import_for_its_own_items() {
        let module_path = vec![String::from("project")];
        let scope = module_scope("project", &module_path);
        let file_source = "pub struct FrameCounter { pub count: u64 }\n";

        assert!(scope
            .imports_for(
                file_source,
                &[],
                "fn counter() { let _ = FrameCounter { count: 0 }; }"
            )
            .is_empty());
    }

    /// An item the crate root already provides is left to `use <package>::*`:
    /// naming the module's own path would fail when that module is private.
    #[test]
    fn an_item_the_crate_root_provides_is_left_to_the_package_glob() {
        let directory = ScratchDirectory::new("imports_provided_by_root");
        let crate_root = directory.write("lib.rs", "pub use physics::simulate_ball;\n");
        let module_path = vec![String::from("project"), String::from("physics")];
        let scope = PatchScope {
            package: "project",
            source_root: directory.path(),
            crate_root: &crate_root,
            module_path: &module_path,
        };
        let file_source = "pub fn simulate_ball(state: &mut PhysicsState) {}\n";

        let imports = scope.imports_for(
            file_source,
            &[],
            "fn physics_system() { simulate_ball(&mut state); }",
        );

        assert!(imports.is_empty(), "{imports:?}");
    }

    /// An import the body never names is left out, even though the patch
    /// cannot resolve it.
    #[test]
    fn an_import_the_body_never_names_is_left_out() {
        let module_path = vec![String::from("project")];
        let scope = module_scope("project", &module_path);

        let imports = scope.imports_for(
            "",
            &[String::from(
                "use physics::{ball_spawn_state, physics_system};",
            )],
            "fn counter_system() { let _threshold = 200.0; }",
        );

        assert!(imports.is_empty(), "{imports:?}");
    }

    /// An import the body names is kept, and is rooted where the patch
    /// resolves it.
    #[test]
    fn an_import_the_body_names_is_rooted() {
        let module_path = vec![String::from("pill_audio"), String::from("audio_manager")];
        let scope = module_scope("pill_audio", &module_path);

        let imports = scope.imports_for(
            "",
            &[String::from("use crate::sound::Sound;")],
            "fn play(&self, sound: Sound) {}",
        );

        assert_eq!(imports, vec!["use pill_audio::sound::Sound;"]);
    }

    /// `super` steps up from the file's own module, `crate` becomes the
    /// package, and a bare module name is resolved against the file's module.
    #[test]
    fn every_relative_root_is_rewritten() {
        let directory = ScratchDirectory::new("imports_relative_roots");
        directory.write("audio_manager/effects.rs", "");
        let module_path = vec![String::from("pill_audio"), String::from("audio_manager")];
        let scope = PatchScope {
            package: "pill_audio",
            source_root: directory.path(),
            crate_root: directory.path(),
            module_path: &module_path,
        };

        let imports = scope.imports_for(
            "",
            &[
                String::from("use super::AudioManager;"),
                String::from("use crate::sound::Sound;"),
                String::from("use effects::Reverb;"),
                String::from("use serde::Deserialize;"),
            ],
            "fn apply(&self, manager: AudioManager, sound: Sound, reverb: Reverb, d: Deserialize) {}",
        );

        assert_eq!(imports[0], "use pill_audio::AudioManager;");
        assert_eq!(imports[1], "use pill_audio::sound::Sound;");
        assert_eq!(
            imports[2],
            "use pill_audio::audio_manager::effects::Reverb;"
        );
        // An extern crate is not a module of this crate, so it stays as
        // written.
        assert_eq!(imports[3], "use serde::Deserialize;");
    }

    /// A glob is kept, because the names it brings in cannot be listed.
    #[test]
    fn a_glob_is_kept() {
        let directory = ScratchDirectory::new("imports_glob");
        directory.write("settings.rs", "");
        let module_path = vec![String::from("project")];
        let scope = PatchScope {
            package: "project",
            source_root: directory.path(),
            crate_root: directory.path(),
            module_path: &module_path,
        };

        let imports =
            scope.imports_for("", &[String::from("use settings::*;")], "fn movement() {}");

        assert_eq!(imports, vec!["use project::settings::*;"]);
    }

    /// A brace group binds each name it lists, aliases included, so a body
    /// naming one keeps the whole statement.
    #[test]
    fn a_brace_group_binds_the_names_it_lists() {
        assert_eq!(
            bound_names("{AtomicBool, Ordering as Order, nested::{Alpha, Beta}}"),
            vec![
                String::from("AtomicBool"),
                String::from("Order"),
                String::from("Alpha"),
                String::from("Beta"),
            ]
        );
        assert!(bound_names("*").is_empty());
    }

    /// A name written only in a comment is not a use. The shell mentions its
    /// own `tests` module in a comment, and reading that as a use made the
    /// patch import a module that is declared in test builds only.
    #[test]
    fn a_name_in_a_comment_is_not_a_use() {
        let module_path = vec![String::from("pill_editor_shell"), String::from("shell")];
        let scope = module_scope("pill_editor_shell", &module_path);
        let file_source = "#[cfg(test)]\npub mod tests;\n\npub fn run_frame() {}\n";

        let imports = scope.imports_for(
            file_source,
            &[],
            "fn run_frame() {\n    // The shell's own tests read this line.\n}",
        );

        assert!(imports.is_empty(), "{imports:?}");
    }

    /// A name is matched as a whole identifier, so a longer name containing it
    /// does not keep its import.
    #[test]
    fn a_name_matches_as_a_whole_identifier() {
        assert!(names_identifier(
            "AudioSettings::default()",
            "AudioSettings"
        ));
        assert!(!names_identifier(
            "MyAudioSettings::default()",
            "AudioSettings"
        ));
        assert!(!names_identifier("audio_settings", "AudioSettings"));
    }
}
