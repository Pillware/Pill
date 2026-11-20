//! The background half of the fast patch: it compiles a body and loads the
//! library that comes out of it.
//!
//! # Responsibilities
//!
//! - Hold everything one patch build needs, so it can move to another thread
//!   and come back without borrowing the frame, the engine or the session.
//! - Run the replayed `rustc` invocation and load the library it produces. The
//!   artifact is read once here first, so the platform's scan of a newly
//!   written file is paid on this thread instead of at the frame boundary.
//!
//! # Design
//!
//! Patching one body (a `rustc` run plus a `LoadLibrary`) takes hundreds of
//! milliseconds, so it runs off the frame thread. The frame thread prepares a
//! [`PatchAttempt`], a worker turns it into [`BodyOutcome`]s, and the frame
//! boundary installs them.
//!
//! Nothing in this file touches the engine, a loaded module artifact or the
//! session. That is what makes the middle step safe on any thread: no dispatch
//! slot is written, no registry changes, and nothing that belongs to the
//! thread owning the boundary is used.

// Standard library
use std::path::PathBuf;
use std::time::SystemTime;

// External crates
use libloading::Library;
use pill_core::debug;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;

// Current crate
use super::compile::CargoRustcLine;
use super::{failure_code, source, PatchRefusal, PatchStages};

// =============================================================================
// Types
// =============================================================================

/// One pending patch attempt: what the frame thread prepared, and what a
/// worker or the activation still needs.
///
/// It owns its inputs, so it can outlive the frame's borrows while the build
/// runs. Nothing in it points into the engine, an artifact or the session.
pub(crate) struct PatchAttempt {
    /// The changed file, for the snapshot recorded once every body is live.
    pub(super) path: PathBuf,
    /// The file's contents as read during classification.
    pub(super) new_contents: String,
    /// The modification time when those contents were read.
    pub(super) modified: Option<SystemTime>,
    /// One entry per changed body, in the order they were classified.
    pub(super) bodies: Vec<PreparedBody>,
    /// Timings so far; the worker and the activation add theirs.
    pub(super) stages: PatchStages,
    /// When the attempt began, so the reported total covers preparation, the
    /// build and the wait for the next frame boundary. The total is measured
    /// from here because the build happens between frames, so the frame that
    /// collects the attempt cannot time the whole span itself.
    pub(crate) started: Instant,
}

/// A changed body, ready to compile.
pub(crate) struct PreparedBody {
    /// The declaration as the scanner reported it.
    pub(super) declaration: source::HotFunction,
    /// The name the engine registry and the prologue route are keyed by.
    pub(super) qualified: String,
    /// The name a dispatch slot is keyed by, which leaves out the receiver
    /// type. It is kept here from preparation because activation files the
    /// slot route under it.
    pub(super) slot_name: String,
    /// The patch crate's name, unique within the process.
    pub(super) crate_name: String,
    /// The generated source on disk.
    pub(super) source_path: PathBuf,
    /// Where the compiled library is written.
    pub(super) artifact_path: PathBuf,
    /// `--extern` entries beyond the ones the replayed line carries.
    pub(super) extra_externs: Vec<String>,
    /// The directory holding the dependency rlibs the replayed line names.
    pub(super) staged_dependencies: PathBuf,
    /// The captured compiler invocation to replay.
    pub(super) line: CargoRustcLine,
    /// The package name, for the "can't find crate" hint.
    pub(super) package: String,
}

/// What the build produced for one body.
pub(crate) enum BodyOutcome {
    /// Compiled and loaded; the frame boundary installs it.
    Built(BuiltBody),
    /// The build failed. The running implementation is untouched, and no
    /// later body was attempted: the build stops at the first failure.
    Failed(PatchRefusal),
}

