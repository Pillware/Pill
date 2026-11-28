//! The CPU rasterizer: turns a frame's mesh instances into framebuffer pixels.
//!
//! # Responsibilities
//!
//! - Transform each instance's triangles from object space to the panel
//!   ([`SceneRasterizer::render`]).
//! - Clip a triangle at the camera's near side and drop the parts behind it.
//! - Fill the covered pixels with a flat-shaded, textured colour, kept in
//!   front-to-back order by a 16-bit depth buffer.
//! - Shade one directional light, the way the GPU chain's key light does.
//!
//! # Design
//!
//! One pass draws every instance of the frame. There is no batching and no
//! draw-order key: the depth buffer decides what is visible, so the order the
//! instances arrive in changes nothing on screen.
//!
//! Shading is flat, per triangle, from the triangle's own face normal. That is
//! enough for the panels this renderer targets and costs one dot product per
//! triangle instead of one per pixel. The face is shaded from the side the
//! camera sees, so a mesh whose winding points inward still draws lit.
//!
//! The light is one directional key light with a flat ambient term, the same
//! direction, colour and intensity the GPU chains' `shade_pbr_surface` uses for
//! its key light. The GPU rig adds a fill and a rim light; a panel at 240x240
//! pixels shows the shape from its key light alone, and one light keeps the
//! inner loop small enough for a device core without an FPU-heavy budget.
//!
//! Nothing here allocates per triangle or per pixel: the depth buffer is sized
//! once, the clipping of a triangle uses a fixed four-corner array, and the
//! pixel loop writes straight into the framebuffer.

// External crates
use pill_core::math::Matrix4f;
use pill_engine::{Asset, AssetManager, Handle};
use pill_renderer_api::{AssetKey, RenderInstance, RenderViewport};
use pill_rendering::assets::{Material, Mesh};
use pill_rendering_core::instance::model_rows;
use pill_rendering_core::materials;

// Current crate
use crate::framebuffer::{pack_rgb565, unpack_rgb565, Framebuffer};
use crate::texture_cache::{CachedTexture, TextureCache};

// =============================================================================
// Constants
// =============================================================================

/// The view-space distance from the camera below which a point counts as
/// behind it and is clipped away. The shared projection code uses the same
/// value for debug geometry.
const NEAR_CLIP_DEPTH: f32 = 1e-3;

/// The key light's direction, pointing from the light toward the scene.
///
/// The same vector the GPU chain's `pbr_surface.hlsl` gives its key light.
const KEY_LIGHT_DIRECTION: [f32; 3] = [-0.5, -0.6, -1.0];

/// The key light's colour, in the same channel order a texture stores.
const KEY_LIGHT_COLOUR: [f32; 3] = [1.0, 0.97, 0.92];

/// The key light's brightness, multiplied into each colour channel.
const KEY_LIGHT_INTENSITY: f32 = 3.0;

/// The flat ambient term that stands in for light bouncing around the scene,
/// the same strength the GPU chain's `pbr_surface.hlsl` uses.
const AMBIENT_STRENGTH: f32 = 0.05;

/// A depth buffer value that no drawn pixel is behind.
const DEPTH_AT_FAR_PLANE: u16 = u16::MAX;

// =============================================================================
// ScreenVertex
// =============================================================================

/// One triangle corner after projection: where it lands on the panel and what
/// the pixel loop interpolates from it.
#[derive(Clone, Copy)]
struct ScreenVertex {
    /// Horizontal position in panel pixels.
    x: f32,
    /// Vertical position in panel pixels, growing down.
    y: f32,
    /// Normalised depth: `0.0` at the near plane, `1.0` at the far one.
    depth: f32,
    /// `1 / w`, which turns the interpolated texture coordinates back into
    /// perspective-correct ones.
    inv_w: f32,
    /// Texture coordinate `u` divided by `w`.
    u_over_w: f32,
    /// Texture coordinate `v` divided by `w`.
    v_over_w: f32,
}

