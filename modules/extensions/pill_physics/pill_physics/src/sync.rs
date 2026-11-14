//! The system that keeps the rapier world in step with the entities.
//!
//! # Responsibilities
//!
//! - Creates a body for every entity that carries both components, and drops
//!   the body of an entity that no longer does.
//! - Rebuilds a body whose components were edited, and moves a dynamic body
//!   whose transform was moved from outside the simulation.
//! - Hands a kinematic body the pose game code moved its transform to.
//! - Steps the simulation by the frame's time.
//! - Copies a dynamic body's stepped pose back into the entity's transform.

// Standard library
use std::collections::HashSet;

// External crates
use pill_engine::{system, Entity, Query, Res, ResMut, SystemError, Time, TransformComponent};

// Current crate
use crate::components::{BodyKind, ColliderComponent, RigidBodyComponent};
use crate::events::PhysicsEvents;
use crate::manager::PhysicsManager;

// =============================================================================
// physics_sync
// =============================================================================

/// Advances the simulation by one frame and moves its results into the
/// entities.
///
/// An entity is simulated while it carries both a
/// [`RigidBodyComponent`] and a [`ColliderComponent`]: a body with nothing to
/// collide with has no contact to resolve, so it is not a body this module
/// runs.
///
/// # Errors
///
/// Returns [`SystemError::MissingResource`] when the physics manager or the
/// engine clock is absent, which only happens in a world this module was not
/// registered in.
#[system(name = "physics", order = 900)]
pub fn physics_sync(
    time: Res<Time>,
    mut manager: ResMut<PhysicsManager>,
    mut events: ResMut<PhysicsEvents>,
    mut bodies: Query<(
        Entity,
        &RigidBodyComponent,
        &ColliderComponent,
        &mut TransformComponent,
    )>,
) -> Result<(), SystemError> {
    let Some(mut manager) = manager.get_mut() else {
        return Err(SystemError::MissingResource {
            name: String::from("PhysicsManager"),
        });
    };
    let Some(time) = time.get() else {
        return Err(SystemError::MissingResource {
            name: String::from("Time"),
        });
    };

    // Step 1: bring the simulation's bodies in line with the entities that
    // carry a body and a collider this frame.
    let mut live_entities = HashSet::new();
    for (entity, body, collider, transform) in bodies.iter_mut() {
        live_entities.insert(entity);
        if !manager.has_body(entity) {
            manager.spawn_body(entity, &transform, body, collider);
        } else if manager.apply_component_edits(entity, &transform, body, collider) {
            // The body was rebuilt from the edited components at the
            // transform's pose, so there is nothing more to carry over.
        } else if body.body_kind == BodyKind::Dynamic {
            // A dynamic body owns its pose, unless someone moved the transform
            // since the last write-back.
            manager.follow_moved_transform(entity, &transform);
        } else if body.body_kind == BodyKind::KinematicPosition {
            // A kinematic body is placed by game code, so its transform is the
            // authority and the body follows it.
            manager.push_kinematic(entity, &transform);
        }
    }
    manager.despawn_missing(&live_entities);

    // Step 2: advance the simulation by this frame's time, then publish what
    // touched what. The collection is replaced rather than appended to, so a
    // reader always sees the frame that just ran.
    manager.step(time.delta_seconds());
    let frame_events = manager.drain_events();
    if let Some(mut events) = events.get_mut() {
        *events = frame_events;
    }

    // Step 3: copy the stepped poses into the transforms of the bodies the
    // simulation moved. A fixed or kinematic body is left where it is.
    for (entity, body, _collider, mut transform) in bodies.iter_mut() {
        if body.body_kind == BodyKind::Dynamic {
            manager.write_back(entity, &mut transform);
        }
    }
    Ok(())
}
