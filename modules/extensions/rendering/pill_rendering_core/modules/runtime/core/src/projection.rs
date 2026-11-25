//! Projecting debug geometry to the screen, and clipping it at the camera's
//! near side.
//!
//! # Responsibilities
//!
//! - Turn a world-space segment into the six clip-space corners of a
//!   screen-space quad of a requested pixel width, or report that it is not
//!   visible ([`line_quad`]).
//! - Turn a world-space point into the screen-aligned quad of a requested pixel
//!   size ([`point_quad`]).
//! - Clip a projected segment or polygon at the camera's near side, so geometry
//!   behind the camera is left out instead of wrapping across the screen
//!   ([`clip_segment_to_near`], [`clip_polygon_to_near`]).
//! - Push a projected polygon's corners outwards, so its anti-aliasing band has
//!   room to fade ([`expand_polygon`]).
//! - Place a world-space label in the viewport ([`label_origin`]).
//!
//! # Design
//!
//! Everything is in clip space with a `w` of its own, so a corner that has been
//! widened on screen maps back to the same depth it came from: a quad stays
//! flat against the surface it belongs to, and the depth test keeps working.
//!
//! The width of a line and the size of a point are applied *after* projection,
//! in pixels, which is why a line keeps the same width at any distance.
//!
//! This is the processor side of debug drawing, shared by every renderer that
//! draws the engine's debug geometry. Building vertices for a particular
//! graphics stack stays in that renderer.

// =============================================================================
// Constants
// =============================================================================

/// The view-space distance from the camera below which a point counts as
/// behind it and is clipped away.
pub const NEAR_CLIP_DEPTH: f32 = 1e-3;

/// The smallest sine of a corner's half-angle [`expand_polygon`] accepts. At a
/// sharper corner the vertex would be pushed out without limit.
const MINIMUM_MITER_SINE: f32 = 0.02;

// =============================================================================
// Types
// =============================================================================

/// A projected polygon corner: its clip-space position, and the barycentric
/// corner it stands for.
///
/// Clipping a polygon adds corners on the cut, so each corner carries the
/// barycentric weight it should blend with once the shape is drawn.
pub type ClipVertex = ([f32; 4], [f32; 3]);

// =============================================================================
// Free Functions
// =============================================================================

/// The six clip-space corners of the quad covering `start`-`end`, widened by
/// `offset` pixels on each side. Returns `None` when the segment is behind the
/// camera or shorter than a pixel on screen.
pub fn line_quad(
    start: [f32; 3],
    end: [f32; 3],
    view_projection: &pill_core::math::Matrix4f,
    viewport_size: (f32, f32),
    offset: f32,
) -> Option<[[f32; 4]; 6]> {
    let project = |position: [f32; 3]| {
        (*view_projection * glam::Vec4::new(position[0], position[1], position[2], 1.0)).to_array()
    };
    let (start, end) = clip_segment_to_near(project(start), project(end))?;
    let (start_pixel, end_pixel) = (
        to_pixels(start, viewport_size),
        to_pixels(end, viewport_size),
    );
    let along = [end_pixel.0 - start_pixel.0, end_pixel.1 - start_pixel.1];
    let length = (along[0] * along[0] + along[1] * along[1]).sqrt();
    if length < 1.0 {
        return None;
    }
    let normal = [-along[1] / length * offset, along[0] / length * offset];
    let corners = |clip_position: [f32; 4]| {
        let pixel = to_pixels(clip_position, viewport_size);
        [1.0, -1.0].map(|side| {
            from_pixels(
                (pixel.0 + normal[0] * side, pixel.1 + normal[1] * side),
                viewport_size,
                clip_position[2],
                clip_position[3],
            )
        })
    };
    let [start_plus, start_minus] = corners(start);
    let [end_plus, end_minus] = corners(end);
    Some([
        start_plus,
        start_minus,
        end_plus,
        end_plus,
        start_minus,
        end_minus,
    ])
}

/// The six clip-space corners of a point's screen-aligned quad around its
/// projected centre, its half extent being `size_pixels`'s half plus
/// `extra_pixels`. `None` when the point is behind the camera.
pub fn point_quad(
    position: [f32; 3],
    size_pixels: f32,
    view_projection: &pill_core::math::Matrix4f,
    viewport_size: (f32, f32),
    extra_pixels: f32,
) -> Option<[[f32; 4]; 6]> {
    let clip =
        (*view_projection * glam::Vec4::new(position[0], position[1], position[2], 1.0)).to_array();
    if clip[3] < NEAR_CLIP_DEPTH {
        return None;
    }
    let centre = to_pixels(clip, viewport_size);
    let half = size_pixels.max(1.0) * 0.5 + extra_pixels;
    let corner = |x: f32, y: f32| {
        from_pixels(
            (centre.0 + x * half, centre.1 + y * half),
            viewport_size,
            clip[2],
            clip[3],
        )
    };
    Some([
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ])
}

