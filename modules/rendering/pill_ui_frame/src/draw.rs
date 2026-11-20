//! What a UI wants drawn, as plain data.
//!
//! # Responsibilities
//!
//! - Defines the data a backend draws: [`DrawList`], with its vertices, its
//!   indices and the commands that use them (clip changes and draws), plus the
//!   values those commands carry ([`UiVertex`], [`Color`], [`Rect`],
//!   [`TextureId`]).
//! - Batches consecutive quads that share a texture and no clip change into
//!   one command, so a run of glyph quads is one draw.
//!
//! # Design
//!
//! The list only grows while a frame is built; the owner clears it before
//! building the next frame. Positions are logical pixels with the origin at the
//! UI's top-left corner and y growing down. The backend applies the monitor's
//! scale factor when it draws.
//!
//! Clips nest. The backend intersects each [`DrawList::push_clip`] rectangle
//! with the clip already in force, and every draw between a push and its
//! matching [`DrawList::pop_clip`] is clipped to the result.
//!
//! Every draw samples a texture that the producer put in the frame's texture
//! set. A solid fill samples a white texel in one of those textures, because
//! the format has no separate fill command.

// External crates
use pill_core::math::Vector2f;

// =============================================================================
// Color
// =============================================================================

/// A colour in sRGB. Each channel is an `f32` in `0.0..=1.0`.
///
/// Alpha is straight, not premultiplied: it is stored on its own and is not
/// multiplied into the colour channels.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Color {
    /// Red channel.
    pub r: f32,
    /// Green channel.
    pub g: f32,
    /// Blue channel.
    pub b: f32,
    /// Alpha channel. `0.0` is fully transparent and `1.0` is opaque.
    pub a: f32,
}

impl Color {
    /// Opaque white.
    pub const WHITE: Color = Color::rgba(1.0, 1.0, 1.0, 1.0);

    /// A colour from straight alpha components in `0.0..=1.0`.
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
        Color { r, g, b, a }
    }

    /// A colour from 8-bit sRGB components.
    pub fn from_rgba8(rgba: [u8; 4]) -> Color {
        Color {
            r: rgba[0] as f32 / 255.0,
            g: rgba[1] as f32 / 255.0,
            b: rgba[2] as f32 / 255.0,
            a: rgba[3] as f32 / 255.0,
        }
    }
}

impl Default for Color {
    fn default() -> Color {
        Color::WHITE
    }
}

// =============================================================================
// Rect
// =============================================================================

/// An axis-aligned rectangle in logical pixels.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Rect {
    /// The corner with the smaller x and y.
    pub min: Vector2f,
    /// The corner with the larger x and y.
    pub max: Vector2f,
}

impl Rect {
    /// A rectangle from its two corners, whatever their order.
    pub fn new(min: Vector2f, max: Vector2f) -> Rect {
        Rect {
            min: Vector2f::new(min.x.min(max.x), min.y.min(max.y)),
            max: Vector2f::new(min.x.max(max.x), min.y.max(max.y)),
        }
    }

    /// A rectangle from its top-left corner and its size.
    pub fn from_min_size(min: Vector2f, size: Vector2f) -> Rect {
        Rect {
            min,
            max: min + size,
        }
    }

    /// The width, along x.
    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    /// The height, along y.
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    /// The overlap of two rectangles, or `None` when they do not overlap.
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let min = Vector2f::new(self.min.x.max(other.min.x), self.min.y.max(other.min.y));
        let max = Vector2f::new(self.max.x.min(other.max.x), self.max.y.min(other.max.y));
        (min.x <= max.x && min.y <= max.y).then_some(Rect { min, max })
    }
}

// =============================================================================
// Texture and vertex types
// =============================================================================

/// Identifies the texture a draw samples.
///
/// The number is the one the producer gave the texture in the frame's texture
/// set.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(pub u64);

/// One vertex of a UI triangle.
///
/// `position` is in logical pixels, `uv` is a texture coordinate in
/// `0.0..=1.0`, and `color` is multiplied into the sampled texel.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct UiVertex {
    /// Position in logical pixels, y down.
    pub position: Vector2f,
    /// Texture coordinate. `(0.0, 0.0)` is the texture's top-left corner.
    pub uv: Vector2f,
    /// The vertex colour.
    pub color: Color,
}

