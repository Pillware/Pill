//! Per-function hot patching: the fast path beside the whole-module reload.
//!
//! # Responsibilities
//!
//! - Decide whether a source change is a body-only edit of a function the host
//!   can address, and refuse loudly when it is anything else.
//! - Generate, compile and load a small patch library for that one function.
//! - Install the replacement at a frame boundary: the engine's registry, an
//!   artifact's dispatch slot, or the function's prologue.
//!
//! # Design
//!
//! This never replaces the whole-module reload. It tries first, and every
//! change it refuses goes to [`LoadedProject::reload`](crate::project_module).
//!
//! The check is deliberately careful. It removes the bodies of the functions
//! the host can reach from both the old and the new source, then compares the
//! rest. A body-only edit of those functions leaves the two versions equal; a
//! changed signature, type, constant, import or other function keeps them
//! different. Refusing a patch only costs one reload. Accepting one by mistake
//! would install code built for a memory layout the running world no longer
//! has, so the check prefers to refuse.
//!
//! Patch libraries are never unloaded: a dispatch slot may point into one for
//! the rest of the process.

// A development-only facility, refused at compile time in an optimized build.
//
// Hot patching runs `rustc`, writes DLLs into the temp directory and loads
// them into the running process, and every annotated function pays an
// indirection. None of that belongs in a shipped binary, and shipping it by
// accident would fail quietly instead of loudly - so the build stops here.
// Enable it only in a debug profile.
#[cfg(not(debug_assertions))]
compile_error!(
    "hot patching is a development facility and cannot be built with \
     optimizations: it invokes `rustc` at runtime and loads generated libraries \
     into the process. Build without `--release`."
);

// Standard library
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// External crates
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::{debug, info, warn};
use pill_engine::Engine;

// Current crate
/// Patch compilation: the rustc invocation and its artifacts.
pub(crate) mod compile;
/// Patch generations: each function's patch history, which a rollback reads.
pub(crate) mod generations;
/// The changed file's imports, rooted where a generated patch resolves them.
mod imports;
/// Patch routes: where a body is installed and how calls reach it.
pub(crate) mod routes;
/// The source scanner, shared with every crate's build script.
///
/// The implementation lives in `pill_hot_scan` because the host and the build
/// scripts must find exactly the same function start and read exactly the same
/// declaration. If the two scanners disagreed - for example, if one named a
/// method through its type and the other did not - every method they disagreed
/// about would quietly stop being patchable.
///
/// Re-exported under this name so `source::` call sites say they are asking
/// about source text, not about the scanning crate.
pub(crate) mod source {
    pub use pill_hot_scan::*;
}
/// Patch builds: compiling and loading a patch away from the frame thread.
mod worker;

pub(crate) use compile::CargoRustcLine;

// The captured compiler line and its cache, re-exported for the build runner.
// A module build reads the same line from here, so it does not have to run a
// second `cargo build -v` to find it.
pub(crate) use compile::{flags_cache_path, parse_rustc_line};

// A function's patch history and the routes back through it.
pub(crate) use generations::Generation;
pub use generations::PatchGeneration;

pub(crate) use routes::{LoadedPatch, PrologueRestore};

// One background build's pieces, re-exported for the frame loop that starts a
// build and installs its product at a later frame boundary.
pub(crate) use worker::{build_attempt, BodyOutcome, PatchAttempt};

use crate::native_library::NativeLibrary;

// The three install routes and the patch-image plumbing they share, in scope so
// the install code below does not spell `routes::` at every call site.
use routes::{
    install_everywhere, prologue_patch_everywhere, reset_everywhere, resolve_in,
    resolve_patch_address, resolve_plain_in,
};

// =============================================================================
// Constants
// =============================================================================

/// Export a generated patch carries so the host can find its new function.
///
/// Deliberately NOT `pill_hot_resolve`: a patch links the patched crate's rlib
/// to reach its types, and that rlib already exports that symbol. Two
/// `#[no_mangle]` definitions of one name in a single artifact is a link error.
pub(super) const PATCH_RESOLVER_EXPORT: &[u8] = b"pill_patch_resolve";

/// Export that reports where a patched plain function lives.
///
/// Named from `PATCH_RESOLVER_EXPORT` by the same macro that generates it, and
/// distinct from the crate's own `pill_hot_resolve_plain` for the same reason.
pub(super) const PATCH_PLAIN_EXPORT: &[u8] = b"pill_patch_resolve_plain";

/// Prefix that separates a patch's registry entry from the crate's own.
///
/// It is required. A patch links the project's rlib, which already carries that
/// crate's `#[pill_hot]` descriptors, so the patch library contains both the
/// old and the new entry for the same function. Asking for the bare name
/// returns whichever one the linker put first, which can be the old address
/// with a matching signature hash: the patch then installs without error and
/// changes nothing.
const PATCH_NAME_PREFIX: &str = "pill_patch::";

// =============================================================================
// Outcome
// =============================================================================

/// Why a change could not be satisfied by a patch, or why an attempt failed.
///
/// The `code` is a stable kebab-case tag and the `detail` is the sentence a
/// developer reads. Splitting them means a test can assert a specific reason
/// without matching prose that is free to improve, which is what makes the
/// per-reason cases in `devops/tests/` meaningful rather than brittle.
#[derive(Debug, Clone)]
pub(crate) struct PatchRefusal {
    /// Stable tag, logged as the `code` field beside the reason.
    pub code: &'static str,
    /// Human explanation, logged as the `detail` field.
    pub detail: String,
}

