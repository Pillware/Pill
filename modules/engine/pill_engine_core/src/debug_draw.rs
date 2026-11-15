//! Debug drawing that any system can use: lines, shapes and text labels, plus
//! the gizmo handles that an editor tool draws on top.
//!
//! # Responsibilities
//!
//! - Keep what systems ask to see, for one frame or for a set time, in
//!   [`DebugDraw`], a world resource: line segments, shapes turned into line
//!   segments, text labels placed in the world or on the screen, and gizmo
//!   lines and triangles that carry a handle id.
//! - Count down how long each item stays and return one frame of plain data
//!   for the renderer. [`DebugDraw::collect_frame`] builds a [`DebugFrame`].
//! - Turn everything off. While drawing is disabled, no drawing call records
//!   anything, so nothing reaches the renderer.
//!
//! # Design
//!
//! The resource is plain data in the engine core and names no renderer, so a
//! game, an extension or an editor crate can draw without depending on one.
//! Every renderer that implements the contract draws the same frame. Shapes
//! are line segments by the time they leave here, so a renderer only has to
//! draw a line, a triangle and a text label. Label text is owned (`String`)
//! rather than borrowed from the module that drew it, so a reload cannot
//! leave a label pointing into memory that was unloaded.

// Current crate
use crate::resource::Resource;

// =============================================================================
// Constants
// =============================================================================

/// Segments a circle is drawn with.
const CIRCLE_SEGMENTS: usize = 32;

// =============================================================================
// DebugColor
// =============================================================================

/// A colour in linear RGBA, with one value per channel in `0..=1`.
pub type DebugColor = [f32; 4];

// =============================================================================
// DebugLifetime
// =============================================================================

/// How long one drawn item stays.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DebugLifetime {
    /// Drawn in the next frame only, then dropped - the default.
    #[default]
    OneFrame,
    /// Drawn every frame for this many seconds.
    Seconds(f32),
}

// =============================================================================
// DebugStyle
// =============================================================================

/// How one drawn item looks and lasts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugStyle {
    /// Its colour.
    pub color: DebugColor,
    /// Whether scene geometry in front of it hides it (`true`), or it draws
    /// over everything (`false`).
    pub depth_test: bool,
    /// Whether this is the editor's own drawing - a drag guide, a constraint
    /// line - rather than the game's. An editor view draws it. A game surface
    /// does not.
    pub chrome: bool,
    /// How long it stays.
    pub lifetime: DebugLifetime,
}

impl DebugStyle {
    /// Returns the default style in `color`.
    pub fn colored(color: DebugColor) -> Self {
        Self {
            color,
            ..Self::default()
        }
    }
}

impl Default for DebugStyle {
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            depth_test: true,
            chrome: false,
            lifetime: DebugLifetime::OneFrame,
        }
    }
}

// =============================================================================
// LabelAnchor
// =============================================================================

/// Where a text label is placed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LabelAnchor {
    /// A world position, projected by each surface's camera.
    World([f32; 3]),
    /// Physical pixels from a surface's top-left corner.
    Screen {
        /// Pixels from the left edge.
        x: f32,
        /// Pixels from the top edge.
        y: f32,
    },
}

// =============================================================================
// DebugLine
// =============================================================================

/// One line segment of a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DebugLine {
    /// Where it starts, in world space.
    pub start: [f32; 3],
    /// Where it ends, in world space.
    pub end: [f32; 3],
    /// Its colour.
    pub color: DebugColor,
    /// Whether scene geometry hides it.
    pub depth_test: bool,
    /// Whether only an editor view draws it.
    pub chrome: bool,
}

// =============================================================================
// DebugLabel
// =============================================================================

/// One text label of a frame.
#[derive(Debug, Clone, PartialEq)]
pub struct DebugLabel {
    /// Where it is placed.
    pub anchor: LabelAnchor,
    /// What it says. The renderer's bitmap font draws it in uppercase and
    /// draws `?` for a character it does not have.
    pub text: String,
    /// Its colour.
    pub color: DebugColor,
}

// =============================================================================
// GizmoLine
// =============================================================================

/// One gizmo line, drawn on top of everything and pickable by its handle id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GizmoLine {
    /// Where it starts, in world space.
    pub start: [f32; 3],
    /// Where it ends, in world space.
    pub end: [f32; 3],
    /// Its colour.
    pub color: DebugColor,
    /// The id a pick of this line reports. It must fit in the low 31 bits,
    /// because a pick sets the top bit to mark a handle.
    pub handle: u32,
}

// =============================================================================
// GizmoTriangle
// =============================================================================