/// A segment clipped to the camera's near side: `None` when both ends are
/// behind it, otherwise the part that crosses into view.
pub fn clip_segment_to_near(start: [f32; 4], end: [f32; 4]) -> Option<([f32; 4], [f32; 4])> {
    match (start[3] >= NEAR_CLIP_DEPTH, end[3] >= NEAR_CLIP_DEPTH) {
        (false, false) => None,
        (true, true) => Some((start, end)),
        (false, true) => Some((lerp4(start, end, near_side_crossing(start[3], end[3])), end)),
        (true, false) => Some((
            start,
            lerp4(end, start, near_side_crossing(end[3], start[3])),
        )),
    }
}

/// A polygon clipped to the camera's near side, every vertex keeping the
/// barycentric corner it stands for.
pub fn clip_polygon_to_near(polygon: &[ClipVertex]) -> Vec<ClipVertex> {
    let mut clipped = Vec::with_capacity(polygon.len() + 1);
    for (index, current) in polygon.iter().enumerate() {
        let next = polygon[(index + 1) % polygon.len()];
        let current_inside = current.0[3] >= NEAR_CLIP_DEPTH;
        if current_inside {
            clipped.push(*current);
        }
        if current_inside != (next.0[3] >= NEAR_CLIP_DEPTH) {
            let t = near_side_crossing(current.0[3], next.0[3]);
            clipped.push((lerp4(current.0, next.0, t), lerp3(current.1, next.1, t)));
        }
    }
    clipped
}

/// Moves every vertex of a projected polygon `offset` pixels away from its two
/// edges. A straight corner moves along the edge normal; a sharp corner moves
/// farther, so both edges clear the original shape by `offset` and the
/// feathered band has no gaps at the joins.
pub fn expand_polygon(
    polygon: &[ClipVertex],
    viewport_size: (f32, f32),
    offset: f32,
) -> Vec<ClipVertex> {
    let points: Vec<(f32, f32)> = polygon
        .iter()
        .map(|vertex| to_pixels(vertex.0, viewport_size))
        .collect();
    let count = points.len();
    // Step 1: Find which way the polygon winds, so every edge normal can point
    // outwards.
    let double_area: f32 = (0..count)
        .map(|index| {
            let current = points[index];
            let next = points[(index + 1) % count];
            current.0 * next.1 - next.0 * current.1
        })
        .sum();
    let orientation = if double_area < 0.0 { -1.0 } else { 1.0 };
    // Step 2: Move each corner outwards along the bisector of its two edges.
    let mut expanded = Vec::with_capacity(count);
    for index in 0..count {
        let current = points[index];
        let previous = points[(index + count - 1) % count];
        let next = points[(index + 1) % count];
        let incoming = unit_direction([current.0 - previous.0, current.1 - previous.1]);
        let outgoing = unit_direction([next.0 - current.0, next.1 - current.1]);
        let vertex = polygon[index];
        // A reflex corner of a clipped polygon sits on the cut, where nothing
        // needs widening, and a corner whose edges have no direction cannot be
        // widened at all. Both stay where they are.
        let moved = match (incoming, outgoing) {
            (Some(incoming), Some(outgoing)) => {
                let incoming_normal = [incoming[1] * orientation, -incoming[0] * orientation];
                let outgoing_normal = [outgoing[1] * orientation, -outgoing[0] * orientation];
                let turn = incoming[0] * outgoing[1] - incoming[1] * outgoing[0];
                let bisector = [
                    incoming_normal[0] + outgoing_normal[0],
                    incoming_normal[1] + outgoing_normal[1],
                ];
                match (turn * orientation > 0.0)
                    .then(|| unit_direction(bisector))
                    .flatten()
                {
                    Some(direction) => {
                        let along = (direction[0] * outgoing_normal[0]
                            + direction[1] * outgoing_normal[1])
                            .max(MINIMUM_MITER_SINE);
                        let reach = offset / along;
                        (
                            current.0 + direction[0] * reach,
                            current.1 + direction[1] * reach,
                        )
                    }
                    None => current,
                }
            }
            _ => current,
        };
        expanded.push((
            from_pixels(moved, viewport_size, vertex.0[2], vertex.0[3]),
            vertex.1,
        ));
    }
    expanded
}

