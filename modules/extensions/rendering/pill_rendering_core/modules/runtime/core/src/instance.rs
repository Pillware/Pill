//! The model matrix one instance draws with, as the rows a shader reads.
//!
//! # Responsibilities
//!
//! - Build a transform component's model matrix ([`model_rows`]).
//!
//! # Design
//!
//! The matrix is built once per instance on the processor, and a shader puts
//! the rows back into a matrix. Building it per vertex instead would repeat the
//! same work for every vertex of a mesh that is drawn many times, and sending
//! the rotation as Euler angles would cost three inverse trigonometric calls
//! per instance.
//!
//! Only the top three rows are produced. The fourth row of an affine matrix is
//! always `0, 0, 0, 1`, so a shader supplies it itself.

// External crates
use glam::{Quat, Vec3};
use pill_rendering::components::TransformComponent;

// =============================================================================
// Free Functions
// =============================================================================

/// Builds one instance's model matrix from a transform component: the scale is
/// applied first, then the rotation, then the translation.
///
/// Row `i` of the result holds the `i`th value of each of the three scaled
/// rotation axes, then the `i`th value of the translation. That is the matrix
/// laid out the way a vertex shader reads a matrix passed as vertex
/// attributes, which is how both renderers hand it over.
///
/// A rotation that is not finite or has no length is ignored, so the instance
/// draws unrotated rather than with a degenerate matrix.
pub fn model_rows(transform_component: &TransformComponent) -> [[f32; 4]; 3] {
    // Step 1: Read the rotation as a unit quaternion and build the whole
    // affine matrix from the component's scale, rotation and translation.
    let rotation = Quat::from_array(pill_engine::common_components::rotation_or_identity(
        transform_component.rotation,
    ));
    let model = glam::Affine3A::from_scale_rotation_translation(
        Vec3::from_array(transform_component.scale),
        rotation,
        Vec3::from_array(transform_component.translation),
    );
    // Step 2: Copy the matrix out by row. glam stores it by column, and a
    // shader wants rows.
    let axes = [
        model.matrix3.x_axis,
        model.matrix3.y_axis,
        model.matrix3.z_axis,
    ];
    let translation = model.translation;
    let row = |index: usize| {
        [
            axes[0][index],
            axes[1][index],
            axes[2][index],
            translation[index],
        ]
    };
    [row(0), row(1), row(2)]
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The world position a shader computes for a model-space `point`: each of
    /// the three rows dotted with `(point, 1)`. That is what a shader's
    /// `mul(model, v)` does once the fourth row is `0, 0, 0, 1`.
    fn shader_transform(rows: [[f32; 4]; 3], point: Vec3) -> Vec3 {
        let homogeneous = point.extend(1.0);
        let row = |index: usize| glam::Vec4::from_array(rows[index]).dot(homogeneous);
        Vec3::new(row(0), row(1), row(2))
    }

    fn largest_difference(left: Vec3, right: Vec3) -> f32 {
        (left - right).abs().max_element()
    }

    fn transform(translation: [f32; 3], rotation: [f32; 4], scale: [f32; 3]) -> TransformComponent {
        TransformComponent {
            translation,
            rotation,
            scale,
        }
    }

    #[test]
    fn the_shader_applies_the_rotation_the_component_holds() {
        for rotation in [
            Quat::IDENTITY,
            Quat::from_rotation_x(0.4),
            Quat::from_rotation_y(2.0),
            Quat::from_rotation_z(-1.2),
            Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.3),
            // Past half a turn: the quaternion's scalar part is negative. The
            // matrix comes straight from the quaternion, never from an angle
            // wrapped into a shorter range.
            Quat::from_rotation_y(4.5),
        ] {
            let rows = model_rows(&transform([0.0; 3], rotation.to_array(), [1.0; 3]));
            for point in [Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(0.3, -1.2, 2.5)] {
                let difference =
                    largest_difference(shader_transform(rows, point), rotation * point);
                assert!(
                    difference < 1e-5,
                    "the shader's point differs by {difference} for {rotation:?}"
                );
            }
        }
    }

    #[test]
    fn scale_applies_before_rotation_and_translation_after() {
        let rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let rows = model_rows(&transform(
            [1.0, 2.0, 3.0],
            rotation.to_array(),
            [4.0, 5.0, 6.0],
        ));

        // +X scaled by 4, turned a quarter about Z onto +Y, then moved.
        let moved = shader_transform(rows, Vec3::X);
        assert!(
            largest_difference(moved, Vec3::new(1.0, 6.0, 3.0)) < 1e-5,
            "{moved:?}"
        );
    }

    #[test]
    fn a_broken_rotation_draws_unrotated() {
        let rows = model_rows(&transform([0.0; 3], [0.0; 4], [1.0; 3]));

        assert_eq!(shader_transform(rows, Vec3::X), Vec3::X);
    }
}