impl ScreenVertex {
    /// Projects one clip-space corner into panel pixels.
    fn from_clip(clip: [f32; 4], uv: [f32; 2], viewport: RenderViewport) -> Self {
        let inv_w = 1.0 / clip[3];
        Self {
            x: viewport.x as f32 + (clip[0] * inv_w * 0.5 + 0.5) * viewport.width as f32,
            y: viewport.y as f32 + (0.5 - clip[1] * inv_w * 0.5) * viewport.height as f32,
            depth: (clip[2] * inv_w).clamp(0.0, 1.0),
            inv_w,
            u_over_w: uv[0] * inv_w,
            v_over_w: uv[1] * inv_w,
        }
    }
}

// =============================================================================
// ClipVertex
// =============================================================================

/// One triangle corner in clip space, with the texture coordinate it carries.
///
/// Clipping the near side adds corners on the cut, so each corner keeps its own
/// texture coordinate to blend once the shape is drawn.
#[derive(Clone, Copy)]
struct ClipVertex {
    /// Position before the perspective divide.
    clip: [f32; 4],
    /// Texture coordinate at this corner.
    uv: [f32; 2],
}

// =============================================================================
// SceneRasterizer
// =============================================================================

/// What one scene pass draws: the draw list, the camera it is seen through and
/// the store those two name.
///
/// It exists so [`SceneRasterizer::render`] takes the pass as one value instead
/// of six arguments that always travel together.
pub struct ScenePass<'a> {
    /// Matrix every vertex is projected through.
    pub view_projection: &'a Matrix4f,
    /// Drawing happens inside this rectangle of the framebuffer.
    pub viewport: RenderViewport,
    /// Where the camera is, which is the direction the key light comes from.
    pub camera_position: [f32; 3],
    /// One entry per mesh instance, in frame order.
    pub instances: &'a [RenderInstance],
    /// The store the meshes and materials are read from.
    pub assets: &'a AssetManager,
}

/// Draws a frame's mesh instances into a [`Framebuffer`] on the CPU.
///
/// The framebuffer belongs to the caller, so one rasterizer can draw into
/// several of them. Its state is the depth buffer, which is sized to the
/// framebuffer it last drew into.
#[derive(Default)]
pub struct SceneRasterizer {
    /// Depth of the nearest pixel written so far, one entry per framebuffer
    /// pixel. A larger value is further away.
    depth: Vec<u16>,
}

impl SceneRasterizer {
    /// Creates a rasterizer with no depth buffer yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Draws `pass`'s instances into `framebuffer`.
    ///
    /// Returns whether any triangle reached the framebuffer, which is what the
    /// renderer reports as a pass with draws.
    pub fn render(
        &mut self,
        framebuffer: &mut Framebuffer,
        textures: &mut TextureCache,
        pass: &ScenePass<'_>,
    ) -> bool {
        let ScenePass {
            view_projection,
            viewport,
            camera_position,
            instances,
            assets,
        } = *pass;
        if viewport.width == 0 || viewport.height == 0 {
            return false;
        }
        self.ensure_depth(framebuffer);
        self.depth.fill(DEPTH_AT_FAR_PLANE);

        let mut drew = false;
        for instance in instances {
            let Some(mesh) = assets.get(handle_from_key::<Mesh>(instance.mesh)) else {
                continue;
            };
            let material = assets.get(handle_from_key::<Material>(instance.material));
            let base_colour = material.map(materials::base_colour).unwrap_or([1.0; 3]);
            let texture_index = material
                .and_then(materials::base_colour_texture)
                .and_then(|handle| textures.ensure(assets, handle));
            let model = model_rows(&instance.transform);

            // Step 1: Draw each triangle. A mesh's index list is one triangle
            // per three entries; an entry past the vertex list is skipped
            // rather than read out of bounds.
            for triangle in mesh.indices.as_chunks::<3>().0 {
                let [first_index, second_index, third_index] =
                    [triangle[0], triangle[1], triangle[2]];
                let (Some(first), Some(second), Some(third)) = (
                    mesh.vertices.get(first_index as usize).copied(),
                    mesh.vertices.get(second_index as usize).copied(),
                    mesh.vertices.get(third_index as usize).copied(),
                ) else {
                    continue;
                };
                let world =
                    [first, second, third].map(|vertex| world_position(&model, vertex.position));
                let shade = shade_face(world, camera_position);
                let vertices = [
                    ClipVertex {
                        clip: clip_position(view_projection, world[0]),
                        uv: first.texture_coordinates,
                    },
                    ClipVertex {
                        clip: clip_position(view_projection, world[1]),
                        uv: second.texture_coordinates,
                    },
                    ClipVertex {
                        clip: clip_position(view_projection, world[2]),
                        uv: third.texture_coordinates,
                    },
                ];

                // Step 2: Clip at the camera's near side, then draw the
                // resulting polygon as a fan of triangles.
                let (count, clipped) = clip_triangle_to_near(vertices);
                let texture = texture_index.and_then(|index| textures.entry(index));
                for corner in 1..count.saturating_sub(1) {
                    let screen = [clipped[0], clipped[corner], clipped[corner + 1]]
                        .map(|vertex| ScreenVertex::from_clip(vertex.clip, vertex.uv, viewport));
                    drew |=
                        self.rasterize(framebuffer, viewport, screen, shade, base_colour, texture);
                }
            }
        }
        drew
    }