/// Where a label's text starts, in surface pixels: a screen anchor as given, a
/// world anchor projected through the camera into the viewport. `None` for a
/// world point behind the camera.
pub fn label_origin(
    anchor: pill_renderer_api::LabelAnchor,
    view_projection: &pill_core::math::Matrix4f,
    viewport: pill_renderer_api::components::RenderViewport,
) -> Option<(f32, f32)> {
    match anchor {
        pill_renderer_api::LabelAnchor::Screen { x, y } => Some((x, y)),
        pill_renderer_api::LabelAnchor::World(position) => {
            let clip =
                *view_projection * glam::Vec4::new(position[0], position[1], position[2], 1.0);
            if clip.w <= f32::EPSILON {
                return None;
            }
            let normalized_x = clip.x / clip.w;
            let normalized_y = clip.y / clip.w;
            Some((
                viewport.x as f32 + (normalized_x * 0.5 + 0.5) * viewport.width as f32,
                viewport.y as f32 + (0.5 - normalized_y * 0.5) * viewport.height as f32,
            ))
        }
    }
}

/// How far along the way from a point at `start_depth` to one at `end_depth`
/// the camera's near side lies.
fn near_side_crossing(start_depth: f32, end_depth: f32) -> f32 {
    (NEAR_CLIP_DEPTH - start_depth) / (end_depth - start_depth)
}

/// Four components along the way from `start` to `end`.
fn lerp4(start: [f32; 4], end: [f32; 4], t: f32) -> [f32; 4] {
    std::array::from_fn(|index| start[index] + (end[index] - start[index]) * t)
}

/// Three components along the way from `start` to `end`.
fn lerp3(start: [f32; 3], end: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|index| start[index] + (end[index] - start[index]) * t)
}

/// A clip-space position in pixels from the centre of the drawn region.
///
/// A renderer that builds a vertex per projected point needs this on its own as
/// well: it is the same conversion [`line_quad`] widens and [`point_quad`]
/// centres on.
pub fn to_pixels(clip_position: [f32; 4], viewport_size: (f32, f32)) -> (f32, f32) {
    let inverse = 1.0 / clip_position[3];
    (
        clip_position[0] * inverse * 0.5 * viewport_size.0,
        clip_position[1] * inverse * 0.5 * viewport_size.1,
    )
}

/// The scene depth a label's anchor sits at, `0` at the near plane and `1` at
/// the far one, or `None` when the label is anchored to the screen and so has
/// no place in the scene.
///
/// The depth test of a label happens here, at its anchor rather than across its
/// text, so a label either shows as a whole or not at all.
pub fn label_depth(
    anchor: pill_renderer_api::LabelAnchor,
    view_projection: &pill_core::math::Matrix4f,
) -> Option<f32> {
    let pill_renderer_api::LabelAnchor::World(position) = anchor else {
        return None;
    };
    let clip = *view_projection * glam::Vec4::new(position[0], position[1], position[2], 1.0);
    (clip.w > f32::EPSILON).then(|| (clip.z / clip.w).clamp(0.0, 1.0))
}

/// The clip-space position a pixel position maps back to at `depth`.
fn from_pixels(point: (f32, f32), viewport_size: (f32, f32), depth: f32, w: f32) -> [f32; 4] {
    [
        point.0 * 2.0 / viewport_size.0 * w,
        point.1 * 2.0 / viewport_size.1 * w,
        depth,
        w,
    ]
}

