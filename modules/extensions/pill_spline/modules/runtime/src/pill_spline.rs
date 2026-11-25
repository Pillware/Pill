//! Extension providing spline paths.
//!
//! # Responsibilities
//!
//! - Declare the crate's modules and re-export the types a project, the editor
//!   tool and the managed mirror name.
//! - Register the [`Spline`] component through the extension ABI and fill the
//!   world up to the demo path count ([`register`]).
//! - Provide the artifact's one-time entry point ([`init`]).
//!
//! # Design
//!
//! This file holds the artifact surface and nothing else: the component and its
//! sampling modes live in [`component`], the curve maths in [`math`], the
//! drawing in [`gizmo`], and the seeded path in [`demo`]. Keeping the
//! component out of this file is what lets a suite edit one concern without
//! rewriting the entry points.
//!
//! [`register`] is the half a reload repeats, so it is idempotent: it fills up
//! to [`DEMO_SPLINE_COUNT`] rather than spawning a fresh path on every
//! generation, and a failed generation rolls back by calling it again on the
//! older image. [`init`] exists because a statically linked build loads the
//! artifact once and calls that entry point alone, so it defers to `register`.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::*;

/// The component, its sampling modes and the mirrored entry points.
pub mod component;
/// The seeded demonstration path and its entity count.
pub mod demo;
/// The curve, its control points and its tangents, drawn as editor-only.
pub mod gizmo;
/// The two curve segments a spline samples from.
pub mod math;

pub use component::{
    register_component, Spline, TangentMode, TooManyControlPoints, MAX_CONTROL_POINTS,
};
pub use demo::{demo_spline, DEMO_SPLINE_COUNT};
pub use gizmo::draw_spline_gizmo;
pub use math::{catmull_rom, hermite};

// The build script scans this crate and emits one address entry per function
// into `function_inventory.rs`; the `include!` is what makes every function
// resolvable by qualified path with nothing in this file annotated.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Registration
// =============================================================================

/// Registers the module's components against the host engine and fills the
/// world up to its spline count.
///
/// This is the half a reload repeats. It must be idempotent, because a first
/// load reaches it too, immediately before [`init`], and a failed generation
/// rolls back by calling it again on the older image.
///
/// The module registers no system: it contributes a component type and the math
/// to sample it, leaving movement along a path to whoever owns that behaviour.
///
/// # Errors
///
/// Returns 1 when an entity cannot be created.
pub fn register(engine: &mut Engine) -> u32 {
    // Fill up to the target count rather than spawning a new path on every
    // rebuild, because a reload keeps the entities already created - and fills
    // back up when a dropped column took them with it.
    let existing_spline_count = {
        let mut query = Query::<&Spline>::new(engine.world_mut());
        query.iter_mut().count()
    };
    for _ in existing_spline_count..DEMO_SPLINE_COUNT {
        if engine
            .world_mut()
            .create_entity()
            .with(demo_spline())
            .build()
            .is_err()
        {
            // Report the failure so the host keeps the previous generation
            // instead of running with a half-populated world.
            return 1;
        }
    }

    // NOTE: this line is asserted on. `MODULE_REGISTERED_MESSAGE` in
    // `devops/core/suite_common.py` matches the message text against the
    // host's stdout, and scenarios in `devops/tests/test_hot_reload_suite.py`
    // additionally require the `existing=` field - which is what proves a
    // reload preserved the entities the previous generation created rather
    // than spawning a fresh set. Reword either and those suites fail with
    // "Missing required token", which reads like a reload failure and is not.
    pill_core::info!(
        target: telemetry_target::ECS,
        splines = DEMO_SPLINE_COUNT,
        existing = existing_spline_count,
        max_control_points = MAX_CONTROL_POINTS,
        "pill_spline module registered"
    );
    0
}

/// The artifact's one-time entry point.
///
/// The module keeps nothing that has to run once: everything it owns is
/// registration, and [`register`] re-runs all of it on every generation. This
/// function exists because a statically linked build loads the module once and
/// calls this entry point alone, so it defers to [`register`].
///
/// Public so the generated `host_module_pill_spline` wrapper can carry it as
/// `pill_module_init` for the host to find in a loaded DLL.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[path = "pill_spline_tests.rs"]
mod pill_spline_tests;
