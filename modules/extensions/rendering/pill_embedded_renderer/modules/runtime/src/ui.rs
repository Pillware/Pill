//! Drawing the frame's UI into the panel.
//!
//! # Responsibilities
//!
//! - Keep the UI textures a frame carries, one entry per texture id at the
//!   revision that was applied ([`UiRasterizer::apply_textures`]).
//! - Draw a surface's draw list ([`DrawList`]) into the framebuffer: textured,
//!   vertex-coloured triangles, clipped by the list's own clip commands.
//! - Blend straight alpha over what the scene already drew.
//!
//! # Design
//!
//! The lists are plain data from `pill_ui_frame`, the same lists the GPU
//! renderer paints, so a game builds its UI once and either renderer draws it.
//! The list's positions are in logical pixels; they are mapped onto the panel
//! with one factor per surface, `scale_factor * panel_width / window_width`.
//! That is what makes a UI built for a window readable on a 240-pixel panel:
//! the same layout, shrunk to the panel.
//!
//! A texture is copied into the cache only when the frame carries a revision
//! the cache does not hold, and the image is handed over as the shared
//! [`Arc`] the producer built, so applying a set allocates nothing for a
//! texture that did not change.
//!
//! The clip stack is a vector that is reused between frames, and the pixel
//! loop writes straight into the framebuffer: after the first frame a UI that
//! does not change allocates nothing.

// Standard library
use std::sync::Arc;

// External crates
use pill_renderer_api::RenderViewport;
use pill_ui_frame::{
    Color, DrawCommand, DrawList, TextureId, UiDraw, UiTextureFilter, UiTextureSet,
};

// Current crate
use crate::framebuffer::Framebuffer;

// =============================================================================
// CachedUiTexture
// =============================================================================

/// One UI texture at the revision the cache last applied.
struct CachedUiTexture {
    /// The id the draw lists sample it by.
    id: TextureId,
    /// The revision of the image held here.
    revision: u64,
    /// Texels across.
    width: u32,
    /// Texel rows.
    height: u32,
    /// Whether the image is sampled with interpolation.
    filter: UiTextureFilter,
    /// The whole image as straight-alpha RGBA8 rows.
    pixels: Arc<[u8]>,
}

impl CachedUiTexture {
    /// Reads the texel at a texture coordinate.
    ///
    /// `Nearest` reads the one texel the coordinate lands in; `Linear` mixes
    /// the four around it, which keeps a scaled or rotated drawing smooth.
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        if self.width == 0 || self.height == 0 {
            return [0.0; 4];
        }
        let texel = |x: u32, y: u32| -> [f32; 4] {
            let x = x.min(self.width - 1);
            let y = y.min(self.height - 1);
            let offset = (y as usize * self.width as usize + x as usize) * 4;
            let channel = |index: usize| self.pixels[offset + index] as f32 / 255.0;
            [channel(0), channel(1), channel(2), channel(3)]
        };
        let x = (u.clamp(0.0, 1.0) * self.width as f32 - 0.5).max(0.0);
        let y = (v.clamp(0.0, 1.0) * self.height as f32 - 0.5).max(0.0);
        match self.filter {
            UiTextureFilter::Nearest => texel(x as u32, y as u32),
            UiTextureFilter::Linear => {
                let (x0, y0) = (x.floor() as u32, y.floor() as u32);
                let (fx, fy) = (x - x0 as f32, y - y0 as f32);
                let mix = |a: f32, b: f32, t: f32| a + (b - a) * t;
                let top = texel(x0, y0);
                let top_right = texel(x0 + 1, y0);
                let bottom = texel(x0, y0 + 1);
                let bottom_right = texel(x0 + 1, y0 + 1);
                std::array::from_fn(|channel| {
                    mix(
                        mix(top[channel], top_right[channel], fx),
                        mix(bottom[channel], bottom_right[channel], fx),
                        fy,
                    )
                })
            }
        }
    }
}

// =============================================================================
// UiRasterizer
// =============================================================================

/// Draws the frame's UI draw lists into the panel.
#[derive(Default)]
pub struct UiRasterizer {
    /// The textures the draw lists sample.
    textures: Vec<CachedUiTexture>,
    /// The clip rectangles that are open, innermost last.
    clips: Vec<[f32; 4]>,
}

