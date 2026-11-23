//! Drawing the frame's debug geometry into the panel: lines and labels.
//!
//! # Responsibilities
//!
//! - Draw the frame's debug line segments as screen-facing quads, projected and
//!   clipped at the camera's near side by the shared projection code
//!   ([`render`]).
//! - Draw the frame's labels with a small bitmap font in uppercase.
//! - Hide a line the scene covers when the line asks to be depth-tested.
//!
//! # Design
//!
//! The projection is the same code the GPU renderer's overlay uses
//! (`pill_rendering_core::projection`), so a debug line lands on the same pixels
//! on both backends. A line becomes two triangles whose screen corners carry the
//! segment's depth, which is what lets the depth buffer hide it behind scene
//! geometry.
//!
//! The font is drawn here rather than sampled from a texture: a label is a
//! three-pixel-wide, five-pixel-tall bitmap per character, scaled up so it is
//! legible on a small panel. That keeps the label path free of a texture atlas
//! while the UI path keeps its images.
//!
//! Gizmos, selection outlines, the scene grid and picking belong to the editor,
//! which always uses the GPU renderer, so nothing here draws them.

// External crates
use pill_core::math::Matrix4f;
use pill_engine::{DebugFrame, DebugLabel};
use pill_renderer_api::RenderViewport;
use pill_rendering_core::projection::{label_origin, line_quad, to_pixels};

// Current crate
use crate::framebuffer::Framebuffer;

// =============================================================================
// Constants
// =============================================================================

/// How far each side of a line quad is moved from the segment, in pixels. A
/// line is therefore one and a half pixels wide, which reads as one pixel plus
/// a soft edge on a small panel.
const LINE_HALF_WIDTH: f32 = 0.75;

/// How many panel pixels one font pixel covers.
const GLYPH_SCALE: u32 = 2;

/// The width of one character cell in font pixels: three glyph columns and one
/// of spacing.
const GLYPH_ADVANCE: u32 = 4;

/// The height of one font row in font pixels.
const GLYPH_ROWS: u32 = 5;

// =============================================================================
// Free Functions
// =============================================================================

/// Draws the frame's debug lines and labels into `framebuffer`.
///
/// `depth` is the scene's depth buffer, one entry per panel pixel, or an empty
/// slice when there is none; a line that asks for a depth test against an empty
/// buffer is drawn. Returns whether anything reached the framebuffer.
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
    for line in &frame.lines {
        // A line that belongs to the editor's own chrome is drawn by an editor
        // view only, which this renderer never is.
        if line.chrome {
            continue;
        }
        drew |= draw_line(framebuffer, line, view_projection, viewport, depth);
    }
    for label in &frame.labels {
        drew |= draw_label(framebuffer, label, view_projection, viewport);
    }
    drew
}

// =============================================================================
// Lines
// =============================================================================

/// Draws one line segment, or nothing when it is behind the camera.
fn draw_line(
    framebuffer: &mut Framebuffer,
    line: &pill_engine::DebugLine,
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
            line.depth_test,
            depth,
        );
    }
    drew
}

