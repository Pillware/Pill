//! Drawing the frame's debug geometry into the panel: lines and labels.
//!
//! # Responsibilities
//!
//! - Draw the frame's debug line segments as screen-facing quads, projected and
//!   clipped at the camera's near side by the shared projection code
//!   ([`render`]).
//! - Draw the frame's labels in the shared bitmap font, at the style the label
//!   asks for.
//! - Honour each drawing's depth: hidden by the scene, over it, or showing
//!   through it in the muted look.
//!
//! # Design
//!
//! The projection is the same code the GPU renderer's overlay uses
//! (`pill_rendering_core::projection`), so a debug line lands on the same pixels
//! on both backends. A line becomes two triangles whose screen corners carry the
//! segment's depth, which is what lets the depth buffer hide it behind scene
//! geometry.
//!
//! The glyphs come from the font both renderers share
//! (`pill_rendering_core::debug_font`) and are drawn straight into the
//! framebuffer, so the label path needs no texture atlas while the UI path keeps
//! its images.
//!
//! Gizmos, selection outlines, the scene grid and picking belong to the editor,
//! which always uses the GPU renderer, so nothing here draws them.

// External crates
use pill_core::math::Matrix4f;
use pill_renderer_api::RenderViewport;
use pill_renderer_api::{DebugDepth, DebugFrame, DebugLabel, LabelAlign};
use pill_rendering_core::debug_font::{glyph_rows, GLYPH_ROWS, GLYPH_WIDTH};
use pill_rendering_core::projection::{label_depth, label_origin, line_quad, to_pixels};

// Current crate
use crate::framebuffer::Framebuffer;

// =============================================================================
// Constants
// =============================================================================

/// How far each side of a line quad is moved from the segment, in pixels. A
/// line is therefore one and a half pixels wide, which reads as one pixel plus
/// a soft edge on a small panel.
const LINE_HALF_WIDTH: f32 = 0.75;

/// How many panel pixels one font pixel covers. The panel draws the shared font
/// at its own size, so one glyph pixel is one panel pixel.
const GLYPH_SCALE: u32 = 1;

/// The width of one character cell in font pixels: the glyph and one column of
/// spacing.
const GLYPH_ADVANCE: u32 = GLYPH_WIDTH + 1;

// =============================================================================
// Free Functions
// =============================================================================

/// Draws the frame's debug lines and labels into `framebuffer`.
///
/// `depth` is the scene's depth buffer, one entry per panel pixel, or an empty
/// slice when there is none; a drawing that asks for a depth test against an
/// empty buffer is drawn. Returns whether anything reached the framebuffer.
pub fn render(
    framebuffer: &mut Framebuffer,
    frame: &DebugFrame,
    view_projection: &Matrix4f,
    viewport: RenderViewport,
    depth: &[u16],
) -> bool {
    if viewport.width == 0 || viewport.height == 0 {
        return false;
    }
    let mut drew = false;
    for line in frame.visible_lines(false) {
        drew |= draw_line(framebuffer, line, view_projection, viewport, depth);
    }
    for label in frame.visible_labels(false) {
        drew |= draw_label(framebuffer, label, view_projection, viewport, depth);
    }
    drew
}

// =============================================================================
// Lines
// =============================================================================

/// Draws one line segment, or nothing when it is behind the camera.
fn draw_line(
    framebuffer: &mut Framebuffer,
    line: &pill_renderer_api::DebugLine,
    view_projection: &Matrix4f,
    viewport: RenderViewport,
    depth: &[u16],
) -> bool {
    let viewport_size = (viewport.width as f32, viewport.height as f32);
    let Some(quad) = line_quad(
        line.start,
        line.end,
        view_projection,
        viewport_size,
        LINE_HALF_WIDTH,
    ) else {
        return false;
    };

    // Step 1: Turn the six clip-space corners into panel pixels. The shared
    // projection counts pixels from the viewport's centre with y growing up;
    // the panel counts from its top-left corner with y growing down.
    let centre = [
        viewport.x as f32 + viewport.width as f32 * 0.5,
        viewport.y as f32 + viewport.height as f32 * 0.5,
    ];
    let corner = |clip: [f32; 4]| {
        let (x, y) = to_pixels(clip, viewport_size);
        (
            centre[0] + x,
            centre[1] - y,
            (clip[2] / clip[3]).clamp(0.0, 1.0),
        )
    };
    let corners = quad.map(corner);

    // Step 2: Fill the two triangles of the quad.
    let mut drew = false;
    for triangle in [[0, 1, 2], [3, 4, 5]] {
        let [first, second, third] = triangle.map(|index| corners[index]);
        drew |= fill_triangle(
            framebuffer,
            viewport,
            [first, second, third],
            line.color,
            line.depth,
            depth,
        );
    }
    drew
}

