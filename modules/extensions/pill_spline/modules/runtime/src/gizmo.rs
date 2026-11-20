//! The spline's own drawing, called by the engine's gizmo pass.
//!
//! # Responsibilities
//!
//! - Draw one [`Spline`] per entity that carries it: the curve, its control
//!   points and, when the spline stores them, each point's two tangent arms.
//!
//! All of it is chrome: no game surface draws it and none of it is pickable.
//! Interaction belongs to the editor's spline tool, not to this module.

// External crates
use pill_core::math::Vector3f;
use pill_engine::*;

// Current crate
use crate::component::{Spline, TangentMode};

/// The colour the curve is drawn in.
const CURVE_COLOR: [f32; 4] = [0.95, 0.75, 0.2, 1.0];

/// The colour the control points are drawn in.
const POINT_COLOR: [f32; 4] = [0.95, 0.95, 0.95, 1.0];

/// The colour the stored tangents are drawn in.
const TANGENT_COLOR: [f32; 4] = [0.35, 0.75, 0.95, 1.0];

/// How wide a control point is drawn, in pixels.
const POINT_SIZE_PIXELS: f32 = 7.0;

/// How many straight lines one curve segment is drawn with.
const SEGMENTS_PER_CURVE_SEGMENT: usize = 16;

/// Draws one spline: the curve, its control points and, when the spline stores
/// them, the tangents each point leaves and arrives along.
///
/// The engine's gizmo pass calls this once per entity that carries a
/// [`Spline`]; the points are in the spline's own space, so the entity's
/// transform turns them into the place they are drawn.
pub fn draw_spline_gizmo(spline: &Spline, context: &GizmoContext<'_>, draw: &mut GizmoDraw) {
    // Step 1: a spline with fewer than two points is a point, not a path.
    let point_count = spline.control_points().len();
    if point_count < 2 {
        return;
    }
    let to_world_matrix = context.to_world();
    let to_world = |point: Vector3f| {
        [
            to_world_matrix[0][0] * point.x
                + to_world_matrix[0][1] * point.y
                + to_world_matrix[0][2] * point.z
                + to_world_matrix[0][3],
            to_world_matrix[1][0] * point.x
                + to_world_matrix[1][1] * point.y
                + to_world_matrix[1][2] * point.z
                + to_world_matrix[1][3],
            to_world_matrix[2][0] * point.x
                + to_world_matrix[2][1] * point.y
                + to_world_matrix[2][2] * point.z
                + to_world_matrix[2][3],
        ]
    };

    // Step 2: the curve, sampled densely enough that a segment reads as a
    // curve rather than as a chain of straight lines.
    let sample_count = spline.segment_count() * SEGMENTS_PER_CURVE_SEGMENT;
    let curve: Vec<[f32; 3]> = (0..=sample_count)
        .map(|step| to_world(spline.get_location_at(step as f32 / sample_count as f32)))
        .collect();
    draw.polyline(&curve, CURVE_COLOR);

    // Step 3: one handle per control point, so a reader sees what the curve
    // passes through.
    for point in spline.control_points() {
        draw.point(to_world(*point), POINT_SIZE_PIXELS, POINT_COLOR);
    }

    // Step 4: the stored tangents, as the arms a segment leaves and arrives
    // along. An auto spline derives them per sample, so there is nothing stored
    // to draw.
    if spline.tangent_mode() == TangentMode::Auto {
        return;
    }
    for (index, point) in spline.control_points().iter().enumerate() {
        let arms = [
            *point - spline.in_tangents[index],
            *point + spline.out_tangents[index],
        ];
        for arm in arms {
            draw.line(to_world(*point), to_world(arm), TANGENT_COLOR);
        }
    }
}
