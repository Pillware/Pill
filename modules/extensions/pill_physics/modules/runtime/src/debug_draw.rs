//! Draws every collider as an outline, for looking at what the simulation sees.
//!
//! # Responsibilities
//!
//! - Draws the shape of each collider through the engine's gizmo pass, at the
//!   pose the entity's transform holds.
//! - Draws nothing at all unless the settings document turns outlines on, so
//!   the one switch works in every posture: an editor, a development host and a
//!   shipped build that turned it on.
//!
//! # Design
//!
//! The outline is a component gizmo: [`ColliderComponent`] declares it with
//! `#[pill(gizmo = ...)]`, and the engine draws it for every entity that
//! carries the component. An editor hides the type from its preferences; a
//! development host draws it on its game surface. Nothing here registers a
//! system, so a build that installs no gizmo pass pays nothing for the module.

// External crates
use pill_engine::{GizmoContext, GizmoDraw, TransformComponent};
use rapier3d::math::{Rotation, Vector};

// Current crate
use crate::components::{ColliderComponent, ColliderShape};
use crate::manager::PhysicsManager;

/// The green every collider outline is drawn in.
const OUTLINE_COLOR: [f32; 4] = [0.35, 0.95, 0.55, 1.0];

// =============================================================================
// The gizmo
// =============================================================================

/// Draws the outline of one collider while the settings document asks for it.
///
/// The engine's gizmo pass calls this once per entity that carries a
/// [`ColliderComponent`]; the shape is drawn at the collider's offset, turned
/// with the entity.
pub fn draw_collider_outline(
    collider: &ColliderComponent,
    context: &GizmoContext<'_>,
    draw: &mut GizmoDraw,
) {
    // Step 1: the settings document decides, and it is the one switch in every
    // posture. A world without a manager draws nothing, because the switch is
    // unknown rather than on.
    let Some(manager) = context.get_resource::<PhysicsManager>() else {
        return;
    };
    if !manager.debug_draw_enabled() {
        return;
    }
    // Step 2: an entity without a transform has no place to draw at.
    let Some(transform) = context.transform() else {
        return;
    };
    let placed = TransformComponent {
        translation: crate::manager::collider_center(transform, collider),
        ..*transform
    };
    let [first, second, third] = collider.dimensions;
    match collider.shape {
        ColliderShape::Ball => {
            draw.sphere(placed.translation, first, OUTLINE_COLOR);
        }
        ColliderShape::Cuboid => {
            draw_box(draw, &placed, [first, second, third]);
        }
        ColliderShape::Capsule | ColliderShape::Cylinder => {
            let rounded_ends = collider.shape == ColliderShape::Capsule;
            draw_upright_shape(draw, &placed, first, second, rounded_ends);
        }
    }
}

// =============================================================================
// Box outlines
// =============================================================================

/// Draws the twelve edges of a box that is offset by the entity's pose.
pub fn draw_box(draw: &mut GizmoDraw, transform: &TransformComponent, half_extents: [f32; 3]) {
    draw.box_outline(
        transform.translation,
        transform.rotation,
        half_extents,
        OUTLINE_COLOR,
    );
}

// =============================================================================
// Capsule and cylinder outlines
// =============================================================================

/// How many straight lines approximate one half circle of a capsule's cap.
const CAP_ARC_SEGMENTS: usize = 12;

