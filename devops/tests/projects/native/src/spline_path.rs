//! Keeps the spline and its sample meshs aligned with the balls.
//!
//! # Responsibilities
//!
//! - Rebuild the spline's control points from the ball centres every frame.
//! - Walk the sample dots along the curve that results.
//! - Tolerate centres that are one frame old, because the physics system may
//!   run either side of this one within a frame.

use crate::settings::*;
use crate::{PhysicsState, SplineSample};
use pill_core::math::Vector3f;
use pill_engine::*;
use pill_master_renderer_data::TransformComponent;
use pill_spline::Spline;

/// Rebuilds the spline from the ball centres and walks the sample dots along
/// the curve that results.
///
/// `ball_physics` touches the same components in the same frame, and the
/// scheduler is free to batch it either side of this system. A centre can
/// therefore be one frame old by the time it becomes a control point, which is
/// invisible at 60 Hz and keeps the spline out of the physics step.
#[system(name = "spline_path", order = 530)]
pub(crate) fn spline_path_system(
    mut balls: Query<&PhysicsState>,
    mut splines: Query<&mut Spline>,
    mut samples: Query<(&SplineSample, &mut TransformComponent)>,
) -> Result<(), SystemError> {
    // Step 1: collect the ball centres in the order the control points take.
    // Iteration walks the ball archetype row by row and the balls are spawned
    // in index order, so the i-th centre seen belongs to the i-th ball.
    let mut control_points = [Vector3f::ZERO; BALL_COUNT];
    let mut control_point_count = 0;
    for physics in balls.iter_mut() {
        if control_point_count == BALL_COUNT {
            break;
        }
        control_points[control_point_count] =
            Vector3f::new(physics.position_x, physics.position_y, 0.0);
        control_point_count += 1;
    }

    // Step 2: publish the points, then place the dots on the curve they
    // describe. The spline keeps its own copy of the centres, so the balls
    // need no relationship to it and stay free to keep moving.
    for mut spline in splines.iter_mut() {
        spline.control_points[..control_point_count]
            .copy_from_slice(&control_points[..control_point_count]);
        spline.control_point_count = control_point_count as u32;

        for (sample, mut transform) in samples.iter_mut() {
            let location = spline.get_location_at(sample.t);
            // The dot is centred on the sample, and the transform places it.
            transform.translation = [
                (location.x - 400.0) / 80.0,
                (300.0 - location.y) / 80.0,
                0.0,
            ];
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dots only draw the whole curve while the last sample reaches
    /// `t = 1.0`; a step that does not divide 1.0 leaves the tail unmarked.
    #[test]
    fn sample_grid_reaches_both_curve_endpoints() {
        let sample_parameters: Vec<f32> = (0..SPLINE_SAMPLE_COUNT)
            .map(|index| index as f32 * SPLINE_SAMPLE_STEP)
            .collect();

        assert_eq!(sample_parameters.first(), Some(&0.0));
        assert_eq!(sample_parameters.last(), Some(&1.0));
    }
}