/// A compiled and loaded patch image.
pub(crate) struct BuiltBody {
    /// The loaded library. It is never unloaded, because a dispatch slot will
    /// point inside it for the rest of the process.
    pub(super) library: Library,
    /// Size on disk, for the analytics line.
    pub(super) artifact_bytes: u64,
    /// Milliseconds spent in `rustc`.
    pub(super) compile_ms: f64,
    /// Milliseconds spent loading the image.
    pub(super) load_ms: f64,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Compiles and loads the bodies of one attempt, in order.
///
/// Usually runs on the patch build thread; when that thread cannot start, the
/// caller runs it inline instead. It stops at the first failure: the bodies
/// before it are still installable, and the snapshot is not advanced unless
/// every body installs.
pub(crate) fn build_attempt(mut attempt: PatchAttempt) -> (PatchAttempt, Vec<BodyOutcome>) {
    let mut body_outcomes = Vec::with_capacity(attempt.bodies.len());
    for body in &attempt.bodies {
        match build_body(body) {
            Ok(built_body) => {
                let stages = PatchStages {
                    compile: built_body.compile_ms,
                    load: built_body.load_ms,
                    ..PatchStages::default()
                };
                attempt.stages.merge(&stages);
                body_outcomes.push(BodyOutcome::Built(built_body));
            }
            Err(failure) => {
                body_outcomes.push(BodyOutcome::Failed(failure));
                break;
            }
        }
    }
    (attempt, body_outcomes)
}

/// Runs the replayed compiler over one body and loads what it produces.
///
/// # Errors
///
/// `failure_code::COMPILE` when the compiler cannot be started or rejects the
/// generated source, and `failure_code::LOAD` when the produced library cannot
/// be loaded into the process.
fn build_body(body: &PreparedBody) -> Result<BuiltBody, PatchRefusal> {
    let source_path = body.source_path.as_path();
    let artifact_path = body.artifact_path.as_path();
    let crate_name = body.crate_name.as_str();
    let staged_dependencies = Some(body.staged_dependencies.as_path());

    // Step 1: Replay the captured line over the generated source.
    let compile_started = Instant::now();
    let compiler_output = body
        .line
        .replay(
            source_path,
            artifact_path,
            crate_name,
            &body.extra_externs,
            staged_dependencies,
        )
        .output()
        .map_err(|error| {
            PatchRefusal::new(failure_code::COMPILE, format!("cannot run rustc: {error}"))
        })?;
    let compile_ms = compile_started.elapsed().as_secs_f64() * 1000.0;
    if !compiler_output.status.success() {
        let stderr = String::from_utf8_lossy(&compiler_output.stderr);
        // A link failure reports `error: linking with ... failed` first and
        // only gives the real cause on the linker's own line further down, so
        // that line is preferred when present.
        let linker_failure_line = stderr
            .lines()
            .map(str::trim)
            .find(|line| line.contains("rust-lld: error:") || line.contains("LNK"));
        let first_error_line = stderr
            .lines()
            .find(|line| line.starts_with("error"))
            .unwrap_or("rustc rejected the generated patch");
        let package = body.package.as_str();
        let detail = match linker_failure_line {
            Some(linker_failure_line) => format!("{first_error_line} - {linker_failure_line}"),
            // `can't find crate for <this crate>` names the crate being
            // patched, which reads as though its rlib is missing. The rlib is
            // there; it no longer matches the dependency rlibs the replayed
            // line points at, because one of them was rebuilt.
            None if first_error_line.contains("E0463") && first_error_line.contains(package) => {
                format!(
                    "{first_error_line} - the staged rlib no longer matches the dependency \
                     rlibs it links against; a crate `{package}` depends on was \
                     rebuilt after it was staged"
                )
            }
            None => first_error_line.to_string(),
        };
        // The exact command, so a failure can be reproduced by hand instead
        // of guessed at. DEBUG because it is long and only wanted once
        // something has already gone wrong.
        debug!(
            target: telemetry_target::HOT_RELOAD,
            command = body
                .line
                .replay_args(
                    source_path,
                    artifact_path,
                    crate_name,
                    &body.extra_externs,
                    staged_dependencies,
                )
                .join(" ")
                .as_str(),
            "the patch compile that failed"
        );
        return Err(PatchRefusal::new(failure_code::COMPILE, detail));
    }

    // Step 2: Read the newly written file once, on this thread, before loading
    // it. The first read of a new multi-megabyte file pays the platform's
    // anti-malware scan, and paying it here keeps that cost off the frame
    // boundary. The load below then reads a file the page cache already holds.
    // The read result is ignored on purpose: a file that cannot be read fails
    // the load with a better error than this could produce.
    let _ = std::fs::read(artifact_path);

    // Step 3: Load the image and time the load.
    let load_started = Instant::now();
    // SAFETY: the path names the artifact `rustc` wrote a moment ago from the
    // generated source of this same build, so loading it runs only the patch's
    // own initializers against the dependency graph the replayed line named.
    let library = unsafe { Library::new(artifact_path) }.map_err(|error| {
        PatchRefusal::new(
            failure_code::LOAD,
            format!("cannot load the patch library: {error}"),
        )
    })?;
    let load_ms = load_started.elapsed().as_secs_f64() * 1000.0;

    // Read before the install, so the analytics line carries the same artifact
    // size a module reload does. Best effort: a patch still works when its
    // size cannot be read.
    let artifact_bytes = std::fs::metadata(artifact_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    Ok(BuiltBody {
        library,
        artifact_bytes,
        compile_ms,
        load_ms,
    })
}