/// Draws a capsule or a cylinder standing on the entity's own y axis: a ring
/// at each end of the shaft and four lines joining them. With `rounded_ends`
/// each end also gets two half-circle arcs, at right angles, that outline the
/// capsule's hemispherical cap.
pub fn draw_upright_shape(
    draw: &mut GizmoDraw,
    transform: &TransformComponent,
    half_height: f32,
    radius: f32,
    rounded_ends: bool,
) {
    let rotation = rotation_of(transform);
    let center = transform.translation;
    // Step 1: The shape's own axes, turned into world space, give every point
    // below as an offset from the entity's position.
    let axis = |x: f32, y: f32, z: f32| {
        let turned = rotation.mul_vec3(Vector::new(x, y, z));
        [turned.x, turned.y, turned.z]
    };
    let up = axis(0.0, 1.0, 0.0);
    let side_axes = [axis(1.0, 0.0, 0.0), axis(0.0, 0.0, 1.0)];
    let point_at = |offsets: &[([f32; 3], f32)]| {
        let mut point = center;
        for (direction, distance) in offsets {
            for component in 0..3 {
                point[component] += direction[component] * distance;
            }
        }
        point
    };
    // Step 2: A ring at each end of the shaft, and the four side lines.
    for end_sign in [-1.0_f32, 1.0] {
        draw.circle(
            point_at(&[(up, end_sign * half_height)]),
            up,
            radius,
            OUTLINE_COLOR,
        );
    }
    for side in side_axes {
        for side_sign in [-1.0_f32, 1.0] {
            let bottom = point_at(&[(up, -half_height), (side, side_sign * radius)]);
            let top = point_at(&[(up, half_height), (side, side_sign * radius)]);
            draw.line(bottom, top, OUTLINE_COLOR);
        }
    }
    if !rounded_ends {
        return;
    }
    // Step 3: Each cap is a half circle in each of the two upright planes,
    // running from one side of the ring over the pole to the other.
    for end_sign in [-1.0_f32, 1.0] {
        for side in side_axes {
            let arc_point = |index: usize| {
                let angle = index as f32 / CAP_ARC_SEGMENTS as f32 * std::f32::consts::PI;
                let (sine, cosine) = angle.sin_cos();
                point_at(&[
                    (up, end_sign * (half_height + radius * sine)),
                    (side, radius * cosine),
                ])
            };
            for index in 0..CAP_ARC_SEGMENTS {
                draw.line(arc_point(index), arc_point(index + 1), OUTLINE_COLOR);
            }
        }
    }
}

/// The entity's rotation, treating a value that is not usable as the identity.
pub fn rotation_of(transform: &TransformComponent) -> Rotation {
    let [x, y, z, w] = transform.rotation;
    let length_squared = x * x + y * y + z * z + w * w;
    if !length_squared.is_finite() || length_squared < f32::EPSILON {
        return Rotation::IDENTITY;
    }
    Rotation::from_xyzw(x, y, z, w).normalize()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::{GizmoFrame, World};

    /// Draws one cuboid's outline in a world whose settings switch is set, and
    /// answers how many segments the gizmo produced.
    fn outline_segments(debug_draw_enabled: bool) -> usize {
        let mut world = World::new();
        world.register_component::<TransformComponent>();
        crate::components::register(&mut world);
        let mut manager = PhysicsManager::new();
        manager.set_debug_draw(debug_draw_enabled);
        world.insert_resource(manager);
        let entity = world
            .create_entity()
            .with(TransformComponent {
                translation: [0.0, 1.0, 0.0],
                ..TransformComponent::default()
            })
            .with(ColliderComponent {
                shape: ColliderShape::Cuboid,
                dimensions: [1.0, 1.0, 1.0],
                ..ColliderComponent::default()
            })
            .build()
            .expect("the entity builds");
        let collider = *world
            .get_component::<ColliderComponent>(entity)
            .expect("the collider is there");
        let frame = GizmoFrame::default();
        let context = frame.context(&world, entity);
        let mut draw = GizmoDraw::new();
        draw_collider_outline(&collider, &context, &mut draw);
        draw.segment_count()
    }

    /// The outline is a gizmo, and the settings document is the switch that
    /// turns it on: the same function draws twelve edges or nothing.
    #[test]
    fn the_outline_follows_the_settings_switch() {
        assert_eq!(
            outline_segments(true),
            12,
            "a cuboid outline is twelve edges"
        );
        assert_eq!(
            outline_segments(false),
            0,
            "the settings switch stops the outline"
        );
    }
}