/// One gizmo triangle, drawn on top of everything and pickable by its handle
/// id.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GizmoTriangle {
    /// Its corners, in world space.
    pub corners: [[f32; 3]; 3],
    /// Its colour.
    pub color: DebugColor,
    /// The id a pick of this triangle reports. It must fit in the low 31 bits,
    /// because a pick sets the top bit to mark a handle.
    pub handle: u32,
}

// =============================================================================
// DebugFrame
// =============================================================================

/// Everything debug-drawn for one frame, in the form the renderer receives it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DebugFrame {
    /// Line segments, including the ones shapes were turned into.
    pub lines: Vec<DebugLine>,
    /// Text labels.
    pub labels: Vec<DebugLabel>,
    /// Gizmo handle lines.
    pub gizmo_lines: Vec<GizmoLine>,
    /// Gizmo handle triangles.
    pub gizmo_triangles: Vec<GizmoTriangle>,
}

impl DebugFrame {
    /// Whether there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
            && self.labels.is_empty()
            && self.gizmo_lines.is_empty()
            && self.gizmo_triangles.is_empty()
    }
}

// =============================================================================
// Timed
// =============================================================================

/// One item with the lifetime it has left.
#[derive(Debug, Clone)]
struct Timed<T> {
    item: T,
    lifetime: DebugLifetime,
}

// =============================================================================
// DebugDraw
// =============================================================================

/// The debug drawing of a world.
///
/// It is a world resource that systems draw into, and the renderer collects it
/// once a frame. A development host and an editor create it enabled. A
/// shipping or web build creates it disabled, unless the project's settings
/// turn it on (`debug_draw: true`). [`Self::set_enabled`] switches it at any
/// time.
#[derive(Debug, Clone)]
pub struct DebugDraw {
    enabled: bool,
    lines: Vec<Timed<DebugLine>>,
    labels: Vec<Timed<DebugLabel>>,
    gizmo_lines: Vec<GizmoLine>,
    gizmo_triangles: Vec<GizmoTriangle>,
}