/// Fills one triangle with one blended colour, honouring `depth`.
///
/// A drawing that shows through geometry keeps its colour where it is in front
/// and takes the hidden look where it is behind: muted, and checkered when the
/// look asks for it, so the two are told apart without the drawing disappearing.
fn fill_triangle(
    framebuffer: &mut Framebuffer,
    viewport: RenderViewport,
    mut triangle: [(f32, f32, f32); 3],
    color: [f32; 4],
    mode: DebugDepth,
    depth: &[u16],
) -> bool {
    // Step 1: One winding for the three edge tests.
    let mut area = edge_function(triangle[0], triangle[1], triangle[2]);
    if !area.is_finite() || area == 0.0 {
        return false;
    }
    if area < 0.0 {
        triangle.swap(1, 2);
        area = -area;
    }
    let [first, second, third] = triangle;

    // Step 2: Walk the covered pixels inside the drawing area.
    let minimum_x = first
        .0
        .min(second.0)
        .min(third.0)
        .floor()
        .max(viewport.x as f32);
    let maximum_x = first
        .0
        .max(second.0)
        .max(third.0)
        .ceil()
        .min((viewport.x + viewport.width) as f32);
    let minimum_y = first
        .1
        .min(second.1)
        .min(third.1)
        .floor()
        .max(viewport.y as f32);
    let maximum_y = first
        .1
        .max(second.1)
        .max(third.1)
        .ceil()
        .min((viewport.y + viewport.height) as f32);

    let depth_test = matches!(mode, DebugDepth::Hidden | DebugDepth::Dimmed { .. })
        && depth.len() == framebuffer.width() as usize * framebuffer.height() as usize;
    let mut drew = false;
    for y in minimum_y as u32..maximum_y as u32 {
        let pixel_y = y as f32 + 0.5;
        for x in minimum_x as u32..maximum_x as u32 {
            let pixel_x = x as f32 + 0.5;
            let weight_first = edge_of(second, third, pixel_x, pixel_y) / area;
            let weight_second = edge_of(third, first, pixel_x, pixel_y) / area;
            let weight_third = edge_of(first, second, pixel_x, pixel_y) / area;
            if weight_first < 0.0 || weight_second < 0.0 || weight_third < 0.0 {
                continue;
            }
            let mut pixel_color = color;
            if depth_test {
                let line_depth =
                    weight_first * first.2 + weight_second * second.2 + weight_third * third.2;
                let line_depth = (line_depth * f32::from(u16::MAX)) as u16;
                let index = y as usize * framebuffer.width() as usize + x as usize;
                if line_depth > depth[index] {
                    // Geometry is in front of this pixel. A drawing that hides
                    // behind it stops here; a dimmed one goes on in its muted
                    // look, which is what tells a hidden part from a visible one.
                    let Some(hidden) = mode.hidden_look() else {
                        continue;
                    };
                    if hidden.checker && (x + y) % 2 == 1 {
                        continue;
                    }
                    let brightness = hidden.brightness.clamp(0.0, 1.0);
                    pixel_color = [
                        pixel_color[0] * brightness,
                        pixel_color[1] * brightness,
                        pixel_color[2] * brightness,
                        pixel_color[3] * brightness,
                    ];
                }
            }
            framebuffer.blend_pixel(x, y, pixel_color);
            drew = true;
        }
    }
    drew
}

/// Twice the signed area of the triangle the three corners form.
#[inline]
fn edge_function(first: (f32, f32, f32), second: (f32, f32, f32), third: (f32, f32, f32)) -> f32 {
    edge_of(first, second, third.0, third.1)
}

/// Twice the signed area of the triangle `from`, `to` and `(x, y)`.
#[inline]
fn edge_of(from: (f32, f32, f32), to: (f32, f32, f32), x: f32, y: f32) -> f32 {
    (to.0 - from.0) * (y - from.1) - (to.1 - from.1) * (x - from.0)
}

// =============================================================================
// Labels
// =============================================================================