    /// The depth buffer of the frame last drawn: one entry per framebuffer
    /// pixel, a larger value further away.
    ///
    /// The debug pass reads it to hide a line the scene covers.
    pub fn depth(&self) -> &[u16] {
        &self.depth
    }

    /// Sizes the depth buffer to a framebuffer, reallocating only when the
    /// size changed.
    fn ensure_depth(&mut self, framebuffer: &Framebuffer) {
        let pixel_count = framebuffer.width() as usize * framebuffer.height() as usize;
        if self.depth.len() != pixel_count {
            self.depth.clear();
            self.depth.resize(pixel_count, DEPTH_AT_FAR_PLANE);
        }
    }

    /// Fills one projected triangle, keeping the nearer pixel wherever two
    /// triangles overlap.
    ///
    /// Returns whether any pixel passed the depth test.
    fn rasterize(
        &mut self,
        framebuffer: &mut Framebuffer,
        viewport: RenderViewport,
        mut triangle: [ScreenVertex; 3],
        shade: [f32; 3],
        base_colour: [f32; 3],
        texture: Option<&CachedTexture>,
    ) -> bool {
        // Step 1: Give the triangle one winding, so its three edge tests share
        // one sign. A triangle with no area covers no pixel.
        self.ensure_depth(framebuffer);
        let mut area = edge_function(triangle[0], triangle[1], triangle[2]);
        if !area.is_finite() || area == 0.0 {
            return false;
        }
        if area < 0.0 {
            triangle.swap(1, 2);
            area = -area;
        }
        let [first, second, third] = triangle;

        // Step 2: Walk only the pixels inside both the triangle's bounding box
        // and the viewport.
        let viewport_right = viewport.x + viewport.width;
        let viewport_bottom = viewport.y + viewport.height;
        let minimum_x = triangle_min(first.x, second.x, third.x)
            .floor()
            .max(viewport.x as f32)
            .max(0.0);
        let maximum_x = triangle_max(first.x, second.x, third.x)
            .ceil()
            .min(viewport_right as f32);
        let minimum_y = triangle_min(first.y, second.y, third.y)
            .floor()
            .max(viewport.y as f32)
            .max(0.0);
        let maximum_y = triangle_max(first.y, second.y, third.y)
            .ceil()
            .min(viewport_bottom as f32);

        let mut drew = false;
        for y in minimum_y as u32..maximum_y as u32 {
            let pixel_y = y as f32 + 0.5;
            for x in minimum_x as u32..maximum_x as u32 {
                let pixel_x = x as f32 + 0.5;

                // Step 3: The three edge functions are the corner weights once
                // divided by the triangle's area: the weight of a corner is the
                // area of the triangle the pixel makes with the other two.
                let weight_first = edge_function_of(second, third, pixel_x, pixel_y) / area;
                let weight_second = edge_function_of(third, first, pixel_x, pixel_y) / area;
                let weight_third = edge_function_of(first, second, pixel_x, pixel_y) / area;
                if weight_first < 0.0 || weight_second < 0.0 || weight_third < 0.0 {
                    continue;
                }

                // Step 4: Keep the nearest pixel drawn so far.
                let depth = interpolate(
                    weight_first,
                    weight_second,
                    weight_third,
                    first.depth,
                    second.depth,
                    third.depth,
                );
                let depth = (depth.clamp(0.0, 1.0) * f32::from(DEPTH_AT_FAR_PLANE)) as u16;
                let index = y as usize * framebuffer.width() as usize + x as usize;
                if depth >= self.depth[index] {
                    continue;
                }
                self.depth[index] = depth;

                // Step 5: Blend the texture and the material's tint, then the
                // light, into one pixel. Texture coordinates are interpolated
                // perspective-correctly: the weights mix `u / w`, `v / w` and
                // `1 / w`, and the division puts the perspective back.
                let inverse_w = interpolate(
                    weight_first,
                    weight_second,
                    weight_third,
                    first.inv_w,
                    second.inv_w,
                    third.inv_w,
                );
                let texel = match texture {
                    Some(texture) if inverse_w != 0.0 => {
                        let u = interpolate(
                            weight_first,
                            weight_second,
                            weight_third,
                            first.u_over_w,
                            second.u_over_w,
                            third.u_over_w,
                        ) / inverse_w;
                        let v = interpolate(
                            weight_first,
                            weight_second,
                            weight_third,
                            first.v_over_w,
                            second.v_over_w,
                            third.v_over_w,
                        ) / inverse_w;
                        texture.sample_nearest(u, v)
                    }
                    _ => pack_rgb565(1.0, 1.0, 1.0),
                };
                let (texel_red, texel_green, texel_blue) = unpack_rgb565(texel);
                framebuffer.write_pixel(
                    x,
                    y,
                    pack_rgb565(
                        texel_red * base_colour[0] * shade[0],
                        texel_green * base_colour[1] * shade[1],
                        texel_blue * base_colour[2] * shade[2],
                    ),
                );
                drew = true;
            }
        }
        drew
    }
}