impl DebugDraw {
    /// Creates an empty debug drawing in the requested state.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            lines: Vec::new(),
            labels: Vec::new(),
            gizmo_lines: Vec::new(),
            gizmo_triangles: Vec::new(),
        }
    }

    /// Whether drawing calls record anything.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Turns debug drawing on or off.
    ///
    /// Turning it off also forgets every item still waiting to be drawn.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.clear();
        }
    }

    /// Forgets every item still waiting to be drawn.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.labels.clear();
        self.gizmo_lines.clear();
        self.gizmo_triangles.clear();
    }

    /// Draws a line segment from `start` to `end`.
    pub fn line(&mut self, start: [f32; 3], end: [f32; 3], style: DebugStyle) -> &mut Self {
        if self.enabled {
            self.lines.push(Timed {
                item: DebugLine {
                    start,
                    end,
                    color: style.color,
                    depth_test: style.depth_test,
                    chrome: style.chrome,
                },
                lifetime: style.lifetime,
            });
        }
        self
    }

    /// Draws an axis-aligned box around `center`, `half_extents` from it on
    /// each axis.
    pub fn aabb(
        &mut self,
        center: [f32; 3],
        half_extents: [f32; 3],
        style: DebugStyle,
    ) -> &mut Self {
        if !self.enabled {
            return self;
        }
        let corner = |x: f32, y: f32, z: f32| {
            [
                center[0] + x * half_extents[0],
                center[1] + y * half_extents[1],
                center[2] + z * half_extents[2],
            ]
        };
        let corners = [
            corner(-1.0, -1.0, -1.0),
            corner(1.0, -1.0, -1.0),
            corner(1.0, 1.0, -1.0),
            corner(-1.0, 1.0, -1.0),
            corner(-1.0, -1.0, 1.0),
            corner(1.0, -1.0, 1.0),
            corner(1.0, 1.0, 1.0),
            corner(-1.0, 1.0, 1.0),
        ];
        // The near face, the far face, and the four edges joining them.
        for (from, to) in [
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0),
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 4),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            self.line(corners[from], corners[to], style);
        }
        self
    }

    /// Draws a circle around `center` in the plane whose normal is `normal`.
    pub fn circle(
        &mut self,
        center: [f32; 3],
        normal: [f32; 3],
        radius: f32,
        style: DebugStyle,
    ) -> &mut Self {
        if !self.enabled {
            return self;
        }
        let (axis_u, axis_v) = plane_axes(normal);
        let point = |index: usize| {
            let angle = index as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
            let (sin, cos) = angle.sin_cos();
            [
                center[0] + radius * (cos * axis_u[0] + sin * axis_v[0]),
                center[1] + radius * (cos * axis_u[1] + sin * axis_v[1]),
                center[2] + radius * (cos * axis_u[2] + sin * axis_v[2]),
            ]
        };
        for index in 0..CIRCLE_SEGMENTS {
            self.line(point(index), point(index + 1), style);
        }
        self
    }

    /// Draws a sphere around `center` as its three great circles.
    pub fn sphere(&mut self, center: [f32; 3], radius: f32, style: DebugStyle) -> &mut Self {
        for normal in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            self.circle(center, normal, radius, style);
        }
        self
    }

    /// Draws an arrow from `start` to `end`, with a head a fifth of its
    /// length.
    pub fn arrow(&mut self, start: [f32; 3], end: [f32; 3], style: DebugStyle) -> &mut Self {
        if !self.enabled {
            return self;
        }
        self.line(start, end, style);
        let direction = subtract_vectors(end, start);
        let shaft_length = vector_length(direction);
        if shaft_length <= f32::EPSILON {
            return self;
        }
        let forward = scale_vector(direction, 1.0 / shaft_length);
        let (side, up) = plane_axes(forward);
        let head_length = shaft_length * 0.2;
        let back = subtract_vectors(end, scale_vector(forward, head_length));
        for offset in [side, scale_vector(side, -1.0), up, scale_vector(up, -1.0)] {
            self.line(
                end,
                add_vectors(back, scale_vector(offset, head_length * 0.5)),
                style,
            );
        }
        self
    }

    /// Draws a text label at a world position.
    pub fn label_world(&mut self, position: [f32; 3], text: &str, style: DebugStyle) -> &mut Self {
        self.label(LabelAnchor::World(position), text, style)
    }

    /// Draws a text label at physical pixels from a surface's top-left corner.
    pub fn label_screen(&mut self, x: f32, y: f32, text: &str, style: DebugStyle) -> &mut Self {
        self.label(LabelAnchor::Screen { x, y }, text, style)
    }

    /// Draws a text label at `anchor`.
    pub fn label(&mut self, anchor: LabelAnchor, text: &str, style: DebugStyle) -> &mut Self {
        if self.enabled {
            self.labels.push(Timed {
                item: DebugLabel {
                    anchor,
                    text: text.to_owned(),
                    color: style.color,
                },
                lifetime: style.lifetime,
            });
        }
        self
    }

    /// Draws a gizmo line on top for the next frame, pickable by `handle`.
    pub fn gizmo_line(
        &mut self,
        start: [f32; 3],
        end: [f32; 3],
        color: DebugColor,
        handle: u32,
    ) -> &mut Self {
        if self.enabled {
            self.gizmo_lines.push(GizmoLine {
                start,
                end,
                color,
                handle,
            });
        }
        self
    }

    /// Draws a gizmo triangle on top for the next frame, pickable by `handle`.
    pub fn gizmo_triangle(
        &mut self,
        corners: [[f32; 3]; 3],
        color: DebugColor,
        handle: u32,
    ) -> &mut Self {
        if self.enabled {
            self.gizmo_triangles.push(GizmoTriangle {
                corners,
                color,
                handle,
            });
        }
        self
    }

    /// Returns the items to draw this frame and counts the lifetime of the
    /// others down by `delta_seconds`.
    ///
    /// Items that last one frame, and all gizmo geometry, are returned once
    /// and then dropped. Items with a timed lifetime are copied out and kept
    /// until their time runs out. A disabled drawing returns an empty frame.
    pub fn collect_frame(&mut self, delta_seconds: f32) -> DebugFrame {
        if !self.enabled {
            return DebugFrame::default();
        }
        // Step 1: build the frame from the items waiting to be drawn.
        let frame = DebugFrame {
            lines: self.lines.iter().map(|timed| timed.item).collect(),
            labels: self.labels.iter().map(|timed| timed.item.clone()).collect(),
            gizmo_lines: std::mem::take(&mut self.gizmo_lines),
            gizmo_triangles: std::mem::take(&mut self.gizmo_triangles),
        };
        // Step 2: age the items that stay, which drops the one-frame ones.
        age_timed_items(&mut self.lines, delta_seconds);
        age_timed_items(&mut self.labels, delta_seconds);
        frame
    }
}

impl Default for DebugDraw {
    /// Creates an enabled drawing, which is the state a development host
    /// starts with.
    fn default() -> Self {
        Self::new(true)
    }
}

impl Resource for DebugDraw {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Keep
    }

    fn shared_name() -> Option<&'static str> {
        Some("pill_engine::debug_draw::DebugDraw")
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Drops the items whose time ran out and counts the rest down.
fn age_timed_items<T>(timed_items: &mut Vec<Timed<T>>, delta_seconds: f32) {
    timed_items.retain_mut(|timed| match &mut timed.lifetime {
        DebugLifetime::OneFrame => false,
        DebugLifetime::Seconds(remaining) => {
            *remaining -= delta_seconds;
            *remaining > 0.0
        }
    });
}

