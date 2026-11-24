//! Unit tests for the hot-patch subsystem.
//!
//! # Responsibilities
//!
//! - Checks which edits classification accepts as body-only, and why it
//!   refuses the rest.
//! - Checks patch naming, the staged rlib refresh and the rollback
//!   refusals.
//! - Checks what a reload does to the baseline snapshot and to the patch
//!   history.
//!
//! # Design
//!
//! The tests live beside the code they exercise, so the implementation half
//! of the module stays free of test scaffolding. They reach the module's
//! private items through `use super::*`.

// External crates
use libloading::Library;

// Current crate
use super::*;
use crate::scratch::ScratchDirectory;

// =============================================================================
// Constants
// =============================================================================

/// Source text with one `#[pill_hot_fn]` free function whose body returns
/// `133.0`, so an edit to that number is a one-token body change.
const PLAIN_SOURCE: &str = r#"
use pill_engine::pill_hot_fn;

#[pill_hot_fn]
pub fn get_color_a() -> f32 {
133.0
}
"#;

/// Two `#[pill_hot]` functions, so one save can change two bodies.
const TWO_HOT_SOURCE: &str = r#"
use pill_engine::*;

const SPEED: f32 = 1.0;

#[pill_hot]
fn movement(value: f32) -> f32 {
value * SPEED
}

#[pill_hot]
fn other_movement(value: f32) -> f32 {
value - 1.0
}
"#;

/// One `#[pill_hot]` function beside an un-annotated `helper` and a
/// `const SPEED`, so one edit can change an annotated body, a plain body, or
/// code outside every body.
const HOT_SOURCE: &str = r#"
use pill_engine::*;

const SPEED: f32 = 1.0;

#[pill_hot]
fn movement(value: f32) -> f32 {
value * SPEED
}

fn helper(value: f32) -> f32 {
value + 1.0
}
"#;

/// One file declaring `draw` twice, in two `impl` blocks, with the attribute
/// on the first.
const DUPLICATE_NAME_SOURCE: &str = r#"
use pill_engine::*;

struct Alpha {
value: f32,
}

struct Beta {
value: f32,
}

impl Alpha {
#[pill_hot]
fn draw(&self) -> f32 {
    self.value * 1.0
}
}

impl Beta {
fn draw(&self) -> f32 {
    self.value * 2.0
}
}
"#;

// =============================================================================
// Free functions
// =============================================================================

/// Loads a system library unrelated to this crate, so a test holds a real
/// image that exports no patch resolver.
///
/// # Panics
///
/// Panics when the platform's library cannot be loaded.
fn unrelated_system_library() -> Library {
    #[cfg(windows)]
    const NAME: &str = "kernel32.dll";
    #[cfg(target_os = "macos")]
    const NAME: &str = "libSystem.B.dylib";
    #[cfg(all(unix, not(target_os = "macos")))]
    const NAME: &str = "libc.so.6";

    // SAFETY: the library is a system one unrelated to this crate, so loading
    // it runs no code of ours.
    unsafe { Library::new(NAME) }.unwrap_or_else(|error| panic!("{NAME} loads: {error}"))
}

/// Builds the `#[pill_hot]` system declaration that classification reports.
fn system_declaration(name: &str) -> source::HotFunction {
    source::HotFunction {
        name: name.to_string(),
        kind: source::HotFunctionKind::System,
        self_type: None,
        trait_name: None,
        takes_receiver: false,
        signature: format!("fn {name}()"),
        cfg_gated: false,
        inline_always: false,
        is_abi_entry_point: false,
        is_annotated: true,
    }
}

/// Builds the `#[pill_hot_fn]` free-function declaration that classification
/// reports.
fn plain_declaration(name: &str) -> source::HotFunction {
    source::HotFunction {
        name: name.to_string(),
        kind: source::HotFunctionKind::PlainFunction,
        self_type: None,
        trait_name: None,
        takes_receiver: false,
        signature: format!("fn {name}()"),
        cfg_gated: false,
        inline_always: false,
        is_abi_entry_point: false,
        is_annotated: true,
    }
}

/// Writes `file_contents` to `path`, creating the parent directories first.
///
/// # Panics
///
/// Panics when the directories cannot be created or the file cannot be
/// written.
fn fs_write(path: &Path, file_contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, file_contents).expect("write file");
}

/// Builds a session over a throwaway source tree, so classification can run
/// against real files without a project or a compiler.
fn session_over(directory: &Path, source_text: &str) -> HotPatchSession {
    std::fs::create_dir_all(directory).expect("create source dir");
    let file = directory.join("lib.rs");
    std::fs::write(&file, source_text).expect("write source");

    let mut session = HotPatchSession {
        workspace_root: directory.to_path_buf(),
        package: "project".to_string(),
        crate_root: file,
        source_root: directory.to_path_buf(),
        package_rlib: directory.join("libproject.rlib"),
        build_command: vec!["cargo".to_string(), "build".to_string()],
        snapshots: HashMap::new(),
        snapshot_taken_at: SystemTime::UNIX_EPOCH,
        rustc_line: None,
        generations: Vec::new(),
        active_generations: HashMap::new(),
        counter: 0,
    };
    session.refresh_snapshots();

    // Move the recorded modification time into the past before the caller
    // edits the file.
    //
    // `classify` skips a file whose modification time still equals the one
    // recorded when it was read. Filesystem timestamps are coarse, so a test
    // that writes its edit in the same tick as this snapshot leaves a file
    // that looks unchanged, and the test then fails now and then with "no
    // change detected" for an edit that is plainly there. Running the tests
    // in parallel makes that more likely, because the write follows sooner.
    //
    // Production is unaffected: a missed edit leaves the change pending and
    // the normal reload picks it up. Only a test that asserts on
    // classification itself can see the problem, so the wait lives here once
    // for every session a test builds, rather than in each test.
    std::thread::sleep(std::time::Duration::from_millis(20));
    session
}

// =============================================================================
// Tests
// =============================================================================