impl PatchRefusal {
    /// A refusal with a stable code and a formatted explanation.
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// The change touched something a patch cannot express.
pub(crate) mod refusal_code {
    /// A source file appeared that was not in the baseline snapshot.
    pub const NEW_SOURCE_FILE: &str = "new-source-file";
    /// The changed file declares no function the host can address.
    pub const NO_HOT_FUNCTION: &str = "no-hot-function";
    /// Something outside a hot body moved: a signature, type, constant, import
    /// or another function.
    pub const OUTSIDE_HOT_BODY: &str = "outside-hot-body";
    /// More than one source file changed in the same edit.
    pub const MULTIPLE_FILES: &str = "multiple-files";
    /// The edit happened before this session took its baseline snapshot.
    pub const EDITED_BEFORE_ARMING: &str = "edited-before-arming";
    /// A method was edited whose `impl` block names no simple receiver type.
    pub const UNRESOLVED_RECEIVER: &str = "unresolved-receiver";
    /// An associated function without a receiver was edited: the generated
    /// patch has no `Self` to compile its body against.
    pub const ASSOCIATED_WITHOUT_RECEIVER: &str = "associated-without-receiver";
    /// Two addressable declarations in one file share a bare name.
    pub const DUPLICATE_DECLARATION: &str = "duplicate-declaration";
}

/// The change was in scope, but the attempt to deliver it did not complete.
pub(crate) mod failure_code {
    /// The patch source for the edited function could not be produced.
    pub const GENERATE: &str = "generate";
    /// The scratch directory, the generated source, the staged rlib or the
    /// compiler flags could not be prepared.
    pub const PREPARE: &str = "prepare";
    /// `rustc` rejected the generated patch.
    pub const COMPILE: &str = "compile";
    /// The compiled patch could not be mapped into the process.
    pub const LOAD: &str = "load";
    /// The patch did not report the replacement the host asked for.
    pub const RESOLVE: &str = "resolve";
    /// A running artifact refused the replacement, typically because the
    /// signature no longer matches.
    pub const INSTALL: &str = "install";
}

/// What one attempt at a fast patch did.
#[derive(Debug)]
pub(crate) enum PatchOutcome {
    /// A function's implementation was replaced; no reload is needed.
    Patched {
        /// Qualified names of the patched functions, comma-separated when one
        /// save changed several.
        function: String,
        /// Which generation this became; generation zero is the original code
        /// the running artifact was built with.
        generation: u32,
        /// Wall time from starting the attempt to the replacement being
        /// installed.
        elapsed_milliseconds: f64,
        /// Per-stage breakdown, so the dominant cost is visible rather than
        /// guessed at. Without this a slow patch is just a number.
        stages: PatchStages,
        /// Size of the last body's compiled artifact, for the analytics line.
        artifact_bytes: u64,
        /// Which mechanisms delivered this edit. More than one when a single
        /// save changed both an annotated and an un-annotated body.
        routes: Vec<crate::analytics::PatchRoute>,
        /// How many running copies of the changed functions were reached in
        /// total, so a fan-out is visible rather than assumed.
        copies: usize,
    },
    /// Nothing relevant changed.
    Unchanged,
    /// The change is real but out of scope; the caller should reload normally.
    NotPatchable {
        /// Stable code and the sentence explaining it.
        refusal: PatchRefusal,
    },
    /// A patch was attempted and failed. The previous implementation is intact
    /// and the caller should reload normally.
    Failed {
        /// Which function was being patched.
        function: String,
        /// Which generation is still running, so the console says what the
        /// process is executing rather than only what did not happen.
        active_generation: u32,
        /// Stable code and the first line of the failure.
        failure: PatchRefusal,
    },
}

/// What starting a patch attempt found, and whether a build is owed.
///
/// The frame loop reads them this way: `Unchanged` and `NotPatchable` leave the
/// pending edit for the reload path, `Failed` reports what is still running, and
/// `Ready` hands the build to a worker whose product is installed by
/// [`HotPatchSession::activate_attempt`].
pub(crate) enum BeginOutcome {
    /// Nothing relevant changed.
    Unchanged,
    /// The change is real but out of scope; the caller should reload normally.
    NotPatchable {
        /// Stable code and the sentence explaining it.
        refusal: PatchRefusal,
    },
    /// Preparing the replacement failed; the caller should reload normally.
    Failed {
        /// Which function was being prepared.
        function: String,
        /// Which generation is still running, so the console says what the
        /// process is executing rather than only what did not happen.
        active_generation: u32,
        /// Stable code and the first line of the failure.
        failure: PatchRefusal,
    },
    /// A build is owed: hand it to a worker, then activate its product.
    Ready(worker::PatchAttempt),
}

/// Where one patch spent its time, in milliseconds.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct PatchStages {
    /// Reading the sources and deciding the edit is body-only.
    pub classify: f64,
    /// Producing the patch source.
    pub generate: f64,
    /// Asking cargo for the compiler flags. Near zero once this session has
    /// cached the answer.
    pub flags: f64,
    /// `rustc` compiling and linking the patch library.
    pub compile: f64,
    /// `LoadLibrary` on the produced artifact.
    pub load: f64,
    /// Resolving the new address and installing it into every running copy.
    pub activate: f64,
}

/// Renders the per-stage timings as one line for the console.
impl std::fmt::Display for PatchStages {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "classify {:.0}ms | generate {:.0}ms | flags {:.0}ms | \
             compile {:.0}ms | load {:.0}ms | activate {:.1}ms",
            self.classify, self.generate, self.flags, self.compile, self.load, self.activate
        )
    }
}

impl PatchStages {
    /// Add one body's timings into the running total for the whole save.
    ///
    /// `classify` is measured once per save rather than once per body, so the
    /// per-body structs leave it at zero and adding it changes nothing.
    pub fn merge(&mut self, other_stages: &PatchStages) {
        self.classify += other_stages.classify;
        self.generate += other_stages.generate;
        self.flags += other_stages.flags;
        self.compile += other_stages.compile;
        self.load += other_stages.load;
        self.activate += other_stages.activate;
    }
}

// =============================================================================
// HotPatchSession
// =============================================================================

/// What one successfully installed body produced.
///
/// Named for the install rather than for the whole attempt: one attempt may
/// carry several bodies, and the generation number, artifact size and route
/// below describe this one.
struct ApplyResult {
    /// Which generation this became.
    generation: u32,
    /// Size of the compiled patch on disk.
    artifact_bytes: u64,
    /// Which of the three mechanisms delivered it. Reported rather than
    /// inferred downstream: an annotated and an un-annotated plain function are
    /// the same `HotFunctionKind` but take completely different routes.
    route: crate::analytics::PatchRoute,
    /// How many running copies the install reached. One for the engine
    /// registry, which is process-wide; otherwise one per artifact that links
    /// the crate.
    copies: usize,
}

/// One watched source file as this session last read it.
///
/// Contents and modification time are one value because they are one fact: the
/// time is when *these* contents were read. Kept in the type so no caller can
/// update one half without the other.
struct Snapshot {
    /// The file's contents at the moment it was read.
    contents: String,
    /// The file's modification time at that read, or `None` when the
    /// filesystem does not report one.
    ///
    /// `None` means "read the file again every time". That is the safe choice:
    /// a repeated read is cheap, while a missed edit costs a full reload.
    ///
    /// The check compares this value with the file's current modification time,
    /// not with the clock time. Both are file timestamps, and that matters:
    /// Windows file times come from the coarse system clock and can lag `now()`
    /// by a scheduler tick. A file written just after a snapshot could then
    /// report an earlier time, and the edit would be missed.
    modified: Option<SystemTime>,
}

impl Snapshot {
    /// Read one file into a snapshot, or `None` when it cannot be read.
    ///
    /// The time is taken from the same file in the same moment as the contents,
    /// so the two cannot describe different reads.
    fn read(path: &Path) -> Option<Self> {
        let contents = std::fs::read_to_string(path).ok()?;
        let modified = std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok();
        Some(Self { contents, modified })
    }

    /// Whether the file on disk still matches what this snapshot recorded.
    ///
    /// An unknown time on either side answers `false`, so the caller re-reads.
    fn is_current(&self, modified: Option<SystemTime>) -> bool {
        matches!((self.modified, modified), (Some(recorded), Some(current)) if recorded == current)
    }
}

/// One file's in-scope edit, with every field the later steps need.
///
/// The four fields belong together for the same reason [`Snapshot`] is one
/// value: the modification time says when these contents were read. If a later
/// step looked the file's time up again, it would pair these bytes with a newer
/// timestamp, and a save made during the patch would be recorded as already
/// delivered.
#[derive(Debug)]
struct ClassifiedEdit {
    /// The changed file.
    path: PathBuf,
    /// Its changed bodies, sorted by name.
    declarations: Vec<source::HotFunction>,
    /// The file's contents as read during classification.
    new_contents: String,
    /// The file's modification time when those contents were read, or `None`
    /// when the filesystem would not report one.
    modified: Option<SystemTime>,
}