/// One entry in a [`DrawList`]'s command stream: open a clip, close a clip, or
/// draw an index range from one texture.
#[derive(Clone, Debug, PartialEq)]
pub enum DrawCommand {
    /// Clips every draw after this one to `rect`, intersected with the clip
    /// already in force.
    PushClip(Rect),
    /// Return to the clip that was active before the matching push.
    PopClip,
    /// Draw the given index range from `texture`.
    Draw {
        /// The texture to sample.
        texture: TextureId,
        /// The first index into [`DrawList::indices`].
        first_index: u32,
        /// How many indices to draw.
        index_count: u32,
    },
}

// =============================================================================
// DrawList
// =============================================================================

/// Everything one frame of UI asks a backend to draw.
///
/// It holds the vertices, the indices and the commands that use them, in
/// submission order. The owner clears and rebuilds it every frame. The list
/// owns no GPU resources and is plain data any backend can read.
#[derive(Clone, Debug, Default)]
pub struct DrawList {
    /// The frame's vertices, referenced by index.
    pub vertices: Vec<UiVertex>,
    /// The frame's indices, in draw order.
    pub indices: Vec<u32>,
    /// Clip changes and draws, in submission order.
    pub commands: Vec<DrawCommand>,
    /// The number of clips that are currently open, which keeps pushes and pops balanced.
    clip_depth: u32,
}

impl DrawList {
    /// An empty list, ready for a frame.
    pub fn new() -> DrawList {
        DrawList::default()
    }