/// Scales `direction` to unit length, or returns `None` when it is too short
/// to have one.
fn unit_direction(direction: [f32; 2]) -> Option<[f32; 2]> {
    let length = (direction[0] * direction[0] + direction[1] * direction[1]).sqrt();
    if length < 1e-6 {
        return None;
    }
    Some([direction[0] / length, direction[1] / length])
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_renderer_api::components::RenderViewport;
    use pill_renderer_api::LabelAnchor;

    /// Looking down `-Z` from the origin, with a 1:1 viewport.
    fn projection() -> pill_core::math::Matrix4f {
        glam::camera::rh::proj::directx::perspective(1.0, 1.0, 0.1, 100.0)
    }

    fn clip_of(view_projection: &pill_core::math::Matrix4f, position: [f32; 3]) -> [f32; 4] {
        (*view_projection * glam::Vec4::new(position[0], position[1], position[2], 1.0)).to_array()
    }

    // =========================================================================
    // Culling
    // =========================================================================

    /// A segment with both ends behind the camera is left out entirely.
    #[test]
    fn a_segment_behind_the_camera_is_culled() {
        let projection = projection();
        let start = clip_of(&projection, [0.0, 0.0, 5.0]);
        let end = clip_of(&projection, [1.0, 0.0, 6.0]);

        assert!(clip_segment_to_near(start, end).is_none());
        assert!(line_quad(
            [0.0, 0.0, 5.0],
            [1.0, 0.0, 6.0],
            &projection,
            (100.0, 100.0),
            1.0
        )
        .is_none());
    }

    /// A segment that crosses the camera's near side keeps the part in front:
    /// the end behind is moved onto the near side, not dropped.
    #[test]
    fn a_segment_crossing_the_near_side_keeps_the_part_in_front() {
        let projection = projection();
        let (start, end) = clip_segment_to_near(
            clip_of(&projection, [0.0, 0.0, -5.0]),
            clip_of(&projection, [0.0, 0.0, 5.0]),
        )
        .expect("the segment starts in front of the camera");

        assert!((start[3] - 5.0).abs() < 1e-4, "{start:?}");
        assert!(
            end[3] >= NEAR_CLIP_DEPTH,
            "the clipped end sits on the near side"
        );
    }

    /// A polygon with one corner behind the camera gains a corner on the cut,
    /// and each new corner keeps the barycentric weights it interpolated
    /// between.
    #[test]
    fn a_polygon_is_clipped_at_the_near_side() {
        let projection = projection();
        let polygon: Vec<ClipVertex> = [
            ([0.0, 0.0, -5.0], [1.0, 0.0, 0.0]),
            ([1.0, 0.0, -5.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 5.0], [0.0, 0.0, 1.0]),
        ]
        .into_iter()
        .map(|(position, barycentric)| (clip_of(&projection, position), barycentric))
        .collect();

        let clipped = clip_polygon_to_near(&polygon);

        assert_eq!(clipped.len(), 4, "the corner behind adds one on the cut");
        assert!(
            clipped
                .iter()
                .all(|vertex| vertex.0[3] >= NEAR_CLIP_DEPTH - 1e-6),
            "every kept corner is on the camera's side of the cut"
        );
        let weights: f32 = clipped
            .iter()
            .map(|vertex| vertex.1.iter().sum::<f32>())
            .sum();
        assert!(weights > 0.0, "the barycentric corners came through");
    }

    // =========================================================================
    // Projection
    // =========================================================================

    /// A segment straight ahead becomes six corners around the middle of the
    /// viewport, widened by the requested number of pixels.
    #[test]
    fn a_segment_ahead_becomes_a_quad_around_the_middle_of_the_view() {
        let projection = projection();

        let quad = line_quad(
            [-1.0, 0.0, -5.0],
            [1.0, 0.0, -5.0],
            &projection,
            (100.0, 100.0),
            2.0,
        )
        .expect("the segment is in front of the camera");

        let middle = quad.iter().fold(0.0_f32, |widest, corner| {
            widest.max(corner[1].abs() / corner[3])
        });
        assert!(middle < 0.5, "the corners stay near the middle: {middle}");
        assert!(
            quad.iter()
                .any(|corner| (corner[1] / corner[3]).abs() > 0.0),
            "the quad has width"
        );
    }

    /// A point's quad is centred on its projection and square on screen.
    #[test]
    fn a_point_becomes_a_square_quad_around_its_projection() {
        let projection = projection();

        let quad = point_quad([0.0, 0.0, -5.0], 8.0, &projection, (100.0, 100.0), 0.0)
            .expect("the point is in front of the camera");

        let centre_x = quad.iter().map(|corner| corner[0] / corner[3]).sum::<f32>() / 6.0;
        let centre_y = quad.iter().map(|corner| corner[1] / corner[3]).sum::<f32>() / 6.0;
        assert!(centre_x.abs() < 1e-6 && centre_y.abs() < 1e-6, "{quad:?}");
        let width = quad.iter().fold(0.0_f32, |widest, corner| {
            widest.max((corner[0] / corner[3]).abs())
        });
        let height = quad.iter().fold(0.0_f32, |widest, corner| {
            widest.max((corner[1] / corner[3]).abs())
        });
        assert!((width - height).abs() < 1e-6, "the quad is square");
    }

    /// A world label lands inside the viewport's own rectangle, and one behind
    /// the camera is not placed at all.
    #[test]
    fn world_labels_project_into_the_viewport() {
        let projection = projection();
        let viewport = RenderViewport::new(100, 50, 400, 200);

        let (x, y) = label_origin(LabelAnchor::World([0.0, 0.0, -5.0]), &projection, viewport)
            .expect("in front of the camera");

        assert!((x - 300.0).abs() < 1e-3, "{x}");
        assert!((y - 150.0).abs() < 1e-3, "{y}");
        assert!(label_origin(LabelAnchor::World([0.0, 0.0, 5.0]), &projection, viewport).is_none());
        assert_eq!(
            label_origin(
                LabelAnchor::Screen { x: 4.0, y: 8.0 },
                &projection,
                viewport
            ),
            Some((4.0, 8.0))
        );
    }
}