/// Drives the fast path for one loaded module: the project or one extension.
///
/// A session owns that module's snapshots, compiler line and generation
/// history, keyed to the package, build command and staged rlib it patches.
pub(crate) struct HotPatchSession {
    workspace_root: PathBuf,
    package: String,
    crate_root: PathBuf,
    source_root: PathBuf,
    /// The patched crate's own rlib - the staged copy the host built, so
    /// generated sources can `use <crate>::*` and get the identical types the
    /// running module holds.
    package_rlib: PathBuf,
    /// The host's own build command for this crate, feature flags included.
    ///
    /// Captured flags must come from THIS command: features live here and
    /// nowhere else, and a different feature set means different crate
    /// metadata - so a patch built from a bare `cargo build -p <pkg>` line
    /// disagrees with the running world about every `TypeId`.
    build_command: Vec<String>,
    /// Last seen contents of every watched source file, with the modification
    /// time each was read at.
    snapshots: HashMap<PathBuf, Snapshot>,
    /// When those contents were read.
    ///
    /// Used to tell two very different "nothing changed" cases apart: a reload
    /// this session did not cause, and an edit that happened before the session
    /// armed and was therefore captured as the baseline. The second one looks
    /// exactly like a broken watcher from outside.
    snapshot_taken_at: SystemTime,
    /// Compiler flags, captured from cargo once and reused.
    rustc_line: Option<CargoRustcLine>,
    /// Loaded patches, never unloaded.
    generations: Vec<Generation>,
    /// Which generation each patched function is currently running, so a
    /// failure can report what the process is still executing and a rollback
    /// knows where it started. Absent means generation zero, the original.
    active_generations: HashMap<String, u32>,
    /// Makes each patch's crate name and artifact unique within the process.
    counter: u64,
}

impl HotPatchSession {
    /// Prepare a session for one module and take the baseline snapshot of its
    /// sources.
    ///
    /// Returns `None` when the module has no function the host could address,
    /// so a module without one pays nothing, and when the crate's own rlib was
    /// not built - a patch could not link it.
    pub(crate) fn new(
        workspace_root: &Path,
        package: &str,
        watch_directory: &str,
        staging_subdirectory: &str,
        build_command: &[String],
    ) -> Option<Self> {
        let source_root = workspace_root.join(watch_directory);
        // The crate root is `src/lib.rs` unless the manifest's `[lib] path`
        // moves it, which extensions do so their file name is unique
        // across the dependency graph. Resolved through the same helper the
        // build script uses, because a root mistaken for a module would prefix
        // every patch name with a segment the inventory does not carry.
        let crate_root = source_root
            .parent()
            .map(source::crate_root_file)
            .unwrap_or_else(|| source_root.join("lib.rs"));
        // The staged copy, not cargo's per-crate slot. Both the project and
        // every extension write their `rlib` to an unhashed path that any
        // other build of the same package overwrites, and a patch that linked
        // the wrong one would be compiled against a differently configured
        // engine - giving every type a different `TypeId` than the running
        // world holds. The host stages what it built and links only that.
        let package_rlib = workspace_root
            .join(staging_subdirectory)
            .join(format!("lib{package}.rlib"));

        let mut session = Self {
            workspace_root: workspace_root.to_path_buf(),
            package: package.to_string(),
            crate_root,
            source_root,
            package_rlib,
            build_command: build_command.to_vec(),
            snapshots: HashMap::new(),
            snapshot_taken_at: SystemTime::UNIX_EPOCH,
            rustc_line: None,
            generations: Vec::new(),
            active_generations: HashMap::new(),
            counter: 0,
        };
        session.refresh_snapshots();

        // Every function the host could address, not just the annotated ones.
        // An attribute chooses the mechanism, not whether patching is possible,
        // so a crate with no annotations at all is still armed - and a crate
        // whose build script records no addresses reports a clear failure on
        // its first edit instead of quietly falling back to a full reload.
        let addressable_function_count: usize = session
            .snapshots
            .values()
            .map(|snapshot| source::all_functions(&snapshot.contents).len())
            .sum();
        if addressable_function_count == 0 {
            return None;
        }
        let annotated_function_count: usize = session
            .snapshots
            .values()
            .map(|snapshot| source::hot_function_names(&snapshot.contents).len())
            .sum();

        // Without the crate's own rlib a generated patch cannot name the
        // project's types, and every attempt would fail at compile time. Say so
        // once, at startup, rather than on the first edit.
        if !session.package_rlib.is_file() {
            // A warning: whether the fast path is on is the first thing a
            // developer needs to know, and an INFO line is easy to lose among
            // the startup output.
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = package,
                expected = %session.package_rlib.display(),
                "hot patching is idle: the project rlib a patch links to reach the \
                 crate's types was not built"
            );
            return None;
        }