    /// Returns `true` when nothing would be drawn, which means no vertices and no commands.
    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty() && self.commands.is_empty()
    }

    /// Forget the previous frame's work, keeping the allocations.
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.commands.clear();
        self.clip_depth = 0;
    }

    /// Clip every following draw to `rect` until the matching
    /// [`Self::pop_clip`].
    ///
    /// The backend intersects this with whatever clip is already active.
    pub fn push_clip(&mut self, rect: Rect) {
        self.clip_depth += 1;
        self.commands.push(DrawCommand::PushClip(rect));
    }

    /// Closes the most recent [`Self::push_clip`].
    ///
    /// # Panics
    ///
    /// Panics in debug builds when no clip is open, which means there is one
    /// more pop than push. A release build still records the pop as a
    /// [`DrawCommand::PopClip`].
    pub fn pop_clip(&mut self) {
        debug_assert!(self.clip_depth > 0, "pop_clip without a matching push_clip");
        self.clip_depth = self.clip_depth.saturating_sub(1);
        self.commands.push(DrawCommand::PopClip);
    }

    /// Appends a rectangle that samples the part of `texture_id` between
    /// `uv_min` and `uv_max`.
    ///
    /// The vertices go in top-left, top-right, bottom-right, bottom-left
    /// order, so `uv_min` is on the rectangle's top-left corner and `uv_max` on
    /// its bottom-right one, whatever the texture's orientation.
    pub fn push_quad(
        &mut self,
        texture_id: TextureId,
        rect: Rect,
        uv_min: Vector2f,
        uv_max: Vector2f,
        color: Color,
    ) {
        let base = self.vertices.len() as u32;
        let corners = [
            (rect.min.x, rect.min.y, uv_min.x, uv_min.y),
            (rect.max.x, rect.min.y, uv_max.x, uv_min.y),
            (rect.max.x, rect.max.y, uv_max.x, uv_max.y),
            (rect.min.x, rect.max.y, uv_min.x, uv_max.y),
        ];
        for (x, y, u, v) in corners {
            self.vertices.push(UiVertex {
                position: Vector2f::new(x, y),
                uv: Vector2f::new(u, v),
                color,
            });
        }

        let first_index = self.indices.len() as u32;
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        self.append_draw(texture_id, first_index, 6);
    }

    /// Extends the previous draw command when it samples the same texture and
    /// the new indices follow the old ones directly. Otherwise it starts a new
    /// draw command.
    fn append_draw(&mut self, texture_id: TextureId, first_index: u32, index_count: u32) {
        if let Some(DrawCommand::Draw {
            texture: previous_texture_id,
            first_index: previous_first_index,
            index_count: previous_index_count,
        }) = self.commands.last_mut()
        {
            if *previous_texture_id == texture_id
                && previous_first_index.saturating_add(*previous_index_count) == first_index
            {
                *previous_index_count += index_count;
                return;
            }
        }
        self.commands.push(DrawCommand::Draw {
            texture: texture_id,
            first_index,
            index_count,
        });
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn quad_rect(min_x: f32, min_y: f32, side_length: f32) -> Rect {
        Rect::from_min_size(
            Vector2f::new(min_x, min_y),
            Vector2f::new(side_length, side_length),
        )
    }

    /// Pushes one quad onto `list` that samples the whole texture, in white.
    fn push_white_quad(list: &mut DrawList, texture_id: u64, rect: Rect) {
        list.push_quad(
            TextureId(texture_id),
            rect,
            Vector2f::ZERO,
            Vector2f::new(1.0, 1.0),
            Color::WHITE,
        );
    }

    #[test]
    fn a_quad_adds_four_vertices_and_six_indices() {
        let mut list = DrawList::new();
        push_white_quad(&mut list, 1, quad_rect(0.0, 0.0, 10.0));
        assert_eq!(list.vertices.len(), 4);
        assert_eq!(list.indices, [0, 1, 2, 0, 2, 3]);
        assert_eq!(list.vertices[2].uv, Vector2f::new(1.0, 1.0));
        assert_eq!(
            list.commands,
            [DrawCommand::Draw {
                texture: TextureId(1),
                first_index: 0,
                index_count: 6,
            }]
        );
        assert!(!list.is_empty());
    }

    #[test]
    fn consecutive_quads_of_one_texture_share_a_command() {
        let mut list = DrawList::new();
        push_white_quad(&mut list, 3, quad_rect(0.0, 0.0, 8.0));
        push_white_quad(&mut list, 3, quad_rect(8.0, 0.0, 8.0));
        assert_eq!(list.vertices.len(), 8);
        assert_eq!(
            list.commands,
            [DrawCommand::Draw {
                texture: TextureId(3),
                first_index: 0,
                index_count: 12,
            }]
        );
    }

    #[test]
    fn a_texture_change_or_a_clip_change_starts_a_new_command() {
        let mut list = DrawList::new();
        push_white_quad(&mut list, 1, quad_rect(0.0, 0.0, 8.0));
        push_white_quad(&mut list, 7, quad_rect(8.0, 0.0, 8.0));
        list.push_clip(quad_rect(0.0, 0.0, 100.0));
        push_white_quad(&mut list, 7, quad_rect(16.0, 0.0, 8.0));
        assert_eq!(list.commands.len(), 4);
        assert!(matches!(list.commands[2], DrawCommand::PushClip(_)));
        assert!(matches!(
            list.commands[3],
            DrawCommand::Draw {
                first_index: 12,
                index_count: 6,
                ..
            }
        ));
    }

    #[test]
    fn clip_pushes_and_pops_stay_balanced_across_clear() {
        let mut list = DrawList::new();
        list.push_clip(quad_rect(0.0, 0.0, 10.0));
        list.pop_clip();
        list.clear();
        assert!(list.is_empty());
        assert_eq!(list.commands.len(), 0);
        // After `clear`, a new frame can push a clip again without the debug assertion failing.
        list.push_clip(quad_rect(0.0, 0.0, 10.0));
        list.pop_clip();
    }

    #[test]
    fn rectangles_normalize_and_intersect() {
        let rect = Rect::new(Vector2f::new(10.0, 10.0), Vector2f::new(0.0, 0.0));
        assert_eq!(rect.min, Vector2f::ZERO);
        assert_eq!(rect.max, Vector2f::new(10.0, 10.0));

        let other = Rect::from_min_size(Vector2f::new(5.0, 5.0), Vector2f::new(10.0, 10.0));
        assert_eq!(
            rect.intersection(&other),
            Some(Rect::from_min_size(
                Vector2f::new(5.0, 5.0),
                Vector2f::new(5.0, 5.0)
            ))
        );
        let apart = Rect::from_min_size(Vector2f::new(50.0, 50.0), Vector2f::new(1.0, 1.0));
        assert_eq!(rect.intersection(&apart), None);
    }
}