/// Draws one label's text from its anchor, in the style it asks for.
///
/// The label's depth is tested at its anchor: a label whose anchor the scene
/// covers is dropped as a whole, or drawn in the muted look when it asks to
/// show through.
fn draw_label(
    framebuffer: &mut Framebuffer,
    label: &DebugLabel,
    view_projection: &Matrix4f,
    viewport: RenderViewport,
    depth: &[u16],
) -> bool {
    let Some((origin_x, origin_y)) = label_origin(label.anchor, view_projection, viewport) else {
        return false;
    };
    // Step 1: the anchor's depth, tested once for the whole label.
    let mut color = label.color;
    if let Some(anchor_depth) = label_depth(label.anchor, view_projection) {
        let index = origin_y as usize * framebuffer.width() as usize + origin_x as usize;
        let covered = depth.len() == framebuffer.width() as usize * framebuffer.height() as usize
            && depth[index] < (anchor_depth * f32::from(u16::MAX)) as u16;
        if covered {
            let Some(hidden) = label.depth.hidden_look() else {
                return false;
            };
            let brightness = hidden.brightness.clamp(0.0, 1.0);
            color = [
                color[0] * brightness,
                color[1] * brightness,
                color[2] * brightness,
                color[3] * brightness,
            ];
        }
    }

    // Step 2: the text's cell, placed around the anchor by its alignment, and
    // the plate behind it.
    let scale = label.scale.max(0.1);
    let cell = |value: u32| (value as f32 * scale) as u32;
    let columns = label.text.chars().count() as u32;
    let text_width = columns * cell(GLYPH_ADVANCE);
    let start_x = match label.align {
        LabelAlign::Left => origin_x,
        LabelAlign::Center => origin_x - text_width as f32 * 0.5,
        LabelAlign::Right => origin_x - text_width as f32,
    };
    let mut drew = false;
    if let Some(background) = label.background {
        drew |= fill_plate(
            framebuffer,
            start_x,
            origin_y,
            text_width,
            cell(GLYPH_ROWS as u32),
            background,
        );
    }

    // Step 3: the glyphs, one cell each.
    for (index, character) in label.text.chars().enumerate() {
        let Some(glyph) = glyph_rows(character) else {
            continue;
        };
        let cell_x = start_x + (index as u32 * cell(GLYPH_ADVANCE)) as f32;
        drew |= draw_glyph(framebuffer, glyph, cell_x, origin_y, color, scale);
    }
    drew
}

/// Fills one label's background plate and answers whether it drew anything.
fn fill_plate(
    framebuffer: &mut Framebuffer,
    origin_x: f32,
    origin_y: f32,
    width: u32,
    height: u32,
    color: [f32; 4],
) -> bool {
    let mut drew = false;
    for y in origin_y as u32..origin_y as u32 + height {
        for x in origin_x as u32..origin_x as u32 + width {
            framebuffer.blend_pixel(x, y, color);
            drew = true;
        }
    }
    drew
}

