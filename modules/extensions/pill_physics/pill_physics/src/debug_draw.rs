//! Draws every collider as an outline, for looking at what the simulation sees.
//!
//! # Responsibilities
//!
//! - Draws the shape of each simulated collider through the engine's
//!   [`DebugDraw`] collection, at the pose the entity's transform holds.
//! - Draws nothing at all unless the settings document turns outlines on.

// External crates
use pill_engine::{
    system, DebugDraw, DebugStyle, Query, Res, ResMut, SystemError, TransformComponent,
};
use rapier3d::math::{Rotation, Vector};

// Current crate
use crate::components::{ColliderComponent, ColliderShape};
use crate::manager::PhysicsManager;

/// The green every collider outline is drawn in.
const OUTLINE_COLOR: [f32; 4] = [0.35, 0.95, 0.55, 1.0];

// =============================================================================
// physics_debug_draw
// =============================================================================

/// Draws the outline of every collider while the settings document asks for it.
///
/// # Errors
///
/// Returns [`SystemError::MissingResource`] when the physics manager or the
/// engine's debug collection is absent, which only happens in a world this
/// module was not registered in.
#[system(name = "physics_debug_draw", order = 901)]
pub fn physics_debug_draw(
    manager: Res<PhysicsManager>,
    mut debug_draw: ResMut<DebugDraw>,
    mut colliders: Query<(&ColliderComponent, &TransformComponent)>,
) -> Result<(), SystemError> {
    let Some(manager) = manager.get() else {
        return Err(SystemError::MissingResource {
            name: String::from("PhysicsManager"),
        });
    };
    if !manager.debug_draw_enabled() {
        return Ok(());
    }
    let Some(mut debug_draw) = debug_draw.get_mut() else {
        return Err(SystemError::MissingResource {
            name: String::from("DebugDraw"),
        });
    };
    let style = DebugStyle::colored(OUTLINE_COLOR);
    for (collider, transform) in colliders.iter_mut() {
        let [first, second, third] = collider.dimensions;
        match collider.shape {
            ColliderShape::Ball => {
                debug_draw.sphere(transform.translation, first, style);
            }
            ColliderShape::Cuboid => {
                draw_box(&mut debug_draw, transform, [first, second, third], style);
            }
            ColliderShape::Capsule | ColliderShape::Cylinder => {
                let rounded_ends = collider.shape == ColliderShape::Capsule;
                draw_upright_shape(
                    &mut debug_draw,
                    transform,
                    first,
                    second,
                    rounded_ends,
                    style,
                );
            }
        }
    }
    Ok(())
}

// =============================================================================
// Box outlines
// =============================================================================

/// Draws the twelve edges of a box that is offset by the entity's pose.
pub fn draw_box(
    debug_draw: &mut DebugDraw,
    transform: &TransformComponent,
    half_extents: [f32; 3],
    style: DebugStyle,
) {
    let rotation = rotation_of(transform);
    let center = transform.translation;
    // Step 1: The eight corners, each rotated into world space.
    let mut corners = [[0.0_f32; 3]; 8];
    for (index, corner) in corners.iter_mut().enumerate() {
        let offset = [
            if index & 1 == 0 {
                -half_extents[0]
            } else {
                half_extents[0]
            },
            if index & 2 == 0 {
                -half_extents[1]
            } else {
                half_extents[1]
            },
            if index & 4 == 0 {
                -half_extents[2]
            } else {
                half_extents[2]
            },
        ];
        let turned = rotation.mul_vec3(Vector::new(offset[0], offset[1], offset[2]));
        *corner = [
            center[0] + turned.x,
            center[1] + turned.y,
            center[2] + turned.z,
        ];
    }
    // Step 2: A corner joins the three whose index differs in exactly one bit.
    for (index, corner) in corners.iter().enumerate() {
        for bit in [1, 2, 4] {
            let other = index ^ bit;
            if other < index {
                debug_draw.line(*corner, corners[other], style);
            }
        }
    }
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
    debug_draw: &mut DebugDraw,
    transform: &TransformComponent,
    half_height: f32,
    radius: f32,
    rounded_ends: bool,
    style: DebugStyle,
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
        debug_draw.circle(point_at(&[(up, end_sign * half_height)]), up, radius, style);
    }
    for side in side_axes {
        for side_sign in [-1.0_f32, 1.0] {
            let bottom = point_at(&[(up, -half_height), (side, side_sign * radius)]);
            let top = point_at(&[(up, half_height), (side, side_sign * radius)]);
            debug_draw.line(bottom, top, style);
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
                debug_draw.line(arc_point(index), arc_point(index + 1), style);
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