/// Fills one triangle with one blended colour, honouring the depth test when
/// it is asked for.
fn fill_triangle(
    framebuffer: &mut Framebuffer,
    viewport: RenderViewport,
    mut triangle: [(f32, f32, f32); 3],
    color: [f32; 4],
    depth_test: bool,
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

    let depth_test =
        depth_test && depth.len() == framebuffer.width() as usize * framebuffer.height() as usize;
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
            if depth_test {
                let line_depth =
                    weight_first * first.2 + weight_second * second.2 + weight_third * third.2;
                let line_depth = (line_depth * f32::from(u16::MAX)) as u16;
                let index = y as usize * framebuffer.width() as usize + x as usize;
                if line_depth > depth[index] {
                    continue;
                }
            }
            framebuffer.blend_pixel(x, y, color);
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

/// Draws one label's text from its anchor, in uppercase.
fn draw_label(
    framebuffer: &mut Framebuffer,
    label: &DebugLabel,
    view_projection: &Matrix4f,
    viewport: RenderViewport,
) -> bool {
    let Some((origin_x, origin_y)) = label_origin(label.anchor, view_projection, viewport) else {
        return false;
    };
    // A screen anchor is given in panel pixels and a world anchor is projected
    // into the viewport, so both are panel pixels here: the panel is the
    // surface.
    let mut drew = false;
    for (index, character) in label.text.chars().enumerate() {
        let Some(glyph) = glyph(character) else {
            continue;
        };
        let cell_x = origin_x + (index as u32 * GLYPH_ADVANCE * GLYPH_SCALE) as f32;
        drew |= draw_glyph(framebuffer, glyph, cell_x, origin_y, label.color);
    }
    drew
}

/// Draws one glyph's lit font pixels at `origin`, scaled up.
fn draw_glyph(
    framebuffer: &mut Framebuffer,
    glyph: [u8; GLYPH_ROWS as usize],
    origin_x: f32,
    origin_y: f32,
    color: [f32; 4],
) -> bool {
    let mut drew = false;
    for (row, bits) in glyph.iter().enumerate() {
        for column in 0..3u32 {
            if bits & (1 << (2 - column)) == 0 {
                continue;
            }
            for offset_y in 0..GLYPH_SCALE {
                for offset_x in 0..GLYPH_SCALE {
                    let x = origin_x as u32 + column * GLYPH_SCALE + offset_x;
                    let y = origin_y as u32 + row as u32 * GLYPH_SCALE + offset_y;
                    framebuffer.blend_pixel(x, y, color);
                    drew = true;
                }
            }
        }
    }
    drew
}

/// The five rows of one character's glyph, or `None` for a character the font
/// has no glyph for (a space draws nothing).
///
/// Each row holds three bits, the leftmost column in bit 2. Characters are
/// drawn in uppercase; a lowercase letter uses its uppercase glyph.
fn glyph(character: char) -> Option<[u8; GLYPH_ROWS as usize]> {
    let uppercase = character.to_ascii_uppercase();
    let glyph = match uppercase {
        'A' => [0b010, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b011, 0b100, 0b100, 0b100, 0b011],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b011, 0b100, 0b101, 0b101, 0b011],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b010],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b110, 0b101, 0b101, 0b101, 0b101],
        'O' => [0b010, 0b101, 0b101, 0b101, 0b010],
        'P' => [0b110, 0b101, 0b110, 0b100, 0b100],
        'Q' => [0b010, 0b101, 0b101, 0b110, 0b011],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b011, 0b100, 0b010, 0b001, 0b110],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b011],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b110, 0b001, 0b010, 0b100, 0b111],
        '3' => [0b110, 0b001, 0b010, 0b001, 0b110],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b110, 0b001, 0b110],
        '6' => [0b011, 0b100, 0b110, 0b101, 0b010],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b010, 0b101, 0b010, 0b101, 0b010],
        '9' => [0b010, 0b101, 0b011, 0b001, 0b110],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        ':' => [0b000, 0b010, 0b000, 0b010, 0b000],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        ' ' => [0b000; GLYPH_ROWS as usize],
        _ => return None,
    };
    Some(glyph)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_engine::{DebugColor, DebugLine, LabelAnchor};
    use pill_renderer_api::CameraComponent;
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
    fn line(start: [f32; 3], end: [f32; 3], depth_test: bool) -> DebugLine {
        DebugLine {
            start,
            end,
            color: [1.0, 0.0, 0.0, 1.0],
            depth_test,
            chrome: false,
        }
    }

    /// The pixels a segment from `-0.5` to `0.5` on X lights up at both ends.
    #[test]
    fn a_projected_line_draws_at_both_its_ends() {
        let mut framebuffer = Framebuffer::new(240, 240);
        let mut frame = DebugFrame::default();
        frame
            .lines
            .push(line([-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], false));

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
    fn a_depth_tested_line_is_hidden_by_the_scene() {
        for depth_test in [true, false] {
            let mut framebuffer = Framebuffer::new(240, 240);
            let mut frame = DebugFrame::default();
            frame
                .lines
                .push(line([-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], depth_test));
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
            assert_eq!(drawn, !depth_test, "depth_test={depth_test}");
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
        assert_eq!(glyph('a'), glyph('A'));
        assert_eq!(glyph('?'), None);
        assert_eq!(glyph(' '), Some([0; GLYPH_ROWS as usize]));
    }

    /// A line behind the camera draws nothing instead of a streak across the
    /// panel.
    #[test]
    fn a_line_behind_the_camera_draws_nothing() {
        let mut framebuffer = Framebuffer::new(240, 240);
        let mut frame = DebugFrame::default();
        frame
            .lines
            .push(line([0.0, 0.0, 10.0], [0.0, 0.0, 20.0], false));

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
            .push(line([-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], false));

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