/// Returns two unit-length axes that span the plane whose normal is `normal`.
fn plane_axes(normal: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let normal_length = vector_length(normal);
    let normal = if normal_length > f32::EPSILON {
        scale_vector(normal, 1.0 / normal_length)
    } else {
        [0.0, 1.0, 0.0]
    };
    // The reference axis must not be parallel to the normal, or the cross
    // product would be zero. X works unless the normal points mostly along X.
    let reference_axis = if normal[0].abs() < 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let axis_u = normalize_vector(cross_vectors(normal, reference_axis));
    let axis_v = cross_vectors(normal, axis_u);
    (axis_u, axis_v)
}

fn add_vectors(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

fn subtract_vectors(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn scale_vector(vector: [f32; 3], factor: f32) -> [f32; 3] {
    [vector[0] * factor, vector[1] * factor, vector[2] * factor]
}

fn cross_vectors(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn vector_length(vector: [f32; 3]) -> f32 {
    (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt()
}

fn normalize_vector(vector: [f32; 3]) -> [f32; 3] {
    let length = vector_length(vector);
    if length > f32::EPSILON {
        scale_vector(vector, 1.0 / length)
    } else {
        vector
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A disabled drawing records nothing and returns an empty frame.
    #[test]
    fn a_disabled_drawing_records_nothing() {
        let mut draw = DebugDraw::new(false);
        draw.line([0.0; 3], [1.0; 3], DebugStyle::default())
            .label_world([0.0; 3], "x", DebugStyle::default())
            .gizmo_line([0.0; 3], [1.0; 3], [1.0; 4], 1);
        assert!(draw.collect_frame(0.016).is_empty());
    }

    /// One-frame items are drawn once, timed ones until their time runs out.
    #[test]
    fn items_live_as_long_as_their_lifetime() {
        let mut draw = DebugDraw::new(true);
        draw.line([0.0; 3], [1.0; 3], DebugStyle::default());
        draw.line(
            [0.0; 3],
            [2.0; 3],
            DebugStyle {
                lifetime: DebugLifetime::Seconds(0.05),
                ..DebugStyle::default()
            },
        );
        assert_eq!(draw.collect_frame(0.02).lines.len(), 2);
        assert_eq!(draw.collect_frame(0.02).lines.len(), 1);
        assert_eq!(draw.collect_frame(0.02).lines.len(), 1);
        assert!(draw.collect_frame(0.02).lines.is_empty());
    }

    /// The shapes come out as line segments. A box is twelve edges and a
    /// sphere is three circles.
    #[test]
    fn shapes_are_tessellated_into_lines() {
        let mut draw = DebugDraw::new(true);
        draw.aabb([0.0; 3], [1.0; 3], DebugStyle::default());
        assert_eq!(draw.collect_frame(0.0).lines.len(), 12);
        draw.sphere([0.0; 3], 1.0, DebugStyle::default());
        assert_eq!(draw.collect_frame(0.0).lines.len(), 3 * CIRCLE_SEGMENTS);
        draw.arrow([0.0; 3], [0.0, 0.0, 1.0], DebugStyle::default());
        assert_eq!(draw.collect_frame(0.0).lines.len(), 5);
    }

    /// Every point of a circle lies at its radius from its center.
    #[test]
    fn a_circle_keeps_its_radius() {
        let mut draw = DebugDraw::new(true);
        draw.circle([1.0, 2.0, 3.0], [0.3, 0.9, 0.1], 2.0, DebugStyle::default());
        for line in draw.collect_frame(0.0).lines {
            let distance = vector_length(subtract_vectors(line.start, [1.0, 2.0, 3.0]));
            assert!((distance - 2.0).abs() < 1e-4, "{distance}");
        }
    }

    /// Switching off forgets what was waiting, and gizmo geometry lasts one
    /// frame.
    #[test]
    fn switching_off_forgets_waiting_items() {
        let mut draw = DebugDraw::new(true);
        draw.gizmo_triangle([[0.0; 3]; 3], [1.0; 4], 7);
        let frame = draw.collect_frame(0.0);
        assert_eq!(frame.gizmo_triangles[0].handle, 7);
        assert!(draw.collect_frame(0.0).gizmo_triangles.is_empty());
        draw.line(
            [0.0; 3],
            [1.0; 3],
            DebugStyle {
                lifetime: DebugLifetime::Seconds(10.0),
                ..DebugStyle::default()
            },
        );
        draw.set_enabled(false);
        draw.set_enabled(true);
        assert!(draw.collect_frame(0.0).is_empty());
    }
}