impl UiRasterizer {
    /// An empty rasterizer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies the textures a frame carries: a texture at a new revision
    /// replaces the one held, and a freed texture is dropped.
    pub fn apply_textures(&mut self, set: &UiTextureSet) {
        for texture in &set.textures {
            match self
                .textures
                .iter_mut()
                .find(|cached| cached.id == texture.id)
            {
                Some(cached) => {
                    if cached.revision == texture.revision {
                        continue;
                    }
                    cached.revision = texture.revision;
                    cached.width = texture.width;
                    cached.height = texture.height;
                    cached.filter = texture.filter;
                    cached.pixels = Arc::clone(&texture.pixels);
                }
                None => self.textures.push(CachedUiTexture {
                    id: texture.id,
                    revision: texture.revision,
                    width: texture.width,
                    height: texture.height,
                    filter: texture.filter,
                    pixels: Arc::clone(&texture.pixels),
                }),
            }
        }
        for freed in &set.frees {
            self.textures.retain(|cached| cached.id != *freed);
        }
    }

    /// Draws one surface's list into `framebuffer`.
    ///
    /// `window_size` is the surface's size in physical pixels, which is what
    /// turns the list's logical pixels into panel pixels. Returns whether
    /// anything reached the framebuffer.
    pub fn render(
        &mut self,
        framebuffer: &mut Framebuffer,
        draw: &UiDraw,
        viewport: RenderViewport,
        window_size: (u32, u32),
    ) -> bool {
        if window_size.0 == 0 || viewport.width == 0 || viewport.height == 0 {
            return false;
        }
        let scale = draw.scale_factor * viewport.width as f32 / window_size.0 as f32;

        // Step 1: The clip starts as the whole drawing area.
        self.clips.clear();
        self.clips.push([
            viewport.x as f32,
            viewport.y as f32,
            (viewport.x + viewport.width) as f32,
            (viewport.y + viewport.height) as f32,
        ]);

        // Step 2: Run the command stream: clip changes move the stack, draws
        // fill their index range.
        let list: &DrawList = &draw.list;
        let mut drew = false;
        for command in &list.commands {
            match command {
                DrawCommand::PushClip(rect) => {
                    let current = self.clips.last().copied().unwrap_or([
                        0.0,
                        0.0,
                        framebuffer.width() as f32,
                        framebuffer.height() as f32,
                    ]);
                    let min_x = current[0].max(viewport.x as f32 + rect.min.x * scale);
                    let min_y = current[1].max(viewport.y as f32 + rect.min.y * scale);
                    let max_x = current[2].min(viewport.x as f32 + rect.max.x * scale);
                    let max_y = current[3].min(viewport.y as f32 + rect.max.y * scale);
                    self.clips
                        .push([min_x, min_y, max_x.max(min_x), max_y.max(min_y)]);
                }
                DrawCommand::PopClip => {
                    // The outermost clip is never popped: a list that emits an
                    // extra pop keeps drawing inside the drawing area.
                    if self.clips.len() > 1 {
                        self.clips.pop();
                    }
                }
                DrawCommand::Draw {
                    texture,
                    first_index,
                    index_count,
                } => {
                    let Some(texture) = self.textures.iter().find(|cached| cached.id == *texture)
                    else {
                        continue;
                    };
                    let clip = self.clips.last().copied().unwrap_or([0.0; 4]);
                    let start = *first_index as usize;
                    let end = start.saturating_add(*index_count as usize);
                    for triangle in list.indices
                        [start.min(list.indices.len())..end.min(list.indices.len())]
                        .chunks_exact(3)
                    {
                        let corners = [
                            list.vertices.get(triangle[0] as usize),
                            list.vertices.get(triangle[1] as usize),
                            list.vertices.get(triangle[2] as usize),
                        ];
                        let [Some(first), Some(second), Some(third)] = corners else {
                            continue;
                        };
                        let screen = [first, second, third].map(|vertex| ScreenVertex {
                            x: viewport.x as f32 + vertex.position.x * scale,
                            y: viewport.y as f32 + vertex.position.y * scale,
                            u: vertex.uv.x,
                            v: vertex.uv.y,
                            color: vertex.color,
                        });
                        drew |= rasterize_ui_triangle(framebuffer, clip, screen, texture);
                    }
                }
            }
        }
        drew
    }

    /// How many textures the cache holds.
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// The capacity of the clip stack, which the renderer reuses between
    /// frames.
    pub fn clip_capacity(&self) -> usize {
        self.clips.capacity()
    }
}