// =============================================================================
// Geometry
// =============================================================================

/// Turns an instance's transform into the world position of one of its
/// vertices.
///
/// The model matrix's rows are what the shared instance code builds, so this
/// uses the same transform the GPU chain's vertex shader applies.
#[inline]
fn world_position(model: &[[f32; 4]; 3], position: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|row| {
        model[row][0] * position[0]
            + model[row][1] * position[1]
            + model[row][2] * position[2]
            + model[row][3]
    })
}

/// Applies the view-projection matrix to a world position.
#[inline]
fn clip_position(view_projection: &Matrix4f, position: [f32; 3]) -> [f32; 4] {
    (*view_projection * glam::Vec4::new(position[0], position[1], position[2], 1.0)).to_array()
}

/// The direction a triangle faces, from its three world positions.
///
/// A triangle with no area has no direction; it is dropped by the pixel loop
/// anyway, so the face reads as unlit.
fn face_normal(world: [[f32; 3]; 3]) -> Option<glam::Vec3> {
    let [first, second, third] = world.map(glam::Vec3::from_array);
    let normal = (second - first).cross(third - first);
    (normal.length_squared() > f32::EPSILON).then(|| normal.normalize())
}

/// Shades a triangle with the key light and the flat ambient term.
///
/// The normal is turned toward the camera first, so the visible side is the one
/// lit and a mesh whose winding points inward still draws lit.
fn shade_face(world: [[f32; 3]; 3], camera_position: [f32; 3]) -> [f32; 3] {
    let to_light = -glam::Vec3::from_array(KEY_LIGHT_DIRECTION).normalize();
    let Some(mut normal) = face_normal(world) else {
        return [AMBIENT_STRENGTH; 3];
    };
    let centre = (glam::Vec3::from_array(world[0])
        + glam::Vec3::from_array(world[1])
        + glam::Vec3::from_array(world[2]))
        / 3.0;
    if normal.dot(glam::Vec3::from_array(camera_position) - centre) < 0.0 {
        normal = -normal;
    }
    let lambert = normal.dot(to_light).max(0.0) * KEY_LIGHT_INTENSITY;
    std::array::from_fn(|channel| AMBIENT_STRENGTH + KEY_LIGHT_COLOUR[channel] * lambert)
}

