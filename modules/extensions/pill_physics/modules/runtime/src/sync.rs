//! The system that keeps the rapier world in step with the entities.
//!
//! # Responsibilities
//!
//! - Creates a body for every entity that carries both components, places a
//!   static collider for every entity that carries a collider alone, and drops
//!   whichever an entity no longer qualifies for.
//! - Applies edits to either component to the live body, and moves a dynamic
//!   body whose transform was moved from outside the simulation.
//! - Copies a dynamic body's stepped pose and velocities back into the
//!   entity's transform and rigid body component.
//! - Hands a kinematic body the pose game code moved its transform to.
//! - Steps the simulation by the frame's time.

// Standard library
use std::collections::HashSet;

// External crates
use pill_engine::{
    system, Entity, Query, Res, ResMut, SystemError, Time, TransformComponent, Without,
};

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
/// An entity with both a [`RigidBodyComponent`] and a [`ColliderComponent`] is
/// a body the simulation moves. An entity with a collider alone is static
/// geometry - or a trigger zone, when the collider is a sensor - that bodies
/// hit and that moves only with its transform. A rigid body alone has nothing
/// to collide with and is not simulated.
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
        &mut RigidBodyComponent,
        &ColliderComponent,
        &mut TransformComponent,
    )>,
    mut static_colliders: Query<
        (Entity, &ColliderComponent, &TransformComponent),
        Without<RigidBodyComponent>,
    >,
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
    let mut live_bodies = HashSet::new();
    for (entity, body, collider, transform) in bodies.iter_mut() {
        live_bodies.insert(entity);
        if !manager.has_body(entity) {
            manager.spawn_body(entity, &transform, &body, collider);
            continue;
        }
        // An Inspector or game-code edit to either component reaches the live
        // body first, so the pose checks below see the body kind it now has.
        manager.apply_component_edits(entity, &body, collider);
        if body.body_kind == BodyKind::Dynamic {
            // A dynamic body owns its pose, unless someone moved the transform
            // since the last write-back.
            manager.follow_moved_transform(entity, &transform);
        } else if body.body_kind == BodyKind::KinematicPosition {
            // A kinematic body is placed by game code, so its transform is the
            // authority and the body follows it.
            manager.push_kinematic(entity, &transform);
        }
    }
    // A collider with no rigid body is static geometry (or a trigger zone):
    // it never moves on its own, and follows its transform when that moves.
    let mut live_static_colliders = HashSet::new();
    for (entity, collider, transform) in static_colliders.iter_mut() {
        live_static_colliders.insert(entity);
        if manager.has_static_collider(entity) {
            manager.update_static_collider(entity, transform, collider);
        } else {
            manager.spawn_static_collider(entity, transform, collider);
        }
    }
    manager.despawn_missing(&live_bodies, &live_static_colliders);

    // Step 2: advance the simulation by this frame's time, then publish what
    // touched what. The collection is replaced rather than appended to, so a
    // reader always sees the frame that just ran.
    manager.step(time.delta_seconds());
    let frame_events = manager.drain_events();
    if let Some(mut events) = events.get_mut() {
        *events = frame_events;
    }

    // Step 3: copy the stepped poses and velocities into the components of the
    // bodies the simulation moved. A fixed or kinematic body is left where it
    // is.
    for (entity, mut body, _collider, mut transform) in bodies.iter_mut() {
        if body.body_kind == BodyKind::Dynamic {
            manager.write_back(entity, &mut transform);
            manager.write_back_velocities(entity, &mut body);
        }
    }
    Ok(())
}