// =============================================================================
// ScreenVertex
// =============================================================================

/// One UI triangle corner in panel pixels.
#[derive(Clone, Copy)]
struct ScreenVertex {
    /// Horizontal position in panel pixels.
    x: f32,
    /// Vertical position in panel pixels, growing down.
    y: f32,
    /// Texture coordinate.
    u: f32,
    /// Texture coordinate.
    v: f32,
    /// The vertex colour, multiplied into the sampled texel.
    color: Color,
}

// =============================================================================
// Rasterizing
// =============================================================================

/// Fills one UI triangle, blended over the framebuffer and cut to `clip`.
///
/// The clip is `[min_x, min_y, max_x, max_y]` in panel pixels. UI geometry is
/// flat, so the vertex values interpolate in screen space with no perspective
/// division. Returns whether any pixel was written.
fn rasterize_ui_triangle(
    framebuffer: &mut Framebuffer,
    clip: [f32; 4],
    mut triangle: [ScreenVertex; 3],
    texture: &CachedUiTexture,
) -> bool {
    // Step 1: One winding for all three edge tests.
    let mut area = edge_function(triangle[0], triangle[1], triangle[2]);
    if !area.is_finite() || area == 0.0 {
        return false;
    }
    if area < 0.0 {
        triangle.swap(1, 2);
        area = -area;
    }
    let [first, second, third] = triangle;

    // Step 2: Walk the pixels inside the triangle, the clip and the panel.
    let minimum_x = first
        .x
        .min(second.x)
        .min(third.x)
        .floor()
        .max(clip[0])
        .max(0.0);
    let maximum_x = first
        .x
        .max(second.x)
        .max(third.x)
        .ceil()
        .min(clip[2])
        .min(framebuffer.width() as f32);
    let minimum_y = first
        .y
        .min(second.y)
        .min(third.y)
        .floor()
        .max(clip[1])
        .max(0.0);
    let maximum_y = first
        .y
        .max(second.y)
        .max(third.y)
        .ceil()
        .min(clip[3])
        .min(framebuffer.height() as f32);

    let mut drew = false;
    for y in minimum_y as u32..maximum_y as u32 {
        let pixel_y = y as f32 + 0.5;
        for x in minimum_x as u32..maximum_x as u32 {
            let pixel_x = x as f32 + 0.5;
            let weight_first = edge_function_of(second, third, pixel_x, pixel_y) / area;
            let weight_second = edge_function_of(third, first, pixel_x, pixel_y) / area;
            let weight_third = edge_function_of(first, second, pixel_x, pixel_y) / area;
            if weight_first < 0.0 || weight_second < 0.0 || weight_third < 0.0 {
                continue;
            }

            // Step 3: Mix the sampled texel with the vertex colour and blend
            // the result over the panel.
            let u = weight_first * first.u + weight_second * second.u + weight_third * third.u;
            let v = weight_first * first.v + weight_second * second.v + weight_third * third.v;
            let mut texel = texture.sample(u, v);
            let color = [first.color, second.color, third.color];
            for (index, texel_channel) in texel.iter_mut().enumerate() {
                let vertex: f32 = weight_first * channel(color[0], index)
                    + weight_second * channel(color[1], index)
                    + weight_third * channel(color[2], index);
                *texel_channel *= vertex;
            }
            if texel[3] > 0.0 {
                framebuffer.blend_pixel(x, y, texel);
                drew = true;
            }
        }
    }
    drew
}

/// Twice the signed area of the triangle the three corners form.
#[inline]
fn edge_function(first: ScreenVertex, second: ScreenVertex, third: ScreenVertex) -> f32 {
    edge_function_of(first, second, third.x, third.y)
}

/// The edge function `from`-`to` at a pixel, which the corner weights divide
/// by the triangle's area.
#[inline]
fn edge_function_of(from: ScreenVertex, to: ScreenVertex, x: f32, y: f32) -> f32 {
    (to.x - from.x) * (y - from.y) - (to.y - from.y) * (x - from.x)
}