/// Draws one glyph's lit font pixels at `origin`, scaled up.
fn draw_glyph(
    framebuffer: &mut Framebuffer,
    glyph: [u8; GLYPH_ROWS],
    origin_x: f32,
    origin_y: f32,
    color: [f32; 4],
    scale: f32,
) -> bool {
    let mut drew = false;
    let step = (GLYPH_SCALE as f32 * scale).max(1.0) as u32;
    for (row, bits) in glyph.iter().enumerate() {
        for column in 0..GLYPH_WIDTH {
            if bits & (1 << (GLYPH_WIDTH - 1 - column)) == 0 {
                continue;
            }
            for offset_y in 0..step {
                for offset_x in 0..step {
                    let x = origin_x as u32 + column * step + offset_x;
                    let y = origin_y as u32 + row as u32 * step + offset_y;
                    framebuffer.blend_pixel(x, y, color);
                    drew = true;
                }
            }
        }
    }
    drew
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_renderer_api::CameraComponent;
    use pill_renderer_api::{DebugColor, DebugDepth, DebugLine, LabelAlign, LabelAnchor};
    use pill_rendering_core::camera::{projection_matrix, view_matrix};

    /// A camera three units in front of the origin, looking down `-Z`.
    fn view_projection() -> Matrix4f {
        let camera = CameraComponent::default();
        let transform = pill_engine::common_components::TransformComponent {
            translation: [0.0, 0.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        projection_matrix(camera.vertical_fov, camera.near, camera.far, 1.0)
            * view_matrix(&transform)
    }

    /// A debug line from `start` to `end` in world space.
    fn line(start: [f32; 3], end: [f32; 3], depth: DebugDepth) -> DebugLine {
        DebugLine {
            start,
            end,
            color: [1.0, 0.0, 0.0, 1.0],
            depth,
            editor_only: false,
        }
    }

    /// The pixels a segment from `-0.5` to `0.5` on X lights up at both ends.
    #[test]
    fn a_projected_line_draws_at_both_its_ends() {
        let mut framebuffer = Framebuffer::new(240, 240);
        let mut frame = DebugFrame::default();
        frame
            .lines
            .push(line([-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], DebugDepth::OnTop));

        let drew = render(
            &mut framebuffer,
            &frame,
            &view_projection(),
            RenderViewport::full(240, 240),
            &[],
        );

        assert!(drew);
        // The segment crosses the middle of the panel: its ends sit about
        // 35 pixels either side of the centre.
        let lit = |x: u32| framebuffer.pixel(x, 120) != Some(0x0000);
        assert!(lit(86), "the left end of the line is drawn");
        assert!(lit(154), "the right end of the line is drawn");
        assert_eq!(
            framebuffer.pixel(120, 40),
            Some(0x0000),
            "away from the line nothing is drawn"
        );
    }

    /// A line that asks to be depth-tested is hidden by scene geometry in front
    /// of it, and one that does not ask is drawn over everything.
    #[test]
    fn a_hidden_line_is_covered_by_the_scene() {
        for hidden in [true, false] {
            let depth = if hidden {
                DebugDepth::Hidden
            } else {
                DebugDepth::OnTop
            };
            let mut framebuffer = Framebuffer::new(240, 240);
            let mut frame = DebugFrame::default();
            frame
                .lines
                .push(line([-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], depth));
            // A scene depth of zero everywhere: geometry at the near plane, in
            // front of any debug line.
            let depth = vec![0u16; 240 * 240];

            render(
                &mut framebuffer,
                &frame,
                &view_projection(),
                RenderViewport::full(240, 240),
                &depth,
            );

            let drawn = framebuffer.pixel(120, 120) != Some(0x0000);
            assert_eq!(drawn, !hidden, "hidden={hidden}");
        }
    }

    /// A label anchored to the screen draws where it is anchored.
    #[test]
    fn a_screen_label_draws_at_its_anchor() {
        let mut framebuffer = Framebuffer::new(240, 240);
        let mut frame = DebugFrame::default();
        frame.labels.push(DebugLabel {
            anchor: LabelAnchor::Screen { x: 10.0, y: 20.0 },
            text: "AB".to_string(),
            color: [1.0, 1.0, 1.0, 1.0],
            depth: DebugDepth::OnTop,
            editor_only: false,
            scale: 1.0,
            align: LabelAlign::Left,
            background: None,
        });

        let drew = render(
            &mut framebuffer,
            &frame,
            &view_projection(),
            RenderViewport::full(240, 240),
            &[],
        );

        assert!(drew);
        // The first glyph starts at the anchor and covers a few pixels.
        let lit = (10..18).any(|x| (20..30).any(|y| framebuffer.pixel(x, y) != Some(0x0000)));
        assert!(lit, "the first character is drawn at the anchor");
        assert_eq!(framebuffer.pixel(200, 200), Some(0x0000));
    }

    /// A lowercase label is drawn in uppercase, so a project logging its state
    /// still gets readable text.
    #[test]
    fn a_lowercase_label_uses_its_uppercase_glyph() {
        assert_eq!(glyph_rows('a'), glyph_rows('A'));
        assert_eq!(glyph_rows('@'), None);
        assert_eq!(glyph_rows(' '), Some([0; GLYPH_ROWS]));
    }

    /// A line behind the camera draws nothing instead of a streak across the
    /// panel.
    #[test]
    fn a_line_behind_the_camera_draws_nothing() {
        let mut framebuffer = Framebuffer::new(240, 240);
        let mut frame = DebugFrame::default();
        frame
            .lines
            .push(line([0.0, 0.0, 10.0], [0.0, 0.0, 20.0], DebugDepth::OnTop));

        let drew = render(
            &mut framebuffer,
            &frame,
            &view_projection(),
            RenderViewport::full(240, 240),
            &[],
        );

        assert!(!drew);
        assert_eq!(framebuffer.pixel(120, 120), Some(0x0000));
    }

    /// A line's own colour is what reaches the panel, mixed with alpha.
    #[test]
    fn a_line_blends_its_colour_over_the_scene() {
        let mut framebuffer = Framebuffer::new(240, 240);
        let mut frame = DebugFrame::default();
        frame
            .lines
            .push(line([-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], DebugDepth::OnTop));

        render(
            &mut framebuffer,
            &frame,
            &view_projection(),
            RenderViewport::full(240, 240),
            &[],
        );

        let (red, green, _) = crate::framebuffer::unpack_rgb565(
            framebuffer
                .pixel(120, 120)
                .expect("the line crosses the centre"),
        );
        let _: DebugColor = [0.0; 4];
        assert!(red > 0.5 && green < 0.1, "the line is red ({red}, {green})");
    }
}
