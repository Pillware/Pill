//! Minimal extension module: one persistable component, one system, and the
//! C# mirror fixtures the bridge suite calls.
//!
//! # Responsibilities
//!
//! - Defines the `ModuleTest` component and the `module_test_processor` system.
//! - Registers both through the extension ABI when the host loads it.
//! - Keeps its entities filled to a target count across hot reloads.
//! - Carries the mirrored value types and free function a C# project reaches
//!   without a component row ([`mirror`]).
//!
//! # Design
//!
//! The crate is a plain `rlib`. The generated `host_module_pill_test` wrapper
//! links it into the `cdylib` that the host stages and loads at runtime, beside
//! the project module and reloaded on its own. All registration work lives in
//! [`register`], a plain Rust function, so the same crate can also be linked
//! statically into a monolithic build. The `extern "C"` exports that
//! `#[pill_module]` emits wrap [`register`]: they catch a panic and turn it
//! into a status code, because a panic that unwound across the module boundary
//! would abort the process.
//!
//! The component is registered as persistable, so its values survive a reload.
//! The reload matches persisted types by name, gives a newly added field its
//! default, and ignores a field the type no longer declares.

// External crates
use pill_core::info;
use pill_core::telemetry::telemetry_target;
use pill_engine::*;

/// The C# mirror fixtures: [`mirror::Tint`], [`mirror::Counter`] and
/// [`mirror::tint_alpha`].
pub mod mirror;

// =============================================================================
// Constants
// =============================================================================

/// Number of entities the module keeps alive.
///
/// Hot reload keeps the entities the previous generation created, so
/// initialization fills the world up to this count instead of spawning a
/// fresh batch each time.
const MODULE_TEST_ENTITY_COUNT: usize = 4;

/// Frame interval a progress report would use, counted in frames.
///
/// Nothing emits a report, so nothing reads the constant and the `allow`
/// above keeps the dead-code lint quiet.
#[allow(dead_code)]
const REPORT_INTERVAL_FRAMES: u64 = 300;

/// Fixed time step, in seconds, added to every entity's accumulated time.
///
/// Only [`module_test_processor`] uses it, and [`register`] installs that
/// system.
const FIXED_DELTA_TIME: f32 = 1.0 / 60.0;

// =============================================================================
// Component
// =============================================================================

/// Per-entity state this module defines.
///
/// The host carries the component's values across hot-reload generations, so
/// the layout is pinned with `#[repr(C)]` and every field stays serde
/// compatible. Adding or removing a field is safe: serde matches fields by
/// name, a new field gets its default, and a removed field is ignored.
#[component(persistable)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ModuleTest {
    /// Number of frames this entity has been processed for.
    pub processed_frame_count: u64,
    /// Accumulated simulated time, in seconds.
    pub accumulated_time: f32,
}

// =============================================================================
// Systems
// =============================================================================

/// Advances every `ModuleTest` component by one frame.
///
/// Registered under the name `module_test_processor`. The scheduler derives
/// the system's access pattern from the query signature, so it places the
/// system among the project's systems automatically.
#[system(name = "module_test_processor", order = 110)]
fn module_test_processor(mut query: Query<&mut ModuleTest>) -> Result<(), SystemError> {
    for mut state in query.iter_mut() {
        state.processed_frame_count += 1;
        state.accumulated_time += FIXED_DELTA_TIME;
    }
    Ok(())
}

// =============================================================================
// Registration
// =============================================================================

/// Fills the world up to the module's entity count.
///
/// This is the half a reload repeats: the resources and entities whose code
/// moved with the arriving image. It must be idempotent, because a first load
/// reaches it too, immediately before [`init`], and a failed generation rolls
/// back by calling it again on the older image.
///
/// The system needs no registration of its own: [`module_test_processor`]
/// declares itself, so the engine registers it before this function runs.
///
/// # Errors
///
/// Returns 1 when an entity cannot be created. The host reads any non-zero
/// status as a failed generation and rolls back to the previous library.
pub fn register(engine: &mut Engine) -> u32 {
    // Step 1: Fill the world up to the target count instead of spawning a
    // fresh batch, because a reload keeps the entities the previous generation
    // created - and fills back up when a dropped column took them with it.
    let existing_entity_count = {
        let mut query = Query::<&ModuleTest>::new(engine.world_mut());
        query.iter_mut().count()
    };
    for _ in existing_entity_count..MODULE_TEST_ENTITY_COUNT {
        if engine
            .world_mut()
            .create_entity()
            .with(ModuleTest::default())
            .build()
            .is_err()
        {
            // Report the failure so the host keeps the previous generation
            // instead of running with a half-populated world.
            return 1;
        }
    }

    info!(
        target: telemetry_target::ECS,
        entities = MODULE_TEST_ENTITY_COUNT,
        existing = existing_entity_count,
        "pill_test module registered"
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
/// Component registration happens automatically from the `PillComponent`
/// derive before either entry point runs. Public so the generated
/// `host_module_pill_test` wrapper can export it as `pill_module_init`.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}
