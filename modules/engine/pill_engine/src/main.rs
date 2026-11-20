//! Minimal binary that checks the ECS library starts up.
//!
//! # Responsibilities
//!
//! - Installs telemetry and constructs an [`Engine`].
//! - Logs the crate version and the live entity count as an initialisation
//!   check.
//! - Lists the package's example programs and how to run one.
//!
//! # Design
//!
//! This is the binary target of the `pill_engine` package. It links the
//! package's library and does nothing beyond starting an [`Engine`], so a
//! library that cannot start fails visibly here. Real workloads live in the
//! example programs (`cargo run --example <name>`).

// Current crate
use pill_engine::Engine;

// =============================================================================
// Entry Point
// =============================================================================

/// Runs the start-up check: installs telemetry, constructs an [`Engine`], and
/// logs the crate version with the world's live entity count.
///
/// A new world holds no entities, so the count starts at 0. The lines after it
/// list the package's example programs and the command that runs one.
fn main() {
    // Step 1: Install telemetry first. The result is ignored on purpose: if
    // the install fails, the logging macros below simply print nothing.
    let _ = pill_core::telemetry::TelemetryBuilder::new().init();

    // Step 2: Construct the engine, which brings the ECS world up.
    let engine = Engine::new();

    // Step 3: Log the version and entity count as the initialisation check.
    pill_core::info!(
        "pill_engine v{} - {} entities",
        env!("CARGO_PKG_VERSION"),
        engine.world().entity_count(),
    );

    // Step 4: List the example programs the package ships and how to run one.
    pill_core::info!("Run examples with: cargo run --example <name>");
    pill_core::info!("Available: iterators_stress_test, change_detection_demo,");
    pill_core::info!("           scripting_demo, parallel_systems_demo, resources_demo,");
    pill_core::info!("           tracy_live (needs --features profiling)");
}