/// One channel of a colour, `0` red, `1` green, `2` blue and `3` alpha.
#[inline]
fn channel(color: Color, index: usize) -> f32 {
    match index {
        0 => color.r,
        1 => color.g,
        2 => color.b,
        _ => color.a,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_ui_frame::{Rect, UiVertex};

    /// A white 1x1 texture, the texel a solid fill samples.
    fn white_texture() -> UiTextureSet {
        let mut set = UiTextureSet::new();
        set.push_full(
            TextureId(1),
            1,
            1,
            1,
            UiTextureFilter::Nearest,
            Arc::from(vec![255u8, 255, 255, 255]),
        );
        set
    }

    /// A quad from `min` to `max` in logical pixels, sampling the whole
    /// texture, tinted `color`.
    fn quad_list(min: [f32; 2], max: [f32; 2], color: Color) -> DrawList {
        let mut list = DrawList::new();
        for (position, uv) in [
            ([min[0], min[1]], [0.0, 0.0]),
            ([max[0], min[1]], [1.0, 0.0]),
            ([max[0], max[1]], [1.0, 1.0]),
            ([min[0], max[1]], [0.0, 1.0]),
        ] {
            list.vertices.push(UiVertex {
                position: pill_core::math::Vector2f::new(position[0], position[1]),
                uv: pill_core::math::Vector2f::new(uv[0], uv[1]),
                color,
            });
        }
        list.indices.extend([0, 1, 2, 0, 2, 3]);
        list.commands.push(DrawCommand::Draw {
            texture: TextureId(1),
            first_index: 0,
            index_count: 6,
        });
        list
    }

    fn ui_draw(list: DrawList) -> UiDraw {
        UiDraw {
            surface: 0,
            scale_factor: 1.0,
            list,
        }
    }

    /// A red quad in the top-left quarter fills only that quarter.
    #[test]
    fn a_quad_fills_the_pixels_it_covers() {
        let mut rasterizer = UiRasterizer::new();
        rasterizer.apply_textures(&white_texture());
        let mut framebuffer = Framebuffer::new(40, 40);
        let draw = ui_draw(quad_list(
            [0.0, 0.0],
            [20.0, 20.0],
            Color::rgba(1.0, 0.0, 0.0, 1.0),
        ));

        let drew = rasterizer.render(
            &mut framebuffer,
            &draw,
            RenderViewport::full(40, 40),
            (40, 40),
        );

        assert!(drew);
        let inside = crate::framebuffer::unpack_rgb565(framebuffer.pixel(10, 10).expect("inside"));
        assert!(
            inside.0 > 0.9 && inside.1 < 0.1,
            "the quad is red: {inside:?}"
        );
        assert_eq!(framebuffer.pixel(30, 30), Some(0x0000));
    }

    /// A clip rectangle cuts the quad down to the part inside it.
    #[test]
    fn a_clip_command_cuts_the_drawing() {
        let mut rasterizer = UiRasterizer::new();
        rasterizer.apply_textures(&white_texture());
        let mut framebuffer = Framebuffer::new(40, 40);
        let mut list = quad_list([0.0, 0.0], [40.0, 40.0], Color::rgba(0.0, 1.0, 0.0, 1.0));
        list.commands.insert(
            0,
            DrawCommand::PushClip(Rect::new(
                pill_core::math::Vector2f::new(0.0, 0.0),
                pill_core::math::Vector2f::new(10.0, 10.0),
            )),
        );
        list.commands.push(DrawCommand::PopClip);
        let draw = ui_draw(list);

        rasterizer.render(
            &mut framebuffer,
            &draw,
            RenderViewport::full(40, 40),
            (40, 40),
        );

        let inside = crate::framebuffer::unpack_rgb565(framebuffer.pixel(5, 5).expect("inside"));
        assert!(inside.1 > 0.9, "inside the clip the quad drew: {inside:?}");
        assert_eq!(
            framebuffer.pixel(20, 20),
            Some(0x0000),
            "outside the clip it did not"
        );
    }

    /// A half-transparent quad blends with what was already there.
    #[test]
    fn a_translucent_quad_blends_over_the_scene() {
        let mut rasterizer = UiRasterizer::new();
        rasterizer.apply_textures(&white_texture());
        let mut framebuffer = Framebuffer::new(8, 8);
        framebuffer.clear(crate::framebuffer::pack_rgb565(0.0, 0.0, 1.0));
        let draw = ui_draw(quad_list(
            [0.0, 0.0],
            [8.0, 8.0],
            Color::rgba(1.0, 0.0, 0.0, 0.5),
        ));

        rasterizer.render(&mut framebuffer, &draw, RenderViewport::full(8, 8), (8, 8));

        // A pixel off the quad's diagonal, where its two triangles meet: on
        // the diagonal both triangles would blend the same pixel twice.
        let (red, green, blue) =
            crate::framebuffer::unpack_rgb565(framebuffer.pixel(2, 5).expect("inside"));
        assert!(
            (red - 0.5).abs() < 0.1 && (blue - 0.5).abs() < 0.1 && green < 0.1,
            "half red over blue is purple: ({red}, {green}, {blue})"
        );
    }

    /// The texture the list samples is what lands on the panel: the left half
    /// of a two-coloured image draws red, the right half green.
    #[test]
    fn a_textured_quad_samples_the_cached_image() {
        let mut set = UiTextureSet::new();
        set.push_full(
            TextureId(7),
            2,
            1,
            1,
            UiTextureFilter::Nearest,
            Arc::from(vec![255u8, 0, 0, 255, 0, 255, 0, 255]),
        );
        let mut rasterizer = UiRasterizer::new();
        rasterizer.apply_textures(&set);
        let mut framebuffer = Framebuffer::new(20, 8);
        let mut list = quad_list([0.0, 0.0], [20.0, 8.0], Color::WHITE);
        list.commands[0] = DrawCommand::Draw {
            texture: TextureId(7),
            first_index: 0,
            index_count: 6,
        };

        rasterizer.render(
            &mut framebuffer,
            &ui_draw(list),
            RenderViewport::full(20, 8),
            (20, 8),
        );

        let left = crate::framebuffer::unpack_rgb565(framebuffer.pixel(2, 4).expect("inside"));
        let right = crate::framebuffer::unpack_rgb565(framebuffer.pixel(17, 4).expect("inside"));
        assert!(
            left.0 > 0.9 && left.1 < 0.1,
            "left is the red texel: {left:?}"
        );
        assert!(
            right.1 > 0.9 && right.0 < 0.1,
            "right is the green texel: {right:?}"
        );
    }

    /// A list that samples a texture the frame never carried draws nothing for
    /// that command instead of the wrong image.
    #[test]
    fn a_missing_texture_draws_nothing() {
        let mut rasterizer = UiRasterizer::new();
        let mut framebuffer = Framebuffer::new(8, 8);
        let draw = ui_draw(quad_list([0.0, 0.0], [8.0, 8.0], Color::WHITE));

        let drew = rasterizer.render(&mut framebuffer, &draw, RenderViewport::full(8, 8), (8, 8));

        assert!(!drew);
    }

    /// Applying the same texture set twice keeps one entry and its revision.
    #[test]
    fn a_texture_is_cached_once_per_revision() {
        let mut rasterizer = UiRasterizer::new();
        let set = white_texture();

        rasterizer.apply_textures(&set);
        rasterizer.apply_textures(&set);

        assert_eq!(rasterizer.texture_count(), 1);
    }

    /// A freed texture leaves the cache.
    #[test]
    fn a_freed_texture_is_dropped() {
        let mut rasterizer = UiRasterizer::new();
        rasterizer.apply_textures(&white_texture());
        let mut freed = UiTextureSet::new();
        freed.push_free(TextureId(1));

        rasterizer.apply_textures(&freed);

        assert_eq!(rasterizer.texture_count(), 0);
    }

    /// The second frame of an unchanged list reuses the clip stack, so
    /// drawing it allocates nothing.
    #[test]
    fn an_unchanged_second_frame_keeps_the_clip_stack() {
        let mut rasterizer = UiRasterizer::new();
        rasterizer.apply_textures(&white_texture());
        let mut framebuffer = Framebuffer::new(40, 40);
        let mut list = quad_list([0.0, 0.0], [40.0, 40.0], Color::WHITE);
        list.commands.insert(
            0,
            DrawCommand::PushClip(Rect::new(
                pill_core::math::Vector2f::new(0.0, 0.0),
                pill_core::math::Vector2f::new(30.0, 30.0),
            )),
        );
        list.commands.push(DrawCommand::PopClip);
        let draw = ui_draw(list);
        let viewport = RenderViewport::full(40, 40);

        rasterizer.render(&mut framebuffer, &draw, viewport, (40, 40));
        let capacity = rasterizer.clip_capacity();
        rasterizer.render(&mut framebuffer, &draw, viewport, (40, 40));

        assert_eq!(rasterizer.clip_capacity(), capacity);
    }
}