        info!(
            target: telemetry_target::HOT_RELOAD,
            module = package,
            functions = addressable_function_count,
            hot_functions = annotated_function_count,
            "per-function hot patching armed"
        );
        Some(session)
    }

    /// Re-read every `.rs` file under the source root into the snapshot map.
    ///
    /// The snapshot is the baseline that [`classify`](Self::classify) compares
    /// against, so it must always describe **what is currently running**. Two
    /// things keep it honest: a successful patch records the new contents
    /// itself, and a full reload refreshes it through this method. Without the
    /// second, one edit that could not be patched would leave the baseline
    /// stale forever. Every later edit would then be compared against old
    /// contents, reported as a change outside a function body, and refused,
    /// even though the reload had already applied it.
    pub(crate) fn refresh_snapshots(&mut self) {
        for path in rust_sources(&self.source_root) {
            if let Some(snapshot) = Snapshot::read(&path) {
                self.snapshots.insert(path, snapshot);
            }
        }
        // Stamped after the reads, so a file written during them counts as
        // newer and is reported rather than silently absorbed.
        self.snapshot_taken_at = SystemTime::now();
    }

    /// Prepare a build for a pending source change, without compiling it.
    ///
    /// Everything here runs on the calling thread and touches only this session
    /// and the filesystem: classifying the edit, generating the patch source,
    /// staging the rlib and obtaining the compiler line to replay. Nothing here
    /// touches the engine or a loaded artifact, so the caller can hand the
    /// result to another thread and install it at a later frame boundary.
    ///
    /// The expensive half, `rustc` and `LoadLibrary`, belongs to
    /// [`worker::build_attempt`]. The finished patch is installed by
    /// [`Self::activate_attempt`].
    pub(crate) fn begin_attempt(&mut self) -> BeginOutcome {
        let started_at = Instant::now();
        let mut stages = PatchStages::default();

        // Step 1: Find which functions' bodies changed, if the edit is
        // body-only and nothing else moved.
        let classify_started_at = Instant::now();
        let classified_edit = self.classify();
        stages.classify = classify_started_at.elapsed().as_secs_f64() * 1000.0;

        let edit = match classified_edit {
            Ok(Some(found)) => found,
            Ok(None) => return BeginOutcome::Unchanged,
            Err(refusal) => return BeginOutcome::NotPatchable { refusal },
        };
        let path = edit.path;

        // Step 2: Generate each changed body's source and collect its build
        // inputs. A failure here leaves nothing installed: the running
        // implementation is untouched, so the caller reloads.
        let mut bodies = Vec::with_capacity(edit.declarations.len());
        for declaration in edit.declarations {
            // The name the running artifact recorded for this function, which
            // is the key the engine registry uses. It is built from the file's
            // position under the source root, so a function in a submodule gets
            // the module's path in its name.
            let qualified = self.qualified_name(&path, &declaration);
            // The slot route asks under a different name; see
            // `slot_lookup_name`.
            let slot_name = self.slot_lookup_name(&path, &declaration);
            match self.prepare_body(
                &path,
                &edit.new_contents,
                &declaration,
                &qualified,
                &slot_name,
                &mut stages,
            ) {
                Ok(body) => bodies.push(body),
                // The running implementation is untouched, so the console
                // reports which generation is still executing rather than
                // only what failed.
                Err(failure) => {
                    return BeginOutcome::Failed {
                        active_generation: self.active_generation(&qualified),
                        function: qualified,
                        failure,
                    };
                }
            }
        }

        BeginOutcome::Ready(worker::PatchAttempt {
            path,
            new_contents: edit.new_contents,
            modified: edit.modified,
            bodies,
            stages,
            started: started_at,
        })
    }

    // -------------------------------------------------------------------------
    // Classification
    // -------------------------------------------------------------------------

    /// Identify body-only edits in exactly one source file.
    ///
    /// `Ok(None)` means nothing changed. `Ok(Some(..))` carries the file's
    /// contents and the modification time read together, so the snapshot the
    /// caller writes describes one observation rather than two.
    ///
    /// # Errors
    ///
    /// Returns the reason the change is out of scope, phrased for the console:
    /// a new source file, a file declaring no function the host can address, a
    /// change outside the hot bodies, a second changed file, an unresolvable
    /// receiver type, a receiver-less associated function, a duplicate bare
    /// name, or an edit that happened before the baseline was taken.
    fn classify(&mut self) -> Result<Option<ClassifiedEdit>, PatchRefusal> {
        let mut result: Option<ClassifiedEdit> = None;

        // Step 1: Find the source files that changed since the baseline.
        //
        // One directory walk, reused below: re-reading every file in the crate
        // on every attempt would make classification proportional to crate size
        // rather than to the edit.
        let sources = rust_sources(&self.source_root);
        let mut any_newer_than_snapshot = false;

        for path in &sources {
            let modified = std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok();
            if modified.is_some_and(|modified| modified > self.snapshot_taken_at) {
                any_newer_than_snapshot = true;
            }

            // A file whose modification time still matches the one recorded
            // when it was read cannot differ from the snapshot, so it needs no
            // read. A file with no snapshot at all is new, and falls through.
            if self
                .snapshots
                .get(path)
                .is_some_and(|snapshot| snapshot.is_current(modified))
            {
                continue;
            }

            let Ok(new_contents) = std::fs::read_to_string(path) else {
                continue;
            };
            let path = path.clone();
            let Some(old_snapshot) = self.snapshots.get(&path) else {
                // A new file is a structural change by definition.
                return Err(PatchRefusal::new(
                    refusal_code::NEW_SOURCE_FILE,
                    format!("new source file {}", path.display()),
                ));
            };
            let old_contents = &old_snapshot.contents;
            if *old_contents == new_contents {
                continue;
            }

            // Step 2: Check the edit only changed function bodies.
            //
            // Every function the host could address, annotated or not. An
            // attribute is not the opt-in: a crate whose build script emits the
            // address inventory makes all of its functions patchable, and the
            // attribute only chooses which mechanism delivers the replacement.
            let functions = source::all_functions(&new_contents);
            // Two declarations in one file that share a bare name cannot be
            // told apart below. The name map keeps the last declaration, while
            // `function_bodies` keeps the first, so an edit to the first would
            // install the second's body at the first one's address. The scanner
            // returns both on purpose instead of dropping one, which is why the
            // collision is refused here.
            {
                let mut seen: std::collections::HashSet<&str> =
                    std::collections::HashSet::with_capacity(functions.len());
                if let Some(duplicate) = functions
                    .iter()
                    .map(|function| function.name.as_str())
                    .find(|name| !seen.insert(name))
                {
                    return Err(PatchRefusal::new(
                        refusal_code::DUPLICATE_DECLARATION,
                        format!(
                            "{} declares two functions named `{duplicate}`; rename one of \
                             them or split them across files",
                            file_label(&path)
                        ),
                    ));
                }
            }
            let hot: HashMap<String, source::HotFunction> = functions
                .into_iter()
                .map(|function| (function.name.clone(), function))
                .collect();
            if hot.is_empty() {
                return Err(PatchRefusal::new(
                    refusal_code::NO_HOT_FUNCTION,
                    format!(
                        "{} changed but declares no function the host can address",
                        file_label(&path)
                    ),
                ));
            }
            let hot_names: std::collections::HashSet<String> = hot.keys().cloned().collect();

            // Anything outside those bodies must be byte-identical.
            let old_stripped = source::strip_function_bodies(old_contents, &hot_names);
            let new_stripped = source::strip_function_bodies(&new_contents, &hot_names);
            if old_stripped != new_stripped {
                return Err(PatchRefusal::new(
                    refusal_code::OUTSIDE_HOT_BODY,
                    format!(
                        "{} changed outside a hot function body (signature, type, \
                     constant, import or another function)",
                        file_label(&path)
                    ),
                ));
            }

            // Step 3: Find which bodies changed.
            //
            // Both revisions are scanned once with all body names at hand, not
            // once per function. Asking about each function separately would
            // rebuild the mask of the file's code every time, which costs tens
            // of milliseconds on a large file at every save.
            let old_bodies = source::function_bodies(old_contents, &hot_names);
            let new_bodies = source::function_bodies(&new_contents, &hot_names);
            let mut changed: Vec<String> = Vec::new();
            for name in &hot_names {
                if old_bodies.get(name) != new_bodies.get(name) {
                    changed.push(name.clone());
                }
            }
            if changed.is_empty() {
                continue;
            }
            if result.is_some() {
                return Err(PatchRefusal::new(
                    refusal_code::MULTIPLE_FILES,
                    "more than one source file changed",
                ));
            }

            // Step 4: Check every changed body can be patched.
            //
            // Several bodies in one save are patched in sequence rather than
            // refused. Each is an independent replacement, so the cost is one
            // compile apiece - still cheaper than the full reload it replaces,
            // and the world is never torn down.
            changed.sort();
            let mut declarations = Vec::with_capacity(changed.len());
            for function in changed {
                let declaration = hot[&function].clone();

                // A method needs its receiver type to be patched: the generated
                // replacement is a trait implementation for that concrete type.
                // A generic `impl` block, or one whose header does not name a
                // single type, has no type to give it, so the edit is refused
                // instead of producing a patch that cannot compile.
                if declaration.takes_receiver && declaration.self_type.is_none() {
                    return Err(PatchRefusal::new(
                        refusal_code::UNRESOLVED_RECEIVER,
                        format!(
                            "`{function}` takes a receiver but its `impl` block does \
                             not name a simple type; a generic or trait implementation \
                             cannot be patched"
                        ),
                    ));
                }
                // An associated function without a receiver has no `Self` to
                // carry. The generated patch copies the body to the top level,
                // where `Self` cannot be named, so the compile would fail.
                // Refusing it here costs one cheap `NotPatchable` answer
                // instead of a patch failure followed by a full reload.
                if !declaration.takes_receiver
                    && (declaration.self_type.is_some() || declaration.trait_name.is_some())
                {
                    let owner = match (&declaration.trait_name, &declaration.self_type) {
                        (Some(trait_name), Some(self_type)) => {
                            format!("`{trait_name}` for `{self_type}`")
                        }
                        (Some(trait_name), None) => format!("the `{trait_name}` trait"),
                        (None, Some(self_type)) => format!("`{self_type}`"),
                        (None, None) => "its `impl` block".to_string(),
                    };
                    return Err(PatchRefusal::new(
                        refusal_code::ASSOCIATED_WITHOUT_RECEIVER,
                        format!(
                            "`{function}` is an associated function without a receiver in \
                             {owner}; a patch has no `Self` to compile its body against"
                        ),
                    ));
                }
                declarations.push(declaration);
            }
            result = Some(ClassifiedEdit {
                path,
                declarations,
                new_contents,
                modified,
            });
        }

        // Nothing changed, but a watched file may be newer than the baseline.
        // That happens when a file is saved while the host is still starting
        // up: the edit was already in the file when the snapshot was taken, so
        // the fast path has nothing to compare against. The change is real, so
        // it is reported as a refusal and the full reload that follows picks
        // it up.
        if result.is_none() && any_newer_than_snapshot {
            return Err(PatchRefusal::new(
                refusal_code::EDITED_BEFORE_ARMING,
                format!(
                    "a source file changed before `{}` armed, so the edit was \
                     captured as the baseline; this reload will pick it up",
                    self.package
                ),
            ));
        }

        Ok(result)
    }

    // -------------------------------------------------------------------------
    // Generation, compilation, activation
    // -------------------------------------------------------------------------

    /// Generate one body's patch source and collect everything its build needs,
    /// up to but not including running `rustc`.
    ///
    /// `path` is the changed file: its module path decides where the patch
    /// roots the imports it carries. `slot_name` is carried, not read here:
    /// activation files the slot route under it. `save_stages` receives this
    /// body's timings once preparation succeeds, so a failed body's partial
    /// cost stays out of the report.
    ///
    /// # Errors
    ///
    /// Returns the refusal code and message for the first step that cannot
    /// complete: generating the patch source, creating or writing in the
    /// scratch directory, refreshing the staged rlib, or capturing the
    /// compiler flags.
    fn prepare_body(
        &mut self,
        path: &Path,
        new_contents: &str,
        declaration: &source::HotFunction,
        qualified_name: &str,
        slot_name: &str,
        save_stages: &mut PatchStages,
    ) -> Result<worker::PreparedBody, PatchRefusal> {
        let mut stages = PatchStages::default();
        self.counter += 1;
        // The package name is part of the patch crate's name because every
        // session counts from one and patch libraries are never unloaded.
        // Without it, the first patch of a second module would try to write a
        // `.dll` the first module still has mapped. Windows refuses that
        // write, so one module's patch would stop every other module from
        // being patched for the rest of the session.
        let crate_name = format!("pill_hotpatch_{}_{}", self.package, self.counter);

        // Step 1: Generate the patch source.
        let generate_started = Instant::now();
        let generated = self
            .generate(path, new_contents, declaration, qualified_name)
            .map_err(|detail| PatchRefusal::new(failure_code::GENERATE, detail))?;
        // Step 2: Write the generated source into the scratch directory.
        // The directory is per process rather than the system temporary one,
        // because it already has a cleanup path: a later run removes the
        // directories of processes that have exited. Patch images are never
        // unloaded, so without that cleanup they would accumulate on disk for
        // every edit of every session.
        let scratch_directory =
            crate::native_library::process_temporary_directory(&self.workspace_root);
        if let Err(error) = std::fs::create_dir_all(&scratch_directory) {
            return Err(PatchRefusal::new(
                failure_code::PREPARE,
                format!("cannot create the patch scratch directory: {error}"),
            ));
        }
        let source_path = scratch_directory.join(format!("{crate_name}.rs"));
        std::fs::write(&source_path, generated.as_bytes()).map_err(|error| {
            PatchRefusal::new(
                failure_code::PREPARE,
                format!("cannot write the generated patch: {error}"),
            )
        })?;
        stages.generate = generate_started.elapsed().as_secs_f64() * 1000.0;

        // Step 3: Refresh the staged rlib so it matches the dependency rlibs
        // the replayed line names, and the two halves of the link closure agree.
        self.refresh_staged_rlib()
            .map_err(|detail| PatchRefusal::new(failure_code::PREPARE, detail))?;
        let package = self.package.clone();

        let artifact_path = scratch_directory.join(format!("{crate_name}.dll"));
        let extra_externs = vec![format!("{}={}", self.package, self.package_rlib.display())];
        // Step 4: Collect the dependency rlibs and capture the compiler flags.
        // The staged set dates from the host's last build of this package,
        // which is the only set guaranteed to match the module rlib being
        // linked. The shared `deps` slots they were copied from are
        // overwritten by any build of the same crate name, including one with
        // different features.
        let staged_dependencies = self
            .workspace_root
            .join(crate::build_runner::STAGED_DEPENDENCY_SUBDIRECTORY);
        let flags_started = Instant::now();
        // The line is cloned rather than borrowed because the worker thread
        // that replays it cannot borrow the session.
        let compiler_line = self
            .rustc_line()
            .map_err(|detail| PatchRefusal::new(failure_code::PREPARE, detail))?
            .clone();
        stages.flags = flags_started.elapsed().as_secs_f64() * 1000.0;
        save_stages.merge(&stages);

        Ok(worker::PreparedBody {
            declaration: declaration.clone(),
            qualified: qualified_name.to_string(),
            slot_name: slot_name.to_string(),
            crate_name,
            source_path,
            artifact_path,
            extra_externs,
            staged_dependencies,
            line: compiler_line,
            package,
        })
    }

    /// Install a finished build at the frame boundary.
    ///
    /// The compile and the load happened before this call, usually on a worker
    /// thread. `outcomes` is the build's report, one entry per body it
    /// attempted, in order; the list stops at the first failure. Everything
    /// that touches the engine, a loaded artifact or a dispatch slot happens
    /// here, on the thread that owns the frame boundary: the prologue route
    /// rewrites live code, so this is a requirement rather than a preference.
    pub(crate) fn activate_attempt(
        &mut self,
        engine: &mut Engine,
        targets: &[(&str, &NativeLibrary)],
        patches: &mut Vec<LoadedPatch>,
        attempt: worker::PatchAttempt,
        outcomes: Vec<worker::BodyOutcome>,
    ) -> PatchOutcome {
        let worker::PatchAttempt {
            path,
            new_contents,
            modified,
            bodies,
            mut stages,
            started,
        } = attempt;
        let mut last_installation: Option<ApplyResult> = None;
        let mut patched_names: Vec<String> = Vec::new();
        // Several bodies in one file are patched in sequence and can take
        // different routes: an annotated and an un-annotated function in the
        // same save use different ones. Every route is reported, so the
        // analytics line describes all the mechanisms that ran.
        let mut patch_routes: Vec<crate::analytics::PatchRoute> = Vec::new();
        let mut copy_count = 0usize;
        for (body, outcome) in bodies.into_iter().zip(outcomes) {
            let installation = match outcome {
                worker::BodyOutcome::Built(built) => {
                    match self.install_body(engine, targets, patches, &body, built, &mut stages) {
                        Ok(installation) => installation,
                        // The running implementation is unchanged, so the
                        // console can name the generation that still executes
                        // instead of reporting only the failure.
                        Err(failure) => {
                            return PatchOutcome::Failed {
                                active_generation: self.active_generation(&body.qualified),
                                function: body.qualified,
                                failure,
                            };
                        }
                    }
                }
                worker::BodyOutcome::Failed(failure) => {
                    return PatchOutcome::Failed {
                        active_generation: self.active_generation(&body.qualified),
                        function: body.qualified,
                        failure,
                    };
                }
            };
            patched_names.push(body.qualified);
            if !patch_routes.contains(&installation.route) {
                patch_routes.push(installation.route);
            }
            copy_count += installation.copies;
            last_installation = Some(installation);
        }

        let Some(installation) = last_installation else {
            return PatchOutcome::Unchanged;
        };

        // Record the new contents only after every body is live, so a partial
        // failure is retried on the next change instead of being treated as
        // done. The time recorded is the one read with these contents, never a
        // fresh stat: a save that happened during the compile must leave the
        // snapshot stale, or the next attempt would skip it as already
        // delivered.
        self.snapshots.insert(
            path,
            Snapshot {
                contents: new_contents,
                modified,
            },
        );
        PatchOutcome::Patched {
            function: patched_names.join(", "),
            generation: installation.generation,
            elapsed_milliseconds: started.elapsed().as_secs_f64() * 1000.0,
            stages,
            artifact_bytes: installation.artifact_bytes,
            routes: patch_routes,
            copies: copy_count,
        }
    }

    /// Install one built patch: keep its image loaded, resolve the replacement
    /// and write it into the running copies.
    ///
    /// The compile, the load and the artifact file read happened before this
    /// call, usually on a worker thread; what runs here is exactly the part
    /// that must happen at the frame boundary.
    ///
    /// # Errors
    ///
    /// Returns `resolve` when the patch does not report the replacement this
    /// route asked for, and `install` when a running copy refuses it, usually
    /// because the function's signature no longer matches.
    fn install_body(
        &mut self,
        engine: &mut Engine,
        targets: &[(&str, &NativeLibrary)],
        patches: &mut Vec<LoadedPatch>,
        body: &worker::PreparedBody,
        built_body: worker::BuiltBody,
        save_stages: &mut PatchStages,
    ) -> Result<ApplyResult, PatchRefusal> {
        let declaration = &body.declaration;
        let function = declaration.name.as_str();
        let function_kind = declaration.kind;
        let qualified = body.qualified.as_str();
        let slot_name = body.slot_name.as_str();
        let crate_name = body.crate_name.as_str();
        let mut stages = PatchStages::default();
        let activate_started = Instant::now();

        // One of the route arms below fills this in. It is saved on the
        // generation, so a rollback can reinstall the same address without
        // recompiling anything.
        let replacement_address;
        let mut signature_hash = 0u64;
        let mut signature_text = String::new();
        let mut prologue_restores: Vec<PrologueRestore> = Vec::new();
        // Both values are set by whichever install arm runs below. The compiler
        // checks that every arm sets them, so there is no placeholder that
        // could hide a missing assignment.
        let patch_route: crate::analytics::PatchRoute;
        let copy_count: usize;

        // The next number is one above the highest this function already has.
        // Each function therefore counts on its own, and a later patch keeps
        // counting after a rollback rather than reusing old numbers.
        let generation = self
            .generations
            .iter()
            .filter(|existing| existing.function == qualified)
            .map(|existing| existing.number)
            .max()
            .unwrap_or(0)
            + 1;
        // Keep the image loaded before anything is written. `Library` unmaps on
        // drop, and any address this install writes into a slot or a function
        // prologue points into this image for the rest of the process. The
        // image is shared by the whole process, not owned by this session: a
        // later patch must also reach the copies inside it. The routes below
        // receive the patch list *without* this entry, so an image is never
        // offered its own replacement.
        patches.push(LoadedPatch {
            function: qualified.to_string(),
            generation,
            library: built_body.library,
        });
        let library = &patches[patches.len() - 1].library;
        let existing_patches = &patches[..patches.len() - 1];

        // Install at the frame boundary. The routes check the signature before
        // writing anything, so a reshaped function is never applied behind a
        // call site compiled for the old shape.
        //
        // This is what the function runs now. The fan-out below puts its
        // copies back on it if one copy refuses the replacement.
        let previous_installation = self.previous_install(qualified);
        match function_kind {
            // A system is dispatched through the engine's own registry. The
            // engine is one shared library, so the registry lives once per
            // process and one install reaches every caller.
            source::HotFunctionKind::System => {
                let lookup_name = format!("{PATCH_NAME_PREFIX}{qualified}");
                let (found_address, found_signature_hash) = resolve_in(library, &lookup_name)
                    .map_err(|detail| PatchRefusal::new(failure_code::RESOLVE, detail))?;
                engine
                    .hot_patch(qualified, found_address, found_signature_hash)
                    .map_err(|error| PatchRefusal::new(failure_code::INSTALL, error.to_string()))?;
                replacement_address = found_address;
                signature_hash = found_signature_hash;
                // The registry is process-wide, so the fan-out count here is
                // one copy.
                patch_route = crate::analytics::PatchRoute::EngineSlot;
                copy_count = 1;
            }
            // This arm handles a plain function with no attribute, so no slot
            // exists to install into. Its running copies are redirected by
            // overwriting their first bytes.
            //
            // The annotated case, handled below, has no central registry
            // either. Its redirect slot is a static compiled into each artifact
            // that links the crate: the project DLL embeds its own copy of a
            // module it depends on, and so does every other module that links
            // it. Offering the replacement to all of them is what avoids the
            // cascading project reload this edit would otherwise trigger.
            source::HotFunctionKind::PlainFunction if !declaration.is_annotated => {
                let found_address = resolve_patch_address(library)
                    .map_err(|detail| PatchRefusal::new(failure_code::RESOLVE, detail))?;
                prologue_restores = prologue_patch_everywhere(
                    targets,
                    existing_patches,
                    qualified,
                    found_address,
                    &declaration.signature,
                )
                .map_err(|detail| PatchRefusal::new(failure_code::INSTALL, detail))?;
                replacement_address = found_address;
                patch_route = crate::analytics::PatchRoute::Prologue;
                copy_count = prologue_restores.len();
            }
            source::HotFunctionKind::PlainFunction => {
                // A method patch registers under the prefixed running name,
                // because its body lives in a local trait instead of at the
                // patch crate's own module path.
                let lookup_name = if declaration.takes_receiver {
                    format!("{PATCH_NAME_PREFIX}{qualified}")
                } else {
                    format!("{crate_name}::{function}")
                };
                let (found_address, found_signature_text) = resolve_plain_in(library, &lookup_name)
                    .map_err(|detail| PatchRefusal::new(failure_code::RESOLVE, detail))?;
                copy_count = install_everywhere(
                    targets,
                    existing_patches,
                    slot_name,
                    found_address,
                    &found_signature_text,
                    previous_installation,
                )
                .map_err(|detail| PatchRefusal::new(failure_code::INSTALL, detail))?;
                replacement_address = found_address;
                signature_text = found_signature_text;
                patch_route = crate::analytics::PatchRoute::ArtifactSlot;
            }
        }
        stages.activate = activate_started.elapsed().as_secs_f64() * 1000.0;

        self.generations.push(Generation {
            prologue_history_dropped: false,
            function: qualified.to_string(),
            // The form the delivering route looks up, so a rollback asks the
            // same question the install did.
            lookup_name: match function_kind {
                source::HotFunctionKind::PlainFunction if declaration.is_annotated => {
                    slot_name.to_string()
                }
                _ => qualified.to_string(),
            },
            number: generation,
            address: replacement_address,
            kind: function_kind,
            signature_hash,
            signature: signature_text,
            prologue_restores,
            installed_at: Instant::now(),
        });
        self.active_generations
            .insert(qualified.to_string(), generation);
        debug!(
            target: telemetry_target::HOT_RELOAD,
            function = qualified,
            generation,
            generations = self.generations.len(),
            "patch generation installed"
        );
        save_stages.merge(&stages);

        Ok(ApplyResult {
            generation,
            artifact_bytes: built_body.artifact_bytes,
            route: patch_route,
            copies: copy_count,
        })
    }

    /// Produces the patch source for one edited function.
    ///
    /// The function is copied as written, and everything it names comes from
    /// the same artifacts the running module linked. That is what keeps type
    /// layout and `TypeId` identical across the boundary. The imports it
    /// carries are the changed file's own module and the file's top-level
    /// `use` lines the body can name, each rooted where the patch crate
    /// resolves it.
    ///
    /// # Errors
    ///
    /// Returns the reason the source cannot be produced: the function is not in
    /// the changed source, a body with a receiver has no known receiver type,
    /// or its signature text cannot be read.
    fn generate(
        &self,
        path: &Path,
        new_contents: &str,
        declaration: &source::HotFunction,
        qualified: &str,
    ) -> Result<String, String> {
        let function = declaration.name.as_str();
        let function_kind = declaration.kind;
        // A method is found through its own `impl` block, not by bare name.
        // Two types may each implement `Default::default`, and one type may
        // have an inherent `draw` beside a trait `draw`. Taking the first
        // `fn draw` in the file would compile the wrong body and install it
        // without warning.
        let located_function = match (&declaration.self_type, declaration.takes_receiver) {
            (Some(self_type), true) => source::find_method(
                new_contents,
                function,
                self_type,
                declaration.trait_name.as_deref(),
            )
            .ok_or_else(|| {
                format!(
                    "cannot locate `{function}` in the `impl` block for `{self_type}`                      in the changed source"
                )
            })?,
            _ => source::find_function(new_contents, function)
                .ok_or_else(|| format!("cannot locate `{function}` in the changed source"))?,
        };

        let module_path = self.module_segments(path);
        let import_scope = imports::PatchScope {
            package: &self.package,
            source_root: &self.source_root,
            crate_root: &self.crate_root,
            module_path: &module_path,
        };
        let imports = import_scope
            .imports_for(
                new_contents,
                &source::top_level_use_statements(new_contents),
                &located_function.text,
            )
            .join("\n");
        let package = &self.package;

        // An un-annotated function is redirected by overwriting its prologue.
        // Nothing in the running artifact was prepared for this, so the patch
        // needs no slot, no descriptor and no signature text. It exports one
        // address, and the host writes a jump to it. A method still goes into a
        // local trait so its body can keep using `self`.
        if !declaration.is_annotated {
            let (definition, address_expression) = if declaration.takes_receiver {
                let self_type = declaration
                    .self_type
                    .as_deref()
                    .ok_or_else(|| format!("`{function}` has no known receiver type"))?;
                let signature = located_function
                    .text
                    .split_once('{')
                    .map(|(head, _)| head.trim().to_string())
                    .ok_or_else(|| format!("cannot read the signature of `{function}`"))?;
                (
                    format!(
                        "trait PillHotMethodPatch {{ {signature}; }}\n\
                         impl PillHotMethodPatch for {self_type} {{ {body} }}",
                        body = located_function.text
                    ),
                    format!("<{self_type} as PillHotMethodPatch>::{function}"),
                )
            } else {
                (located_function.text.clone(), function.to_string())
            };
            return Ok(format!(
                "// GENERATED by pill_development_host hot patching - do not edit.\n\
                 //\n\
                 // One edited function, compiled against the same artifacts the\n\
                 // running module linked. The host overwrites the prologue of\n\
                 // every loaded copy with a jump to the address exported below.\n\
                 #![allow(unused_imports, dead_code, unused_mut)]\n\
                 \n\
                 {imports}\n\
                 use {package}::*;\n\
                 \n\
                 {definition}\n\
                 \n\
                 /// Where this patch's new body is.\n\
                 #[no_mangle]\n\
                 pub extern \"C\" fn pill_patch_address() -> usize {{\n\
                 {address_expression} as *const () as usize\n\
                 }}\n\
                 \n\
                 // A patch links its own copy of everything the body calls, and\n\
                 // those copies are as patchable as any other artifact's. The\n\
                 // resolver is what lets a later patch reach them, so a chain of\n\
                 // hot functions composes instead of freezing at whatever the\n\
                 // callee looked like when this patch was compiled.\n\
                 ::pill_engine::pill_hot_resolver!(pill_patch_resolve);\n"
            ));
        }

        // A method with a receiver. Its body names `self`, so it cannot be
        // copied into a free function. The attribute is given the receiver type
        // and carries the body into a local trait implemented for that type
        // instead. The same macro generates the signature text on both sides,
        // so the two always match instead of depending on the host rebuilding
        // the string.
        if function_kind == source::HotFunctionKind::PlainFunction && declaration.takes_receiver {
            let self_type = declaration
                .self_type
                .as_deref()
                .ok_or_else(|| format!("`{function}` has no known receiver type"))?;
            return Ok(format!(
                "// GENERATED by pill_development_host hot patching - do not edit.\n\
                 //\n\
                 // One edited method, compiled against the same artifacts the\n\
                 // running module linked. The body is carried into a local trait\n\
                 // implemented for the receiver type, which is what lets it keep\n\
                 // using `self`.\n\
                 #![allow(unused_imports, dead_code, unused_mut)]\n\
                 \n\
                 {imports}\n\
                 use {package}::*;\n\
                 \n\
                 #[::pill_engine::pill_hot_fn(\n\
                 name = \"{PATCH_NAME_PREFIX}{qualified}\",\n\
                 self_type = {self_type}\n\
                 )]\n\
                 {body}\n\
                 \n\
                 // A distinct export name: the linked rlib already provides\n\
                 // `pill_hot_resolve`.\n\
                 ::pill_engine::pill_hot_resolver!(pill_patch_resolve);\n",
                body = located_function.text
            ));
        }

        // A plain function needs no registry entry. The generated source adds
        // `#[pill_hot_fn]` again, and the function is looked up under the patch
        // crate's own name. That name is unique per patch, so it cannot be
        // confused with the copy of the same function the linked rlib also
        // contributes.
        if function_kind == source::HotFunctionKind::PlainFunction {
            return Ok(format!(
                "// GENERATED by pill_development_host hot patching - do not edit.\n\
                 //\n\
                 // One edited function, compiled against the same artifacts the\n\
                 // running module linked.\n\
                 #![allow(unused_imports, dead_code, unused_mut)]\n\
                 \n\
                 {imports}\n\
                 use {package}::*;\n\
                 \n\
                 // The same attribute the module itself uses, so the signature\n\
                 // text the two sides compare is produced by one macro from one\n\
                 // piece of source rather than reconstructed by hand.\n\
                 #[::pill_engine::pill_hot_fn]\n\
                 {body}\n\
                 \n\
                 // A distinct export name: the linked rlib already provides\n\
                 // `pill_hot_resolve`.\n\
                 ::pill_engine::pill_hot_resolver!(pill_patch_resolve);\n",
                body = located_function.text
            ));
        }

        Ok(format!(
            "// GENERATED by pill_development_host hot patching - do not edit.\n\
             //\n\
             // One edited function, compiled against the same artifacts the\n\
             // running module linked.\n\
             #![allow(unused_imports, dead_code, unused_mut)]\n\
             \n\
             {imports}\n\
             use {package}::*;\n\
             \n\
             // `name` pins the registry entry to the ORIGINAL path, prefixed so\n\
             // it cannot collide with the copy the linked rlib also carries.\n\
             #[::pill_engine::pill_hot(name = \"{PATCH_NAME_PREFIX}{qualified}\")]\n\
             {body}\n\
             \n\
             // A distinct export name: the linked rlib already provides\n\
             // `pill_hot_resolve`.\n\
             ::pill_engine::pill_hot_resolver!(pill_patch_resolve);\n",
            body = located_function.text
        ))
    }

    /// Returns the module path of a source file, with the package name first.
    ///
    /// The first segment is the package name, which takes the place of `crate`
    /// in `module_path!()`. A function in the crate root therefore sits at
    /// `<package>`, and one in `src/color.rs` at `<package>::color`.
    ///
    /// The crate root contributes no extra segment whatever it is named: a
    /// manifest may point `[lib] path` at a file other than `lib.rs`, and
    /// treating that file as a module would name every function in the crate
    /// one segment too deep.
    fn module_segments(&self, path: &Path) -> Vec<String> {
        let mut path_segments = vec![self.package.clone()];
        if path == self.crate_root {
            return path_segments;
        }
        let Ok(relative) = path.strip_prefix(&self.source_root) else {
            return path_segments;
        };
        let components: Vec<_> = relative.components().collect();
        for (index, component) in components.iter().enumerate() {
            let name = component.as_os_str().to_string_lossy();
            let is_last = index + 1 == components.len();
            let name = if is_last {
                name.strip_suffix(".rs").unwrap_or(&name).to_string()
            } else {
                name.into_owned()
            };
            // `lib.rs` and `mod.rs` stand for the module their folder already
            // names, so they add no segment of their own.
            if name == "lib" || name == "mod" {
                continue;
            }
            path_segments.push(name);
        }
        path_segments
    }

    /// The full name of a declaration: module path, type, function.
    ///
    /// This is what a build script registers, because it can see the enclosing
    /// `impl` block. It is the name the prologue route looks up and the one
    /// used for display and generation bookkeeping.
    fn qualified_name(&self, path: &Path, declaration: &source::HotFunction) -> String {
        let path_segments = self.module_segments(path);
        // The scanner builds the name so the host asks for exactly the one the
        // build script recorded. A hand-written version would drift: an
        // inherent method is named through its receiver type, and a trait
        // method carries both the type and the trait in its name.
        source::inventory_name(&self.package, &path_segments[1..], declaration)
    }

    /// The path a dispatch slot is registered under, which leaves out the type.
    ///
    /// It is deliberately different from [`Self::qualified_name`], and the
    /// difference is forced rather than chosen. A slot's descriptor is an item,
    /// and an item inside a method body cannot name `Self` (`error[E0401]`). A
    /// `#[pill_hot_fn]` on a method can therefore only register
    /// `module_path!() + "::" + name`, while a build script that read the
    /// `impl` block directly has no such limit.
    ///
    /// Two hot methods that share a name in one module collide on the slot
    /// route. The scanner sees both and refuses the edit instead of installing
    /// into whichever registered first.
    fn slot_lookup_name(&self, path: &Path, declaration: &source::HotFunction) -> String {
        let mut lookup_segments = self.module_segments(path);
        lookup_segments.push(declaration.name.clone());
        lookup_segments.join("::")
    }

    /// Brings the staged rlib back in step with cargo's own output.
    ///
    /// A patch links two kinds of rlib: the crate's own comes from the staged
    /// copy, while its dependencies' `--extern` entries come from the replayed
    /// cargo line and point into the module build tree. If the two drift apart,
    /// the compile fails with `error[E0463]: can't find crate for <this
    /// crate>`, which names the wrong crate. The build worker adds a hint that
    /// the staged rlib no longer matches its dependencies.
    ///
    /// The source is the private module build tree under the host's profile
    /// directory ([`crate::config::module_build_artifact_directory`]); a bare
    /// `target/debug` holds nothing the host built. The host loads the module
    /// DLL, not this rlib, so a wrong-featured copy here can make this one
    /// patch fail; the next real build replaces it.
    ///
    /// # Errors
    ///
    /// Returns the error text when a freshly built rlib exists but cannot be
    /// copied over the staged copy.
    fn refresh_staged_rlib(&self) -> Result<(), String> {
        // A wrapper build compiles the extension as a dependency, so the
        // newest rlib under the build's `deps` directory is the one that build
        // produced. A direct build also leaves an unhashed copy that the same
        // helper finds.
        let Some(newest_rlib) = crate::build_runner::newest_extension_rlib(
            &self
                .workspace_root
                .join(crate::config::module_build_artifact_directory()),
            &self.package,
        ) else {
            // Cargo has not produced one; the staged copy is all there is.
            return Ok(());
        };
        if std::fs::metadata(&newest_rlib).is_err() {
            return Ok(());
        }
        // This is the same size and not-older rule the shared-rlib staging
        // uses, so a staged copy left by either path satisfies the other.
        if crate::build_runner::staged_copy_is_current(&newest_rlib, &self.package_rlib) {
            return Ok(());
        }

        std::fs::copy(&newest_rlib, &self.package_rlib).map_err(|error| {
            format!(
                "cannot refresh the staged rlib from {}: {error}",
                newest_rlib.display()
            )
        })?;
        debug!(
            target: telemetry_target::HOT_RELOAD,
            package = self.package.as_str(),
            source = %newest_rlib.display(),
            "restaged the rlib so the patch links a consistent closure"
        );
        Ok(())
    }

    /// Captures the compiler flags, asking cargo on first use.
    ///
    /// # Errors
    ///
    /// Returns the error text when the line has to be captured and cargo cannot
    /// run, the build fails, the crate root cannot be touched, or cargo prints
    /// no invocation for this package.
    fn rustc_line(&mut self) -> Result<&CargoRustcLine, String> {
        if self.rustc_line.is_none() {
            let cache_path = compile::flags_cache_path(&self.workspace_root, &self.package);
            // The crate root is deliberately NOT a freshness input. The flags
            // describe the dependency graph and feature set, and editing source
            // text does not change either. Including a source file would
            // invalidate the cache on every patch and re-run a full
            // `cargo build -v` for flags that were still correct.
            //
            // The crate's own staged rlib IS included. The flags describe the
            // closure this crate links, which changes when the crate is rebuilt:
            // at startup, on a module reload, or when the engine's feature set
            // changes. A cache captured against one feature set goes stale
            // silently against another, and rustc then reports `error[E0463]`
            // when the patch links the fresh rlib against the old externs.
            //
            // The staged copy is named rather than a build-tree path because a
            // wrapper build leaves the extension's rlib under a hashed name the
            // session cannot know. The refresh above brings the staged file up
            // to date, and the patch links it.
            let mut freshness_inputs = vec![
                self.workspace_root.join("Cargo.toml"),
                self.workspace_root.join("Cargo.lock"),
            ];
            freshness_inputs.push(self.package_rlib.clone());
            // The cargo configuration and the pinned toolchain also decide the
            // flags from outside the manifest. A wrapper, a target-directory
            // override or a toolchain switch changes what a replayed line means
            // without changing `Cargo.toml` or the lockfile. They are added only
            // when they exist: `load_if_fresh` returns no cached line for an
            // input it cannot read, so an unconditional push would disable the
            // cache in a workspace that has neither file.
            for relative in [".cargo/config.toml", "rust-toolchain.toml"] {
                let input = self.workspace_root.join(relative);
                if input.exists() {
                    freshness_inputs.push(input);
                }
            }
            let compiler_line = match CargoRustcLine::load_if_fresh(
                &cache_path,
                &freshness_inputs,
                &self.build_command,
            ) {
                Some(cached) => cached,
                None => {
                    let captured_line = CargoRustcLine::capture(
                        &self.workspace_root,
                        &self.package,
                        &self.crate_root,
                        &self.build_command,
                    )?;
                    let _ = captured_line.save(&cache_path, &self.build_command);
                    captured_line
                }
            };
            self.rustc_line = Some(compiler_line);
        }
        Ok(self.rustc_line.as_ref().expect("just populated"))
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Every `.rs` file under `root`, in stable order.
fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut source_paths = Vec::new();
    let mut directories_to_visit = vec![root.to_path_buf()];
    while let Some(directory) = directories_to_visit.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                directories_to_visit.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                source_paths.push(path);
            }
        }
    }
    source_paths.sort();
    source_paths
}

/// A path shortened to its file name, for console messages.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests;