/// Clips a triangle at the camera's near side.
///
/// Returns how many corners the clipped polygon has, and the corners
/// themselves. A triangle fully in front keeps its three corners unchanged; one
/// that crosses the plane gains one; one fully behind the camera has none.
/// The corners live in a fixed array, so the clip allocates nothing.
fn clip_triangle_to_near(vertices: [ClipVertex; 3]) -> (usize, [ClipVertex; 4]) {
    let mut clipped = [vertices[0]; 4];
    let mut count = 0;
    for index in 0..3 {
        let current = vertices[index];
        let next = vertices[(index + 1) % 3];
        let current_inside = current.clip[3] >= NEAR_CLIP_DEPTH;
        let next_inside = next.clip[3] >= NEAR_CLIP_DEPTH;
        if current_inside {
            clipped[count] = current;
            count += 1;
        }
        if current_inside != next_inside {
            let crossing = (NEAR_CLIP_DEPTH - current.clip[3]) / (next.clip[3] - current.clip[3]);
            clipped[count] = ClipVertex {
                clip: std::array::from_fn(|component| {
                    current.clip[component]
                        + (next.clip[component] - current.clip[component]) * crossing
                }),
                uv: std::array::from_fn(|component| {
                    current.uv[component] + (next.uv[component] - current.uv[component]) * crossing
                }),
            };
            // The crossing lies on the plane by construction; rounding can put
            // its `w` a hair below the plane, so it is written exactly.
            clipped[count].clip[3] = NEAR_CLIP_DEPTH;
            count += 1;
        }
    }
    (count, clipped)
}

/// Twice the signed area of the triangle the three corners form.
#[inline]
fn edge_function(first: ScreenVertex, second: ScreenVertex, third: ScreenVertex) -> f32 {
    edge_function_of(first, second, third.x, third.y)
}

/// Twice the signed area of the triangle `from`, `to` and the point
/// `(x, y)`.
///
/// This is the one edge test the rasterizer uses: the triangle's own area is
/// this function at the third corner, and each corner weight is the same
/// function at the pixel, divided by that area.
#[inline]
fn edge_function_of(from: ScreenVertex, to: ScreenVertex, x: f32, y: f32) -> f32 {
    (to.x - from.x) * (y - from.y) - (to.y - from.y) * (x - from.x)
}

/// Mixes three corner values with the weights the pixel loop computed.
#[inline]
fn interpolate(
    first_weight: f32,
    second_weight: f32,
    third_weight: f32,
    a: f32,
    b: f32,
    c: f32,
) -> f32 {
    first_weight * a + second_weight * b + third_weight * c
}

/// The smallest of three screen coordinates.
#[inline]
fn triangle_min(a: f32, b: f32, c: f32) -> f32 {
    a.min(b).min(c)
}

/// The largest of three screen coordinates.
#[inline]
fn triangle_max(a: f32, b: f32, c: f32) -> f32 {
    a.max(b).max(c)
}

