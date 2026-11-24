//! Pill's rigid-body physics: a rapier world behind engine components.
//!
//! # Responsibilities
//!
//! - Declares the component, manager and system modules.
//! - Re-exports the vocabulary a project names: the two components, the body
//!   kinds and shapes, and the manager.
//! - Exposes [`register`], the entry point the host calls on every generation
//!   of this module.
//!
//! # Design
//!
//! An entity is simulated while it carries a
//! [`RigidBodyComponent`] and a [`ColliderComponent`]. The pose stays in the
//! engine's
//! [`TransformComponent`](pill_engine::TransformComponent): the manager builds
//! a body from it and the sync writes a stepped body's pose back into it, so
//! physics needs nothing from a component the renderer and the editor do not
//! already hold.
//!
//! Every value on the boundary is a plain number. rapier is named only inside
//! the manager and the system, which keeps it out of a project's dependency
//! graph, a saved scene file and the managed mirror.

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::*;

// =============================================================================
// Function inventory
// =============================================================================

// The build script scans this crate and writes `function_inventory.rs`, which
// holds one address entry for every function a patch can reach. Including it
// here is what makes those functions resolvable by their qualified paths, with
// nothing annotated in this file. It has to sit at the crate root because the
// generated entries name the modules from the crate root.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Modules
// =============================================================================

/// The two components, the body kinds and the collider shapes.
pub mod components;
/// The collider outlines drawn while the settings document asks for them.
pub mod debug_draw;
/// The three collision events a project reacts to.
pub mod events;
/// The manager that owns the rapier world.
pub mod manager;
/// Ray queries against the simulated world.
pub mod queries;
/// The system that drops the simulation when the world it mirrors was
/// restored.
pub mod restore;
/// The tuning document and its `.physics_settings` asset.
pub mod settings;
/// The system that steps the simulation each frame.
pub mod sync;

// =============================================================================
// Re-exports
// =============================================================================

// The module that declares a type is an implementation detail. A caller names
// `pill_physics::RigidBodyComponent` wherever the type's file lives.
pub use components::{BodyKind, ColliderComponent, ColliderShape, RigidBodyComponent};
pub use debug_draw::draw_collider_outline;
pub use events::{CollisionStarted, CollisionStopped, ContactForce, PhysicsEvents};
pub use manager::{
    PhysicsManager, PhysicsStatistics, DEFAULT_FIXED_TIMESTEP_SECONDS, DEFAULT_GRAVITY,
    MAXIMUM_SUBSTEPS_PER_FRAME,
};
pub use queries::RayHit;
pub use restore::physics_restore;
pub use settings::PhysicsSettings;

// =============================================================================
// Artifact entry points
// =============================================================================

/// Runs once per module image, after [`register`], and builds nothing of its
/// own.
///
/// The simulation starts empty on purpose: a world's bodies belong to the
/// project, which spawns them. The attribute is what writes the module's entry
/// points, so the generated `host_module_pill_physics` wrapper can call in.
///
/// Returns 0.
#[pill_module(register = register)]
pub fn init(_engine: &mut Engine) -> u32 {
    0
}

// =============================================================================
// register
// =============================================================================

/// Declares the extension's components, settings surface and resources.
///
/// The host calls this on every generation, so every step is safe to repeat: the
/// component declarations are idempotent, and a value is only inserted when the
/// world does not already hold one. A kept manager's rapier world is rebuilt
/// when this image is not the one that built it; the bodies come back from the
/// components, which hold their pose and their last velocities.
///
/// Returns 0.
pub fn register(engine: &mut Engine) -> u32 {
    // Step 1: Register the components through their derive's registrars, so they
    // arrive with their field layouts and the Inspector draws real rows.
    components::register(engine.world_mut());

    // Step 2: The system writes each stepped pose into the entity's transform,
    // so the component has to exist even in a headless project that loads no
    // renderer. A renderer's data crate usually registers it; registration is
    // idempotent, so a world that already has it is unaffected.
    pill_engine::register_transform_component(engine.world_mut());

    // Step 4: Insert what the module owns once; everything else here is
    // per-frame data and starts empty.
    // A manager kept from an earlier image holds a rapier world that points
    // into that image, which the host will unload. The previous image is
    // still loaded now, so this is the one safe moment to drop that world;
    // the sync rebuilds every body from the components on its next pass.
    if let Some(manager) = engine.world_mut().get_resource_mut::<PhysicsManager>() {
        if manager.rebuild_if_built_by_another_image() {
            pill_core::info!(
                target: telemetry_target::ECS,
                "[physics] rebuilt the simulation for the reloaded module; bodies return from their components"
            );
        }
    }
    if engine.world().get_resource::<PhysicsManager>().is_none() {
        engine.world_mut().insert_resource(PhysicsManager::new());
    }
    // A manager built while a session runs describes the world that session
    // restored to, so it must not clear itself on the first frame.
    let restore_epoch = engine.world().restore_epoch();
    if let Some(manager) = engine.world_mut().get_resource_mut::<PhysicsManager>() {
        manager.adopt_restore_epoch(restore_epoch);
    }
    if engine.world().get_resource::<PhysicsEvents>().is_none() {
        engine.world_mut().insert_resource(PhysicsEvents::default());
    }
    // The outlines need a collection to draw into. A renderer replaces this one
    // with the collection its frame draws; a headless project has none, and an
    // empty collection costs nothing.
    if engine.world().get_resource::<DebugDraw>().is_none() {
        engine.world_mut().insert_resource(DebugDraw::new(false));
    }

    // Step 5: Declare the settings surface the simulation is tuned through. This
    // runs after the manager is live, and repeats on every generation so the
    // surface never points at code the reload unloaded.
    pill_engine::register_settings::<PhysicsManager>(engine);

    // Step 6: Announce the registration.
    pill_core::info!(
        target: telemetry_target::ECS,
        "pill_physics module registered"
    );
    0
}
