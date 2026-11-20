//! Ray queries against the simulated world.
//!
//! # Responsibilities
//!
//! - Declares [`RayHit`], what a ray that reached something reports.
//! - Casts a ray through every simulated collider and answers with the closest
//!   hit, named as the entity that owns it.
//!
//! # Design
//!
//! The cast walks the simulated colliders and tests each shape in world space.
//! That is exact and linear in the number of bodies, which is the right trade
//! for picking and line-of-sight checks; a broad-phase walk is a follow-up for a
//! world with enough bodies to need it.

// External crates
use rapier3d::math::Vector;
use rapier3d::prelude::Ray;

// Current crate
pub use crate::manager::PhysicsManager;
use pill_engine::Entity;

// =============================================================================
// RayHit
// =============================================================================

/// The closest thing a ray reached.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    /// The entity whose collider the ray reached.
    pub entity: Entity,
    /// How far along the ray the hit is, in world units.
    pub distance: f32,
    /// Where the ray met the shape, in world space.
    pub point: [f32; 3],
}

// =============================================================================
// cast_ray
// =============================================================================

impl PhysicsManager {
    /// Casts a ray through the simulated colliders and answers with the closest
    /// hit within `max_distance`.
    ///
    /// The direction is normalized first, so its length never changes the
    /// answer. A direction of zero length, or a distance that is not positive,
    /// reaches nothing. The ray is solid: starting inside a shape counts as a
    /// hit at distance zero.
    pub fn cast_ray(
        &self,
        origin: [f32; 3],
        direction: [f32; 3],
        max_distance: f32,
    ) -> Option<RayHit> {
        if !max_distance.is_finite() || max_distance <= 0.0 {
            return None;
        }
        let direction_vector = Vector::new(direction[0], direction[1], direction[2]);
        if direction_vector.length_squared() <= f32::EPSILON {
            return None;
        }
        let ray = Ray::new(
            Vector::new(origin[0], origin[1], origin[2]),
            direction_vector.normalize(),
        );

        // Step 1: Test every simulated collider and keep the closest answer.
        let mut closest: Option<RayHit> = None;
        for (entity, collider_handle) in self.collider_owners() {
            let Some(collider) = self.collider_of(collider_handle) else {
                continue;
            };
            let Some(hit) =
                collider
                    .shape()
                    .cast_ray(collider.position(), &ray, max_distance, true)
            else {
                continue;
            };
            let is_closer = closest
                .map(|existing| hit < existing.distance)
                .unwrap_or(true);
            if !is_closer {
                continue;
            }
            let distance = hit;
            closest = Some(RayHit {
                entity,
                distance,
                point: [
                    origin[0] + ray.dir.x * distance,
                    origin[1] + ray.dir.y * distance,
                    origin[2] + ray.dir.z * distance,
                ],
            });
        }
        closest
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use crate::components::{BodyKind, ColliderComponent, ColliderShape, RigidBodyComponent};
    use crate::manager::PhysicsManager;
    use pill_engine::{TransformComponent, World};

    /// A transform at the given height with no rotation.
    fn transform_at(height: f32) -> TransformComponent {
        TransformComponent {
            translation: [0.0, height, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }

    #[test]
    fn a_ray_from_above_hits_the_floor_and_names_it() {
        let mut world = World::new();
        let mut manager = PhysicsManager::new();
        let floor = world.create_entity().build().expect("an entity");
        manager.spawn_body(
            floor,
            &transform_at(0.0),
            &RigidBodyComponent {
                body_kind: BodyKind::Fixed,
                ..RigidBodyComponent::default()
            },
            &ColliderComponent {
                shape: ColliderShape::Cuboid,
                dimensions: [5.0, 0.5, 5.0],
                ..ColliderComponent::default()
            },
        );

        let hit = manager.cast_ray([0.0, 4.0, 0.0], [0.0, -1.0, 0.0], 10.0);

        let hit = hit.expect("the ray should reach the floor");
        assert_eq!(hit.entity, floor);
        assert!(
            (hit.distance - 3.5).abs() < 0.01,
            "distance was {}",
            hit.distance
        );
        assert!((hit.point[1] - 0.5).abs() < 0.01);
    }

    #[test]
    fn a_ray_into_open_air_reaches_nothing() {
        let world = World::new();
        let manager = PhysicsManager::new();
        let _ = world;
        assert_eq!(manager.cast_ray([0.0; 3], [0.0, 1.0, 0.0], 10.0), None);
        assert_eq!(manager.cast_ray([0.0; 3], [0.0; 3], 10.0), None);
        assert_eq!(manager.cast_ray([0.0; 3], [0.0, 1.0, 0.0], 0.0), None);
    }
}