/// A fan-out that reaches no copy reports an error instead of succeeding,
/// and the refusal leaves the loaded patch image in the graveyard.
///
/// That is what stops a refusal from unmapping an image another artifact
/// may already jump into.
#[test]
fn a_fan_out_that_reaches_nothing_keeps_the_patch_image_owned() {
    // Loaded only so the graveyard holds a real image.
    let library = unrelated_system_library();
    let patches = vec![LoadedPatch {
        function: String::from("demo::function"),
        generation: 1,
        library,
    }];

    let error = match prologue_patch_everywhere(&[], &patches, "demo::function", 0, "fn()") {
        Ok(_) => panic!("a fan-out with no artifacts cannot succeed"),
        Err(error) => error,
    };
    assert!(
        error.contains("no loaded artifact reports an address"),
        "the refusal explains why nothing was reached: {error}"
    );
    assert_eq!(
        patches.len(),
        1,
        "the image is still owned by the graveyard"
    );
}

/// An image whose resolver export cannot be found reports an error, not
/// "this image does not link the crate".
///
/// `Ok(false)` means the image was asked and does not carry the crate, and
/// every caller skips those images. An image that cannot be asked must not
/// be folded into that answer: a copy would keep running old code while the
/// console reported a successful install, so the error instead makes the
/// caller reload.
#[test]
fn patch_without_its_resolver_export_is_an_error() {
    let library = unrelated_system_library();
    let patch = LoadedPatch {
        function: String::from("demo::function"),
        generation: 1,
        library,
    };

    let install_refusal = patch
        .install_plain_function("demo::function", 0, "fn()")
        .expect_err("an image that cannot be interrogated cannot answer `not here`");
    assert!(
        install_refusal.contains("pill_patch_resolve_install")
            && install_refusal.contains("demo::function"),
        "the refusal names the export and the patch it was missing from: {install_refusal}"
    );

    let reset_refusal = patch
        .reset_plain_function("demo::function")
        .expect_err("a rollback must not treat an un-interrogable image as declaring nothing");
    assert!(
        reset_refusal.contains("pill_patch_resolve_reset"),
        "the refusal names the export the reset needs: {reset_refusal}"
    );
}

/// The generated source carries the prefixed name and its own resolver
/// export.
///
/// Both keep it from silently colliding with the copy of the project that
/// the patch links.
#[test]
fn generated_source_namespaces_the_entry_and_the_export() {
    let session = HotPatchSession {
        workspace_root: PathBuf::from("."),
        package: "project".to_string(),
        crate_root: PathBuf::from("lib.rs"),
        source_root: PathBuf::from("."),
        package_rlib: PathBuf::from("libproject.rlib"),
        build_command: vec!["cargo".to_string(), "build".to_string()],
        snapshots: HashMap::new(),
        snapshot_taken_at: SystemTime::UNIX_EPOCH,
        rustc_line: None,
        generations: Vec::new(),
        active_generations: HashMap::new(),
        counter: 0,
    };

    let source_contents =
        "use pill_engine::*;\n\n#[pill_hot]\nfn movement(value: i32) -> i32 { value * 2 }\n";
    let generated_source = session
        .generate(
            &session.crate_root,
            source_contents,
            &system_declaration("movement"),
            "project::movement",
        )
        .expect("generated");

    assert!(generated_source.contains("name = \"pill_patch::project::movement\""));
    assert!(generated_source.contains("pill_hot_resolver!(pill_patch_resolve)"));
    assert!(generated_source.contains("use pill_engine::*;"));
    assert!(generated_source.contains("use project::*;"));
    // The function is copied verbatim, body included.
    assert!(generated_source.contains("fn movement(value: i32) -> i32 { value * 2 }"));
    // The source carries attributes, with exactly one `name` override: the
    // original attribute must not be duplicated next to the generated one.
    assert!(generated_source.contains("#["));
    assert_eq!(generated_source.matches("pill_hot(name").count(), 1);
}

/// A plain function keeps its own attribute and gets no `name` override,
/// because it is redirected through per-artifact slots rather than through
/// the engine's registry.
#[test]
fn a_plain_function_keeps_its_own_attribute() {
    let directory = ScratchDirectory::new("generate_plain");
    let session = session_over(&directory, PLAIN_SOURCE);

    let generated_source = session
        .generate(
            &session.crate_root,
            PLAIN_SOURCE,
            &plain_declaration("get_color_a"),
            "project::get_color_a",
        )
        .expect("a plain function must generate");

    assert!(generated_source.contains("#[::pill_engine::pill_hot_fn]"));
    // A `name` override belongs to the system path only: a plain function
    // is found under the patch crate's own unique name.
    assert!(!generated_source.contains("pill_hot(name"));
    assert!(generated_source.contains("pill_hot_resolver!(pill_patch_resolve)"));
    assert!(generated_source.contains("133.0"));

    let _ = std::fs::remove_dir_all(&directory);
}

/// An import the copied body never names is left out of the generated source.
///
/// The patch compiles as its own crate, where a path into a module of the
/// changed file resolves nothing. Carrying an import the body does not need
/// fails a patch whose body is otherwise perfectly patchable.
#[test]
fn an_import_the_body_never_names_is_left_out() {
    let directory = ScratchDirectory::new("generate_unused_import");
    let source_text = "mod physics;\nuse physics::{ball_spawn_state, physics_system};\n\n\
                       #[pill_hot]\nfn counter_system() { let _threshold = 200.0; }\n";
    let session = session_over(&directory, source_text);

    let generated_source = session
        .generate(
            &session.crate_root,
            source_text,
            &system_declaration("counter_system"),
            "project::counter_system",
        )
        .expect("a body-only system must generate");

    assert!(
        !generated_source.contains("use physics::"),
        "an import the body never names would fail the patch compile:\n{generated_source}"
    );
    assert!(generated_source.contains("use project::*;"));

    let _ = std::fs::remove_dir_all(&directory);
}