/// The asset handle the frame's key was built from.
///
/// A frame carries asset keys rather than handles, because it crosses the
/// artifact boundary; the key packs the handle's index and generation, so the
/// handle is rebuilt here.
fn handle_from_key<T: Asset>(key: AssetKey) -> Handle<T> {
    Handle::from_raw(key.0 as u32, (key.0 >> 32) as u32)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A triangle whose winding makes it face `+Z`.
    fn facing_the_camera() -> [ScreenVertex; 3] {
        let corner = |x: f32, y: f32| ScreenVertex {
            x,
            y,
            depth: 0.5,
            inv_w: 1.0,
            u_over_w: 0.0,
            v_over_w: 0.0,
        };
        [corner(1.0, 1.0), corner(5.0, 1.0), corner(3.0, 5.0)]
    }

    /// A triangle blocks the pixels inside it and leaves the others alone.
    #[test]
    fn a_triangle_fills_only_the_pixels_it_covers() {
        let mut rasterizer = SceneRasterizer::new();
        let mut framebuffer = Framebuffer::new(8, 8);
        let viewport = RenderViewport::full(8, 8);

        let drew = rasterizer.rasterize(
            &mut framebuffer,
            viewport,
            facing_the_camera(),
            [1.0; 3],
            [1.0; 3],
            None,
        );

        assert!(drew);
        // Inside: the middle of the triangle's covered area.
        assert_eq!(framebuffer.pixel(3, 2), Some(0xFFFF));
        // Outside: beside it, and above it.
        assert_eq!(framebuffer.pixel(0, 0), Some(0x0000));
        assert_eq!(framebuffer.pixel(7, 7), Some(0x0000));
    }

    /// A triangle that is not in the viewport draws nothing, whatever its
    /// size.
    #[test]
    fn a_viewport_outside_the_frame_draws_nothing() {
        let mut rasterizer = SceneRasterizer::new();
        let mut framebuffer = Framebuffer::new(8, 8);

        let drew = rasterizer.rasterize(
            &mut framebuffer,
            RenderViewport::new(20, 20, 8, 8),
            facing_the_camera(),
            [1.0; 3],
            [1.0; 3],
            None,
        );

        assert!(!drew);
        assert_eq!(framebuffer.pixel(3, 2), Some(0x0000));
    }

    /// The nearer of two triangles keeps its pixels, whichever is drawn last.
    #[test]
    fn the_depth_buffer_keeps_the_nearer_triangle() {
        for near_first in [true, false] {
            let mut rasterizer = SceneRasterizer::new();
            let mut framebuffer = Framebuffer::new(8, 8);
            let viewport = RenderViewport::full(8, 8);
            let mut near = facing_the_camera();
            let mut far = facing_the_camera();
            for corner in near.iter_mut() {
                corner.depth = 0.25;
            }
            for corner in far.iter_mut() {
                corner.depth = 0.75;
            }
            let order = if near_first {
                [(near, 1.0), (far, 0.25)]
            } else {
                [(far, 0.25), (near, 1.0)]
            };

            for (triangle, brightness) in order {
                rasterizer.rasterize(
                    &mut framebuffer,
                    viewport,
                    triangle,
                    [brightness; 3],
                    [1.0; 3],
                    None,
                );
            }

            let (red, _, _) = unpack_rgb565(framebuffer.pixel(3, 2).expect("in bounds"));
            assert!(
                red > 0.75,
                "the near triangle should stay on top (near_first={near_first}, red={red})"
            );
        }
    }

    /// A triangle behind the camera is clipped away instead of wrapping across
    /// the panel.
    #[test]
    fn a_triangle_behind_the_camera_is_clipped_away() {
        let behind = [ClipVertex {
            clip: [-1.0, -1.0, 0.0, -1.0],
            uv: [0.0; 2],
        }; 3];
        let (count, _) = clip_triangle_to_near(behind);

        assert_eq!(count, 0);
    }

    /// A triangle crossing the near plane keeps the part in front and gains
    /// the corners the cut adds.
    #[test]
    fn a_triangle_across_the_near_plane_keeps_its_front_part() {
        let vertices = [
            ClipVertex {
                clip: [-1.0, -1.0, 0.0, 2.0],
                uv: [0.0, 0.0],
            },
            ClipVertex {
                clip: [1.0, -1.0, 0.0, 2.0],
                uv: [1.0, 0.0],
            },
            ClipVertex {
                clip: [0.0, 1.0, 0.0, -2.0],
                uv: [0.5, 1.0],
            },
        ];

        let (count, clipped) = clip_triangle_to_near(vertices);

        assert_eq!(count, 4);
        assert!(clipped[..count]
            .iter()
            .all(|corner| corner.clip[3] >= NEAR_CLIP_DEPTH));
    }

    /// A face turned toward the light is brighter than one turned away from it.
    #[test]
    fn the_key_light_brightens_the_face_it_reaches() {
        let camera = [0.0, 0.0, 3.0];
        let toward_light = [[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]];
        // The same triangle in the XZ plane, whose normal is perpendicular to
        // the light's direction.
        let sideways = [[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [0.0, 0.0, 1.0]];

        let lit = shade_face(toward_light, camera);
        let dim = shade_face(sideways, camera);

        let brightness = |shade: [f32; 3]| shade[0] + shade[1] + shade[2];
        assert!(
            brightness(lit) > brightness(dim),
            "lit {lit:?} should be brighter than dim {dim:?}"
        );
        assert!(brightness(dim) > 0.0, "the ambient term still lights it");
    }

    /// A face is shaded from the side the camera sees, so a mesh whose winding
    /// points away from the camera draws as brightly as one facing it.
    #[test]
    fn a_face_is_shaded_from_the_side_the_camera_sees() {
        let camera = [0.0, 0.0, 3.0];
        let facing = [[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [0.0, 1.0, 0.0]];
        let reversed = [facing[0], facing[2], facing[1]];

        assert_eq!(shade_face(facing, camera), shade_face(reversed, camera));
    }
}
