//! Native editor executable: the Pill engine's editor frontend.
//!
//! # Responsibilities
//!
//! - Install the engine report handler and the shared telemetry stack.
//! - Own the `winit` event loop, the main window, and the editor's own GPU
//!   device that draws the editor's UI.
//! - Host the project and its extensions, including the reloadable editor
//!   shell module, through `pill_development_host`.
//! - Convert the final error into one styled miette report at the single
//!   reporting boundary.
//!
//! # Design
//!
//! Nothing in this executable can change while it runs: the shared platform
//! layer, the loop and the UI presentation never reload. Everything that
//! should reload - panels, their commands and their state - lives in the
//! shell module this binary hosts. A change here needs a restart; a change
//! to the shell module hot-patches or reloads.

// Standard library
use std::path::PathBuf;

// External crates
use pill_core::error;
use pill_core::error::{engine_report, install_engine_report_handler};
use pill_core::telemetry::telemetry_target;

// Current crate
use crate::frontend::EditorError;

/// Reads and writes the OS clipboard for the runner's copy, cut and paste.
mod clipboard;
/// Hands one project asset to the OS as a file drag.
mod drag_out;
/// The editor's errors, with their stable diagnostic codes.
mod frontend;
/// The overlay drawn above the editor window: a DirectComposition visual on
/// Windows, the wgpu surface it presents, and the egui-wgpu renderer that
/// fills it.
mod overlay;
/// The windows, the GPU device and the frame loop.
mod runner;
/// The devops suite's scripted input channel.
mod test_channel;

// =============================================================================
// Telemetry
// =============================================================================

/// Installs the shared telemetry stack before the run loop starts.
///
/// Logging to the terminal is always on, and so is the console panel's
/// capture ring (the in-memory ring of log lines it reads). The return value
/// carries that ring. A file lane (a rolling log file) is added when
/// `ECS_LOG_DIR` is set.
///
/// Setup is best-effort: a failure only degrades telemetry and is reported
/// to stderr without aborting the run.
fn init_telemetry() -> pill_core::telemetry::capture::LogBuffer {
    // Step 1: read the optional log directory from `ECS_LOG_DIR`.
    let file_log_directory = std::env::var_os("ECS_LOG_DIR").map(PathBuf::from);
    // Step 2: install the stack, printing setup failures to stderr. The call
    // gets a clone of the buffer, so even a failed install still returns an
    // empty ring the console panel can read.
    let log_buffer = pill_core::telemetry::capture::LogBuffer::default();
    if let Err(error) =
        pill_player::init_telemetry_with_capture(file_log_directory, Some(log_buffer.clone()))
    {
        // This message cannot go through the logger, because it reports that
        // the logger itself did not install.
        eprintln!("[editor] telemetry setup failed: {error}");
    }
    log_buffer
}

// =============================================================================
// Reporting Boundary
// =============================================================================

/// Runs the editor and reports its final error once.
///
/// # Errors
///
/// Returns a styled report built by [`engine_report`] when the run ends with
/// an error. Before that, the failure is also sent to the log, so it appears
/// inside any active spans and in the log files.
fn main() -> miette::Result<()> {
    // Step 1: install the miette report handler before anything can fail.
    install_engine_report_handler();
    // Step 2: bring up the shared telemetry stack and keep the console
    // panel's capture ring.
    let log_buffer = init_telemetry();
    // Step 3: run the editor, then turn a failure into one report.
    let editor_run: Result<(), EditorError> = runner::run(log_buffer);
    editor_run.map_err(|error| {
        // The fatal error is also logged, so it appears inside any active
        // spans and in the log files next to the run's other messages.
        error!(
            target: telemetry_target::EDITOR,
            error = %error,
            "editor terminated with an error"
        );
        engine_report(error)
    })
}