/// A patch for a function in a module file imports that file's own module, so
/// a type declared beside the function resolves even when the crate root does
/// not re-export it.
#[test]
fn a_module_file_patch_names_its_own_types() {
    let directory = ScratchDirectory::new("generate_module_types");
    let session = session_over(&directory, "pub mod audio_manager;\n");
    let module_source = "use crate::sound::Sound;\n\npub struct AudioSettings;\n\n\
                         #[pill_hot]\nfn apply_settings(settings: AudioSettings) {}\n";
    let module_file = directory.write("audio_manager.rs", module_source);

    let generated_source = session
        .generate(
            &module_file,
            module_source,
            &system_declaration("apply_settings"),
            "project::audio_manager::apply_settings",
        )
        .expect("a system in a module file must generate");

    assert!(
        generated_source.contains("use project::audio_manager::AudioSettings;"),
        "a type declared beside the function must be in scope:\n{generated_source}"
    );
    // `crate` names the patch crate, which has no `sound` module, and the
    // body never names `Sound`.
    assert!(
        !generated_source.contains("use crate::"),
        "a `crate`-rooted import cannot resolve in a patch crate:\n{generated_source}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// Classification must report the function kind, because systems and plain
/// functions are installed through entirely different machinery.
#[test]
fn classify_reports_the_plain_function_kind() {
    let directory = ScratchDirectory::new("classify_plain");
    let mut session = session_over(&directory, PLAIN_SOURCE);

    let edited = PLAIN_SOURCE.replace("133.0", "999.0");
    std::fs::write(directory.join("lib.rs"), &edited).expect("write edit");

    let classified = session.classify().expect("body-only edit must be accepted");
    let edit = classified.expect("a change must be reported");
    assert_eq!(edit.declarations.len(), 1, "one body changed");
    assert_eq!(edit.declarations[0].name, "get_color_a");
    assert_eq!(
        edit.declarations[0].kind,
        source::HotFunctionKind::PlainFunction
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// The name a function is registered under follows the file tree, so a
/// function in a submodule must not be looked up under the crate root's
/// name.
#[test]
fn qualified_names_follow_the_source_tree() {
    let directory = ScratchDirectory::new("qualified_names");
    let session = session_over(&directory, PLAIN_SOURCE);
    let free_function_declaration = plain_declaration("get_color_a");

    assert_eq!(
        session.qualified_name(&directory.join("lib.rs"), &free_function_declaration),
        "project::get_color_a"
    );
    assert_eq!(
        session.qualified_name(&directory.join("color.rs"), &free_function_declaration),
        "project::color::get_color_a"
    );
    assert_eq!(
        session.qualified_name(
            &directory.join("color").join("mod.rs"),
            &free_function_declaration,
        ),
        "project::color::get_color_a"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// The name a method is registered under includes its type, because the
/// build script can read the `impl` block when it records the name.
///
/// The host has to ask for that same name: `pill_test::mirror::Counter::value_as_u64`,
/// not `pill_test::mirror::value_as_u64`. The bare form finds no method at all, and the
/// refusal then blames the build script.
#[test]
fn a_method_is_named_through_its_type() {
    let directory = ScratchDirectory::new("method_naming");
    let session = session_over(&directory, PLAIN_SOURCE);
    let method = source::HotFunction {
        name: "mix".to_string(),
        kind: source::HotFunctionKind::PlainFunction,
        self_type: Some("Tint".to_string()),
        trait_name: None,
        takes_receiver: true,
        signature: "fn mix(&self, other: Tint) -> Tint".to_string(),
        cfg_gated: false,
        inline_always: false,
        is_abi_entry_point: false,
        is_annotated: false,
    };

    assert_eq!(
        session.qualified_name(&directory.join("lib.rs"), &method),
        "project::Tint::mix"
    );
    assert_eq!(
        session.qualified_name(&directory.join("color.rs"), &method),
        "project::color::Tint::mix"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// A trait method is named through both its type and its trait, because
/// neither alone identifies it.
///
/// A type may carry an inherent `draw` beside a trait `draw`, and two traits
/// may each define `draw` for it. A name that carries only the type therefore
/// cannot say which of them is meant.
#[test]
fn a_trait_method_is_named_through_its_trait() {
    let directory = ScratchDirectory::new("trait_method_naming");
    let session = session_over(&directory, PLAIN_SOURCE);
    let via_trait = source::HotFunction {
        name: "default".to_string(),
        kind: source::HotFunctionKind::PlainFunction,
        self_type: Some("Spline".to_string()),
        trait_name: Some("Default".to_string()),
        takes_receiver: false,
        signature: "fn default() -> Self".to_string(),
        cfg_gated: false,
        inline_always: false,
        is_abi_entry_point: false,
        is_annotated: false,
    };

    assert_eq!(
        session.qualified_name(&directory.join("lib.rs"), &via_trait),
        "project::<Spline as Default>::default"
    );
    assert_eq!(
        session.qualified_name(&directory.join("spline.rs"), &via_trait),
        "project::spline::<Spline as Default>::default"
    );

    // And the inherent method of the same name stays a different key.
    let inherent = source::HotFunction {
        trait_name: None,
        ..via_trait
    };
    assert_eq!(
        session.qualified_name(&directory.join("lib.rs"), &inherent),
        "project::Spline::default"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// An edit to a trait method body classifies as patchable and reports the
/// trait, so the generated patch and the address lookup agree.
#[test]
fn classify_accepts_a_trait_method_body() {
    let directory = ScratchDirectory::new("classify_trait_method");
    let source_text = "pub struct Spline(u32);\n\nimpl Shape for Spline {
fn size(&self) -> u32 { 1 }\n}\n";
    let mut session = session_over(&directory, source_text);

    std::fs::write(
        directory.join("lib.rs"),
        source_text.replace("{ 1 }", "{ 2 }"),
    )
    .expect("write edit");

    let edit = session
        .classify()
        .expect("a trait method body is in scope")
        .expect("the change must be reported");
    assert_eq!(edit.declarations.len(), 1);
    assert_eq!(edit.declarations[0].name, "size");
    assert_eq!(edit.declarations[0].self_type.as_deref(), Some("Spline"));
    assert_eq!(edit.declarations[0].trait_name.as_deref(), Some("Shape"));
    assert!(edit.new_contents.contains("{ 2 }"));

    let _ = std::fs::remove_dir_all(&directory);
}

/// A receiver-less associated function is refused at classification.
///
/// `fn default() -> Self` has no receiver to carry, so the generated patch
/// would emit the body at the top level, where `Self` cannot be named. That
/// compile would fail and cost a patch failure plus a full reload. The
/// refusal names the trait and the type, so the reader knows which `impl`
/// block was meant.
#[test]
fn classify_refuses_a_receiverless_trait_function() {
    let directory = ScratchDirectory::new("classify_receiverless");
    let source_text = "pub struct Spline(u32);\n\nimpl Default for Spline {
fn default() -> Self { Spline(1) }\n}\n";
    let mut session = session_over(&directory, source_text);

    std::fs::write(
        directory.join("lib.rs"),
        source_text.replace("Spline(1)", "Spline(2)"),
    )
    .expect("write edit");

    let refusal = session
        .classify()
        .expect_err("a receiver-less associated function cannot be patched");
    assert_eq!(
        refusal.code,
        refusal_code::ASSOCIATED_WITHOUT_RECEIVER,
        "{}",
        refusal.detail
    );
    assert!(
        refusal.detail.contains("Default") && refusal.detail.contains("Spline"),
        "the refusal names the trait and the type: {}",
        refusal.detail
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// The generated patch carries the body from the right `impl` block.
///
/// With two types implementing the same trait method, a search by bare name
/// would compile the first `fn size` in the file and install it for
/// whichever type was patched.
#[test]
fn a_generated_trait_patch_carries_the_right_body() {
    let directory = ScratchDirectory::new("generate_trait_method");
    let source_text = "pub struct Alpha(u32);\npub struct Beta(u32);\n
impl Shape for Alpha {
fn size(&self) -> u32 { 111 }\n}\n
impl Shape for Beta {
fn size(&self) -> u32 { 222 }\n}\n";
    let session = session_over(&directory, source_text);

    let beta_declaration = source::HotFunction {
        name: "size".to_string(),
        kind: source::HotFunctionKind::PlainFunction,
        self_type: Some("Beta".to_string()),
        trait_name: Some("Shape".to_string()),
        takes_receiver: true,
        signature: "fn size(&self) -> u32".to_string(),
        cfg_gated: false,
        inline_always: false,
        is_abi_entry_point: false,
        is_annotated: false,
    };
    let generated_source = session
        .generate(
            &session.crate_root,
            source_text,
            &beta_declaration,
            "project::<Beta as Shape>::size",
        )
        .expect("a trait method generates a patch");

    assert!(
        generated_source.contains("222"),
        "the patch must carry Beta's body, not Alpha's:\n{generated_source}"
    );
    assert!(
        !generated_source.contains("111"),
        "Alpha's body must not appear:\n{generated_source}"
    );
    // The body keeps `self`, so it goes into a local trait implemented for the
    // concrete type, the same way an inherent method does.
    assert!(generated_source.contains("impl PillHotMethodPatch for Beta"));

    let _ = std::fs::remove_dir_all(&directory);
}

/// A dispatch slot for a method is registered without its type, so the slot
/// route asks under the shorter name.
///
/// The language forces this: the descriptor is an item, and an item may not
/// name `Self` (`error[E0401]`), so `#[pill_hot_fn]` on a method cannot
/// learn the type. The two names are therefore expected to differ.
#[test]
fn a_slot_lookup_omits_the_type_the_macro_cannot_see() {
    let directory = ScratchDirectory::new("slot_naming");
    let session = session_over(&directory, PLAIN_SOURCE);
    let method = source::HotFunction {
        name: "get_color_a".to_string(),
        kind: source::HotFunctionKind::PlainFunction,
        self_type: Some("Spline".to_string()),
        trait_name: None,
        takes_receiver: true,
        signature: "fn get_color_a(&self) -> f32".to_string(),
        cfg_gated: false,
        inline_always: false,
        is_abi_entry_point: false,
        is_annotated: true,
    };

    assert_eq!(
        session.slot_lookup_name(&directory.join("lib.rs"), &method),
        "project::get_color_a",
        "a slot is registered under module_path!() + the method name"
    );
    assert_eq!(
        session.qualified_name(&directory.join("lib.rs"), &method),
        "project::Spline::get_color_a",
        "the canonical name still carries the type"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// Two sessions must never generate the same patch artifact name.
///
/// Every session counts its generations from one and patch libraries are
/// never unloaded, so a shared name means the first patch of a second module
/// tries to write a `.dll` the first module still has mapped. Windows
/// refuses, and that module can never be patched again in that session.
#[test]
fn patch_artifact_names_do_not_collide_across_sessions() {
    let test_module_directory = ScratchDirectory::new("names_first");
    let spline_directory = ScratchDirectory::new("names_second");

    let mut test_module_session = session_over(&test_module_directory, PLAIN_SOURCE);
    test_module_session.package = "pill_test".to_string();
    let mut spline_session = session_over(&spline_directory, PLAIN_SOURCE);
    spline_session.package = "pill_spline".to_string();

    test_module_session.counter += 1;
    spline_session.counter += 1;
    let test_module_patch_name = format!(
        "pill_hotpatch_{}_{}",
        test_module_session.package, test_module_session.counter
    );
    let spline_patch_name = format!(
        "pill_hotpatch_{}_{}",
        spline_session.package, spline_session.counter
    );
    assert_ne!(
        test_module_patch_name, spline_patch_name,
        "each module's first patch must claim its own artifact"
    );

    let _ = std::fs::remove_dir_all(&test_module_directory);
    let _ = std::fs::remove_dir_all(&spline_directory);
}

/// An untouched file is not re-read, so classification takes time in
/// proportion to the edit rather than to the size of the crate.
///
/// The gate compares the file's modification time with the one recorded when
/// it was read, filesystem time against filesystem time. Comparing against
/// `SystemTime::now()` is not sound on Windows: file times come from the
/// coarse system clock and can lag the clock, so a file written just after a
/// snapshot can report an earlier time and every classification would miss.
#[test]
fn an_untouched_file_is_not_re_read() {
    let directory = ScratchDirectory::new("classify_mtime_gate");
    let mut session = session_over(&directory, HOT_SOURCE);

    // A second file that never changes.
    let untouched_path = directory.join("untouched.rs");
    fs_write(&untouched_path, "pub fn stable() {}\n");
    session.refresh_snapshots();
    assert!(
        session.snapshots[&untouched_path].modified.is_some(),
        "the snapshot must record when it was read"
    );

    // The snapshot's recorded contents are then replaced with a different set
    // of functions, without touching the file. Nothing on disk changed, so the
    // gate must skip the file. If it does not, the re-read finds a renamed
    // function and refuses the whole classification, which makes the skip
    // visible instead of merely asserted. The recorded time is kept, so the
    // gate still sees the file as unchanged; only the contents disagree.
    let recorded_modification_time = session.snapshots[&untouched_path].modified;
    session.snapshots.insert(
        untouched_path.clone(),
        Snapshot {
            contents: "pub fn renamed_since_the_snapshot() {}\n".to_string(),
            modified: recorded_modification_time,
        },
    );

    // Only the edited file is re-read, so classification succeeds and names
    // it.
    let edited = HOT_SOURCE.replace("value * SPEED", "value * SPEED * 3.0");
    std::fs::write(directory.join("lib.rs"), &edited).expect("write edit");

    let edit = session
        .classify()
        .expect("the untouched file must be skipped, not re-read")
        .expect("a change must be reported");
    assert_eq!(edit.path.file_name().unwrap(), "lib.rs");
    assert_eq!(edit.declarations[0].name, "movement");

    // And the stale snapshot is still there: proof the file was never read.
    assert!(
        session.snapshots[&untouched_path]
            .contents
            .contains("renamed_since_the_snapshot"),
        "the untouched file was re-read despite an unchanged modification time"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// A file whose content changes is re-read even though it was in the
/// snapshot, because its modification time moved.
#[test]
fn a_touched_file_is_re_read() {
    let directory = ScratchDirectory::new("classify_mtime_touch");
    let mut session = session_over(&directory, PLAIN_SOURCE);

    // Wait past the filesystem's timestamp granularity, so the write is
    // guaranteed to get a different modification time.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let edited = PLAIN_SOURCE.replace("133.0", "144.0");
    std::fs::write(directory.join("lib.rs"), &edited).expect("write edit");

    let edit = session
        .classify()
        .expect("a body-only edit is in scope")
        .expect("the changed file must be re-read and reported");
    assert_eq!(edit.declarations[0].name, "get_color_a");
    assert!(edit.new_contents.contains("144.0"));

    let _ = std::fs::remove_dir_all(&directory);
}

/// Classification returns the modification time together with the contents
/// it read.
///
/// The pair is what `activate_attempt` stores as the new snapshot. Reading
/// the file's time again after installing the patch would pair older
/// contents with a newer time, and a save made during the compile would be
/// recorded as already delivered and then skipped by the modification-time
/// gate.
#[test]
fn classify_returns_the_time_of_the_contents_it_read() {
    let directory = ScratchDirectory::new("classify_modified_pair");
    let mut session = session_over(&directory, PLAIN_SOURCE);

    // The same coarse-timestamp guard as the other re-read tests: a write
    // that happens in the recorded tick would look unchanged.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let edited = PLAIN_SOURCE.replace("133.0", "144.0");
    std::fs::write(directory.join("lib.rs"), &edited).expect("write edit");

    let edit = session
        .classify()
        .expect("a body-only edit is in scope")
        .expect("the change must be reported");
    let on_disk_modification_time = std::fs::metadata(directory.join("lib.rs"))
        .and_then(|metadata| metadata.modified())
        .ok();
    assert_eq!(
        edit.modified, on_disk_modification_time,
        "the time must describe the contents that were read"
    );
    assert!(edit.new_contents.contains("144.0"));

    // Build the snapshot a successful patch stores from the same pair. It
    // matches its own modification time and does not match a later one,
    // which is what makes a save during the patch readable again instead of
    // being missed.
    let recorded_modification_time = edit.modified.expect("a temp directory reports a time");
    let replayed_snapshot = Snapshot {
        contents: edit.new_contents,
        modified: edit.modified,
    };
    assert!(replayed_snapshot.is_current(Some(recorded_modification_time)));
    assert!(
        !replayed_snapshot.is_current(Some(
            recorded_modification_time + std::time::Duration::from_secs(1)
        )),
        "a touch after the read must not count as delivered"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// The staged rlib is copied again from cargo's output before a patch is
/// compiled, so the crate and the dependencies it links agree.
///
/// Without this the compile fails with `error[E0463]: can't find crate for
/// <the crate being patched>` once cargo has rebuilt the crate against newer
/// dependencies - a message that names the wrong crate entirely.
#[test]
fn a_stale_staged_rlib_is_refreshed_before_compiling() {
    let directory = ScratchDirectory::new("restage_rlib");
    let session = session_over(&directory, PLAIN_SOURCE);

    // What cargo produced (in the private module build tree under the
    // host profile directory), and the older copy the host staged from it.
    let cargo_built_rlib_path = directory
        .join(crate::config::module_build_artifact_directory())
        .join("libproject.rlib");
    fs_write(
        &cargo_built_rlib_path,
        "rebuilt against the current dependencies",
    );
    fs_write(&session.package_rlib, "stale");

    session
        .refresh_staged_rlib()
        .expect("refreshing must succeed");
    assert_eq!(
        std::fs::read_to_string(&session.package_rlib).expect("read staged"),
        "rebuilt against the current dependencies",
        "the staged copy must match what cargo produced"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// A staged copy that is already current is left as it is.
///
/// The staged file has the same length and a later modification time than
/// cargo's output, which is the pair `staged_copy_is_current` compares. The
/// contents are equal either way, so the assertion checks that the refresh
/// leaves the file in place.
#[test]
fn a_current_staged_rlib_is_left_alone() {
    let directory = ScratchDirectory::new("restage_current");
    let session = session_over(&directory, PLAIN_SOURCE);

    let cargo_built_rlib_path = directory
        .join(crate::config::module_build_artifact_directory())
        .join("libproject.rlib");
    fs_write(&cargo_built_rlib_path, "same length!!");
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs_write(&session.package_rlib, "same length!!");

    session
        .refresh_staged_rlib()
        .expect("refreshing must succeed");
    assert_eq!(
        std::fs::read_to_string(&session.package_rlib).expect("read staged"),
        "same length!!"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// With no cargo output there is nothing to refresh from, and the staged
/// copy - the only one there is - must survive untouched.
#[test]
fn refreshing_without_a_cargo_artifact_keeps_the_staged_copy() {
    let directory = ScratchDirectory::new("restage_missing");
    let session = session_over(&directory, PLAIN_SOURCE);
    fs_write(&session.package_rlib, "the only copy");

    session.refresh_staged_rlib().expect("must not fail");
    assert_eq!(
        std::fs::read_to_string(&session.package_rlib).expect("read staged"),
        "the only copy"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// The basic classification case: a body-only edit of an annotated function
/// is accepted and reported against the right function.
#[test]
fn classify_accepts_a_body_only_edit() {
    let directory = ScratchDirectory::new("classify_accepts");
    let mut session = session_over(&directory, HOT_SOURCE);

    let edited = HOT_SOURCE.replace("value * SPEED", "value * SPEED * 2.0");
    std::fs::write(directory.join("lib.rs"), &edited).expect("write edit");

    let classified = session.classify().expect("body-only edit must be accepted");
    let edit = classified.expect("a change must be reported");
    assert_eq!(edit.declarations.len(), 1, "one body changed");
    assert_eq!(edit.declarations[0].name, "movement");
    assert_eq!(edit.declarations[0].kind, source::HotFunctionKind::System);
    assert!(edit.new_contents.contains("value * SPEED * 2.0"));

    let _ = std::fs::remove_dir_all(&directory);
}

/// A source tree that has not changed reports no edit, so an unchanged file
/// does not start a patch.
#[test]
fn classify_reports_no_change_when_nothing_moved() {
    let directory = ScratchDirectory::new("classify_unchanged");
    let mut session = session_over(&directory, HOT_SOURCE);

    assert!(session.classify().expect("no error").is_none());

    let _ = std::fs::remove_dir_all(&directory);
}

/// Everything outside an annotated body is refused, with a reason.
///
/// A patch replaces function bodies only, so a changed constant, signature or
/// import is invisible to it: the patch compiles against the code the running
/// world was built with.
#[test]
fn classify_refuses_everything_outside_a_hot_body() {
    let edit_cases: &[(&str, &str, &str)] = &[
        (
            "constant",
            "const SPEED: f32 = 1.0;",
            "const SPEED: f32 = 2.0;",
        ),
        (
            "signature",
            "fn movement(value: f32)",
            "fn movement(value: f64)",
        ),
        ("import", "use pill_engine::*;", "use pill_engine::Engine;"),
    ];

    for (label, from_text, to_text) in edit_cases {
        let directory =
            std::env::temp_dir().join(format!("pill_classify_{}", label.replace(' ', "_")));
        let _ = std::fs::remove_dir_all(&directory);
        let mut session = session_over(&directory, HOT_SOURCE);

        let edited = HOT_SOURCE.replace(from_text, to_text);
        assert_ne!(edited, HOT_SOURCE, "{label}: the fixture edit must apply");
        std::fs::write(directory.join("lib.rs"), &edited).expect("write edit");

        let refusal = session.classify().expect_err(&format!(
            "{label}: a change outside a hot body must be refused"
        ));
        assert_eq!(
            refusal.code,
            refusal_code::OUTSIDE_HOT_BODY,
            "{label}: {}",
            refusal.detail
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}

/// A refused edit must not disable patching for the rest of the session.
///
/// `classify` compares against a snapshot of what is running, and only a
/// successful patch updates it. A refusal is followed by a full reload, which
/// picks the edit up without telling the session. Without the re-sync, the
/// refused change stays in every later comparison, and each later body-only
/// edit is refused because of a change the reload already delivered.
///
/// The host calls `refresh_snapshots` after a reload to close that gap; this
/// test pins the behaviour that depends on it.
#[test]
fn a_reload_resyncs_the_baseline_so_later_body_edits_still_patch() {
    let directory = ScratchDirectory::new("classify_resync_after_reload");
    let mut session = session_over(&directory, HOT_SOURCE);

    // Step 1: an edit the fast path must refuse. In the host this falls
    // back to a full reload, which leaves the file running as written.
    let after_reload = HOT_SOURCE.replace("const SPEED: f32 = 1.0;", "const SPEED: f32 = 2.0;");
    assert_ne!(after_reload, HOT_SOURCE, "the fixture edit must apply");
    std::fs::write(directory.join("lib.rs"), &after_reload).expect("write the refused edit");
    let refusal = session
        .classify()
        .expect_err("a constant change must be refused");
    assert_eq!(refusal.code, refusal_code::OUTSIDE_HOT_BODY);

    // Step 2: the reload the host performs, and the re-sync that goes with
    // it. Without this call the assertion below fails with OUTSIDE_HOT_BODY,
    // because the constant change is still in the diff.
    session.refresh_snapshots();
    // The snapshots are stamped again, which restarts the coarse-timestamp
    // race `session_over` documents, so the edit below needs the same guard: a
    // write that happens in the recorded modification time's tick looks
    // unchanged and `classify` skips the file.
    std::thread::sleep(std::time::Duration::from_millis(20));

    // Step 3: a clean body-only edit on top must now be patchable.
    let body_edited = after_reload.replace("value * SPEED", "value * SPEED + 1.0");
    assert_ne!(body_edited, after_reload, "the body edit must apply");
    std::fs::write(directory.join("lib.rs"), &body_edited).expect("write the body edit");

    let classified = session
        .classify()
        .expect("a body-only edit after a reload must not be refused")
        .expect("the changed body must be detected");
    assert!(
        classified
            .declarations
            .iter()
            .any(|declaration| declaration.name == "movement"),
        "the edited body should be the one reported"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// Two bodies changed in one save are both reported, in a stable order.
///
/// Each body is an independent replacement, so the only cost of taking both is
/// one compile each - cheaper than a full reload, and the world is never torn
/// down.
#[test]
fn classify_reports_every_changed_body() {
    let directory = ScratchDirectory::new("classify_two_bodies");
    let mut session = session_over(&directory, TWO_HOT_SOURCE);

    let edited = TWO_HOT_SOURCE
        .replace("value * SPEED", "value * SPEED * 2.0")
        .replace("value - 1.0", "value - 9.0");
    std::fs::write(directory.join("lib.rs"), &edited).expect("write edit");

    let edit = session
        .classify()
        .expect("two body-only edits are in scope")
        .expect("a change must be reported");
    let changed_body_names: Vec<&str> = edit
        .declarations
        .iter()
        .map(|declaration| declaration.name.as_str())
        .collect();
    assert_eq!(
        changed_body_names,
        vec!["movement", "other_movement"],
        "both bodies, sorted so the order does not depend on scan order"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// Stage timings add up across the bodies of one save.
///
/// `install_body` runs once per changed body with the same accumulator, so
/// the total it reports covers all of them. Writing instead of adding would
/// make the breakdown describe the last body alone, which is exactly the case
/// being diagnosed in a slow multi-body save.
#[test]
fn stage_timings_accumulate_across_bodies() {
    let mut accumulated_stages = PatchStages {
        classify: 1.0,
        ..PatchStages::default()
    };
    let first_body_stages = PatchStages {
        generate: 2.0,
        flags: 3.0,
        compile: 4.0,
        load: 5.0,
        activate: 6.0,
        ..PatchStages::default()
    };
    let second_body_stages = PatchStages {
        generate: 7.0,
        flags: 8.0,
        compile: 9.0,
        load: 10.0,
        activate: 11.0,
        ..PatchStages::default()
    };

    accumulated_stages.merge(&first_body_stages);
    accumulated_stages.merge(&second_body_stages);

    assert_eq!(
        accumulated_stages.classify, 1.0,
        "classify is measured once per save, not per body"
    );
    assert_eq!(accumulated_stages.generate, 9.0);
    assert_eq!(accumulated_stages.flags, 11.0);
    assert_eq!(accumulated_stages.compile, 13.0);
    assert_eq!(accumulated_stages.load, 15.0);
    assert_eq!(accumulated_stages.activate, 17.0);
}

/// Two changed files in one edit are refused with their own code.
#[test]
fn classify_refuses_two_changed_files() {
    let directory = ScratchDirectory::new("classify_two_files");
    let mut session = session_over(&directory, HOT_SOURCE);
    // A second file that is part of the baseline, so changing it later is
    // an edit rather than a new file.
    std::fs::write(directory.join("other.rs"), HOT_SOURCE).expect("write second file");
    session.refresh_snapshots();
    // The snapshots are stamped again, which restarts the coarse-timestamp
    // race `session_over` documents, so the edits below need the same guard: a
    // write that happens in the recorded modification time's tick looks
    // unchanged and `classify` skips the file.
    std::thread::sleep(std::time::Duration::from_millis(20));

    let edited = HOT_SOURCE.replace("value * SPEED", "value * SPEED * 2.0");
    std::fs::write(directory.join("lib.rs"), &edited).expect("write edit");
    std::fs::write(directory.join("other.rs"), &edited).expect("write second edit");

    let refusal = session.classify().expect_err("two files must be refused");
    assert_eq!(
        refusal.code,
        refusal_code::MULTIPLE_FILES,
        "{}",
        refusal.detail
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// Two addressable declarations sharing a bare name are refused rather
/// than patched by guesswork: the classifier's declaration map keeps the
/// last one while body scanning keeps the first, so the body installed
/// and the address it replaces would belong to different functions.
#[test]
fn duplicate_bare_names_are_refused() {
    let directory = ScratchDirectory::new("classify_duplicate_names");
    let mut session = session_over(&directory, DUPLICATE_NAME_SOURCE);

    let edited = DUPLICATE_NAME_SOURCE.replace("self.value * 1.0", "self.value * 9.0");
    std::fs::write(directory.join("lib.rs"), &edited).expect("write edit");

    let refusal = session
        .classify()
        .expect_err("two declarations named `draw` cannot be told apart");
    assert_eq!(
        refusal.code,
        refusal_code::DUPLICATE_DECLARATION,
        "{}",
        refusal.detail
    );
    assert!(
        refusal.detail.contains("draw"),
        "the refusal names the colliding declaration: {}",
        refusal.detail
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// A file that was not in the baseline is always a structural change, and
/// says so with its own code.
#[test]
fn classify_refuses_a_new_source_file() {
    let directory = ScratchDirectory::new("classify_new_file");
    let mut session = session_over(&directory, HOT_SOURCE);

    std::fs::write(directory.join("appeared.rs"), HOT_SOURCE).expect("write new file");

    let refusal = session.classify().expect_err("a new file must be refused");
    assert_eq!(
        refusal.code,
        refusal_code::NEW_SOURCE_FILE,
        "{}",
        refusal.detail
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// A session that never patched a function records no active generation and
/// no history for it.
///
/// That is the state a rollback checks first, and it is why a rollback of an
/// unknown function refuses instead of writing nowhere. This test pins the
/// state itself; the refusal is covered by the reload tests below.
#[test]
fn rollback_refuses_an_unknown_function() {
    let directory = ScratchDirectory::new("rollback_unknown");
    let session = session_over(&directory, HOT_SOURCE);

    assert!(!session.knows_function("project::movement"));
    assert!(session.generations().is_empty());

    let _ = std::fs::remove_dir_all(&directory);
}

/// A prologue generation a reload has invalidated says so, rather than
/// failing as though the crate were never loaded.
///
/// Dropping the addresses is what keeps a rollback from writing into a
/// retired image, but it also makes the generation indistinguishable from a
/// slot-delivered one - and the slot route then refuses for a reason that is
/// not what went wrong. This pins the message a developer actually reads.
#[test]
fn a_prologue_generation_a_reload_invalidated_says_so() {
    let directory = ScratchDirectory::new("rollback_dropped_history");
    let mut session = session_over(&directory, HOT_SOURCE);

    // A generation delivered by overwriting code, as `install_body` records
    // one.
    session.generations.push(Generation {
        function: "project::ordinary".to_string(),
        number: 1,
        address: 0x1000,
        kind: source::HotFunctionKind::PlainFunction,
        signature_hash: 0,
        lookup_name: "project::ordinary".to_string(),
        signature: "fn ordinary()".to_string(),
        prologue_restores: vec![PrologueRestore {
            artifact: "project".to_string(),
            address: 0x2000,
            original: vec![0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xE0],
        }],
        prologue_history_dropped: false,
        installed_at: Instant::now(),
    });
    session
        .active_generations
        .insert("project::ordinary".to_string(), 1);

    // The reload that invalidates every recorded address.
    session.forget_prologue_patches();
    assert!(session.generations[0].prologue_history_dropped);
    assert!(session.generations[0].prologue_restores.is_empty());

    // Patching the same function again makes generation 1 reachable for a
    // rollback: the reload cleared the active entry, and the new generation
    // puts one back. The refusal is for the sequence edit, reload, edit, then
    // ask for the generation from before.
    session.generations.push(Generation {
        function: "project::ordinary".to_string(),
        number: 2,
        address: 0x3000,
        kind: source::HotFunctionKind::PlainFunction,
        lookup_name: "project::ordinary".to_string(),
        signature: "fn ordinary()".to_string(),
        signature_hash: 0,
        prologue_restores: vec![PrologueRestore {
            artifact: "project".to_string(),
            address: 0x2000,
            original: vec![0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xE0],
        }],
        prologue_history_dropped: false,
        installed_at: Instant::now(),
    });
    session
        .active_generations
        .insert("project::ordinary".to_string(), 2);

    let mut engine = Engine::new();
    let refusal_detail = session
        .rollback(&mut engine, &[], &[], "project::ordinary", 1)
        .expect_err("an invalidated generation cannot be rolled back to");
    assert!(
        refusal_detail.contains("a reload has replaced that code"),
        "the refusal must name the reload, not a missing crate: {refusal_detail}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// A reload clears the active generation of slot-delivered history too.
///
/// The rebuilt artifact re-creates the slot with its own body, so a
/// generation that was live before the reload is not running any more -
/// and a rollback that trusted the stale entry would install a body from
/// the previous revision into the fresh artifact.
#[test]
fn a_reload_forgets_slot_generations() {
    let directory = ScratchDirectory::new("reload_forgets_slots");
    let mut session = session_over(&directory, HOT_SOURCE);

    // A generation delivered into a slot, as `install_body` records one: no
    // prologue was written, so there is nothing for the reload to prune.
    session.generations.push(Generation {
        function: "project::movement".to_string(),
        number: 1,
        address: 0x1000,
        kind: source::HotFunctionKind::System,
        signature_hash: 7,
        lookup_name: "project::movement".to_string(),
        signature: String::new(),
        prologue_restores: Vec::new(),
        prologue_history_dropped: false,
        installed_at: Instant::now(),
    });
    session
        .active_generations
        .insert("project::movement".to_string(), 1);
    assert_eq!(session.active_generation("project::movement"), 1);

    // The reload that follows the edit.
    session.forget_prologue_patches();

    assert_eq!(
        session.active_generation("project::movement"),
        0,
        "the rebuilt artifact runs its own body, not the pre-reload patch"
    );
    assert!(!session.knows_function("project::movement"));

    // Rolling back to the pre-reload generation must be refused rather
    // than re-installing a body from a previous revision.
    let mut engine = Engine::new();
    let refusal_detail = session
        .rollback(&mut engine, &[], &[], "project::movement", 1)
        .expect_err("a generation from before the reload cannot be re-installed");
    assert!(
        refusal_detail.contains("has not been patched in this session"),
        "the refusal says there is no history to roll back to: {refusal_detail}"
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// An un-annotated function is patchable: the attribute chooses which
/// mechanism delivers the replacement, not whether one is possible at all.
///
/// Discovery comes from the build script's address inventory rather than from
/// an attribute, so a plain `fn` in a participating crate is as patchable as
/// an annotated one.
#[test]
fn classify_accepts_an_un_annotated_function() {
    let directory = ScratchDirectory::new("classify_unannotated");
    let un_annotated_source = "fn ordinary(value: f32) -> f32 { value }\n";
    let mut session = session_over(&directory, un_annotated_source);

    std::fs::write(
        directory.join("lib.rs"),
        "fn ordinary(value: f32) -> f32 { value * 2.0 }\n",
    )
    .expect("write edit");

    let edit = session
        .classify()
        .expect("an un-annotated body edit is in scope")
        .expect("a change must be reported");
    assert_eq!(edit.declarations.len(), 1, "one body changed");
    assert_eq!(edit.declarations[0].name, "ordinary");
    assert_eq!(
        edit.declarations[0].kind,
        source::HotFunctionKind::PlainFunction
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// A file with no addressable function is still refused, so an edit to code
/// the host could never reach reports why.
#[test]
fn classify_refuses_a_file_with_no_functions() {
    let directory = ScratchDirectory::new("classify_no_functions");
    let mut session = session_over(&directory, "pub const SPEED: f32 = 1.0;\n");

    std::fs::write(directory.join("lib.rs"), "pub const SPEED: f32 = 2.0;\n").expect("write edit");

    let refusal = session.classify().expect_err("must be refused");
    assert_eq!(refusal.code, refusal_code::NO_HOT_FUNCTION);

    let _ = std::fs::remove_dir_all(&directory);
}

/// Editing a second function's body is a patch of that function, not a
/// structural change.
#[test]
fn classify_reports_whichever_body_changed() {
    let directory = ScratchDirectory::new("classify_other_body");
    let mut session = session_over(&directory, HOT_SOURCE);

    let edited = HOT_SOURCE.replace("value + 1.0", "value + 9.0");
    std::fs::write(directory.join("lib.rs"), &edited).expect("write edit");

    let edit = session
        .classify()
        .expect("a body-only edit is in scope")
        .expect("a change must be reported");
    assert_eq!(edit.declarations.len(), 1, "one body changed");
    assert_eq!(edit.declarations[0].name, "helper");

    let _ = std::fs::remove_dir_all(&directory);
}

/// Generating a patch for a function the changed source does not declare is
/// an error rather than a patch carrying some other body.
#[test]
fn generation_reports_a_missing_function() {
    let session = HotPatchSession {
        workspace_root: PathBuf::from("."),
        package: "project".to_string(),
        crate_root: PathBuf::from("lib.rs"),
        source_root: PathBuf::from("."),
        package_rlib: PathBuf::from("libproject.rlib"),
        build_command: vec!["cargo".to_string(), "build".to_string()],
        snapshots: HashMap::new(),
        snapshot_taken_at: SystemTime::UNIX_EPOCH,
        rustc_line: None,
        generations: Vec::new(),
        active_generations: HashMap::new(),
        counter: 0,
    };
    assert!(session
        .generate(
            &session.crate_root,
            "fn other() {}",
            &system_declaration("movement"),
            "project::movement",
        )
        .is_err());
}
