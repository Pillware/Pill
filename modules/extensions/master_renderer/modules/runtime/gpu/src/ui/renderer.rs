//! Draws [`DrawList`]s with wgpu: one render pipeline, whole-image texture
//! uploads and scissor clipping.
//!
//! # Responsibilities
//!
//! - [`UiRenderer`]: owns the render pipeline and the textures the frames
//!   carried. It uploads texture changes and draws a surface's [`DrawList`]s.
//! - Applies each list's scale factor, its nested clip rectangles (as scissor
//!   rectangles) and correct sRGB blending.
//!
//! # Design
//!
//! The UI renderer uses the renderer module's device, queue and target. It
//! keeps no camera, no window and no per-frame state, apart from two scratch
//! vectors that hold the surface being drawn. One call per surface
//! ([`UiRenderer::render`]) converts every list for that surface into one pair
//! of reused buffers, because a buffer can be written only once per submission
//! (see [`StreamBuffer`]).
//!
//! Textures arrive as a versioned set ([`UiRenderer::apply_texture_set`]). The
//! renderer remembers the revision it applied for each texture and uploads the
//! whole image whenever that revision differs. A new instance holds no
//! textures, so the next set it receives uploads all of them instead of drawing
//! with empty textures. A draw that names a texture it does not hold draws
//! nothing.
//!
//! Colours: `pill_ui_frame` hands over straight-alpha sRGB. The shader decodes
//! the vertex colour to linear light, and the textures are `Rgba8UnormSrgb`, so
//! the texels it samples are linear too. When the target format is sRGB, the
//! hardware encodes linear light back to sRGB exactly once. Blending is
//! `src * src_alpha + dst * (1 - src_alpha)`.

// Standard library
use std::collections::HashMap;

// External crates
use bytemuck::{Pod, Zeroable};
use pill_core::math::Vector2f;
use pill_ui_frame::{
    DrawCommand, DrawList, Rect, TextureId, UiTexture, UiTextureFilter, UiTextureSet, UiVertex,
};

// Current crate
use crate::stream_buffer::StreamBuffer;

// =============================================================================
// Shader
// =============================================================================

/// The whole pipeline in one WGSL source. The vertex stage turns physical
/// pixels into clip space, and the fragment stage returns linear light that an
/// sRGB target encodes.
const UI_SHADER: &str = r#"
struct Globals {
    // Physical target size in .xy; .zw unused.
    viewport: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var ui_texture: texture_2d<f32>;
@group(1) @binding(1) var ui_sampler: sampler;

struct VertexIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

// The sRGB to linear-light transfer function, per channel.
fn srgb_to_linear(color: vec3<f32>) -> vec3<f32> {
    let low = color / 12.92;
    let high = pow((color + 0.055) / 1.055, vec3<f32>(2.4));
    return select(low, high, color > vec3<f32>(0.04045));
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    let normalized = in.position / globals.viewport.xy;
    var out: VertexOut;
    out.position = vec4<f32>(normalized.x * 2.0 - 1.0, 1.0 - normalized.y * 2.0, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // The sampled texel is linear already (the texture decodes sRGB); the
    // vertex colour is decoded here, and the sRGB target encodes the result.
    let texel = textureSample(ui_texture, ui_sampler, in.uv);
    let linear = srgb_to_linear(in.color.rgb) * texel.rgb;
    return vec4<f32>(linear, in.color.a * texel.a);
}
"#;

// =============================================================================
// GpuVertex
// =============================================================================

/// One vertex as the GPU sees it: a position in physical pixels, a texture
/// coordinate and a colour, each `f32` in the fixed layout the pipeline
/// declares.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuVertex {
    position: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

impl GpuVertex {
    /// Returns `vertex` with its logical position multiplied by `scale_factor`
    /// to give physical pixels. Lists drawn at different scale factors can then
    /// share one buffer and one pass.
    fn scaled(vertex: &UiVertex, scale_factor: f32) -> GpuVertex {
        GpuVertex {
            position: [
                vertex.position.x * scale_factor,
                vertex.position.y * scale_factor,
            ],
            uv: [vertex.uv.x, vertex.uv.y],
            color: [
                vertex.color.r,
                vertex.color.g,
                vertex.color.b,
                vertex.color.a,
            ],
        }
    }
}

/// The vertex attributes that match the fields of [`GpuVertex`].
const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![
    0 => Float32x2,
    1 => Float32x2,
    2 => Float32x4
];

// =============================================================================
// UiRenderer
// =============================================================================

/// The GPU resources that draw `pill_ui_frame` draw lists.
///
/// One instance belongs to one device and one target format. The caller uploads
/// texture changes once per frame ([`Self::apply_texture_set`]) and then calls
/// [`Self::render`] once per surface.
pub struct UiRenderer {
    pipeline: wgpu::RenderPipeline,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    /// The sampler used for textures that ask for [`UiTextureFilter::Linear`].
    sampler: wgpu::Sampler,
    /// The sampler used for textures that ask for [`UiTextureFilter::Nearest`].
    nearest_sampler: wgpu::Sampler,
    /// Every texture the received sets carried and this renderer has not freed,
    /// with the revision it was uploaded at. A new instance starts empty, so
    /// its first set uploads everything.
    textures: HashMap<TextureId, HeldTexture>,
    /// The vertices of every list on the surface being drawn, one list after
    /// the next.
    vertices: StreamBuffer,
    /// The indices of every list on the surface being drawn, in the same
    /// order.
    indices: StreamBuffer,
    /// Vertices converted for the surface being drawn. The next surface clears
    /// and refills them, so their allocation is reused.
    scratch_vertices: Vec<GpuVertex>,
    /// Indices of the surface being drawn, reused in the same way.
    scratch_indices: Vec<u32>,
}

impl UiRenderer {
    /// Creates the pipeline and the resources it draws with for
    /// `target_format`.
    ///
    /// `target_format` must be an sRGB format, such as `Rgba8UnormSrgb` or a
    /// surface format's sRGB variant. The shader writes linear light and relies
    /// on the target to encode it.
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> UiRenderer {
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pill_ui_frame globals layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pill_ui_frame texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pill_ui_frame globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pill_ui_frame globals bind group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pill_ui_frame sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("pill_ui_frame nearest sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pill_ui_frame shader"),
            source: wgpu::ShaderSource::Wgsl(UI_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pill_ui_frame pipeline layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pill_ui_frame pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &VERTEX_ATTRIBUTES,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            multiview_mask: None,
            cache: None,
        });

        UiRenderer {
            pipeline,
            globals_buffer,
            globals_bind_group,
            texture_layout,
            sampler,
            nearest_sampler,
            textures: HashMap::new(),
            vertices: StreamBuffer::new("pill_ui_frame vertices", wgpu::BufferUsages::VERTEX),
            indices: StreamBuffer::new("pill_ui_frame indices", wgpu::BufferUsages::INDEX),
            scratch_vertices: Vec::new(),
            scratch_indices: Vec::new(),
        }
    }

    /// Uploads every texture in `texture_set` whose revision this instance has
    /// not applied yet, then frees the ones the set names.
    ///
    /// A set carries whole images, so a new instance that holds no textures
    /// uploads all of them from this one call and can draw with them straight
    /// away. A running instance uploads only the textures whose revision
    /// changed.
    pub fn apply_texture_set(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture_set: &UiTextureSet,
    ) {
        for texture in &texture_set.textures {
            let applied_revision = self.textures.get(&texture.id).map(|entry| entry.revision);
            if applied_revision != Some(texture.revision) {
                self.apply_texture(device, queue, texture);
            }
        }
        for id in &texture_set.frees {
            self.textures.remove(id);
        }
    }

    /// Uploads one texture's whole image at the revision it carries. A GPU
    /// texture of the same size is reused instead of creating a new one.
    fn apply_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, texture: &UiTexture) {
        let texture_size = (texture.width, texture.height);
        let gpu_texture = match self.textures.remove(&texture.id) {
            Some(entry) if entry.texture_size == texture_size => entry.texture,
            _ => create_srgb_texture(device, "pill_ui_frame texture", texture_size),
        };
        upload_whole_texture(queue, &gpu_texture, texture_size, &texture.pixels);
        let sampler = match texture.filter {
            UiTextureFilter::Nearest => &self.nearest_sampler,
            UiTextureFilter::Linear => &self.sampler,
        };
        let view = gpu_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pill_ui_frame texture bind group"),
            layout: &self.texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        self.textures.insert(
            texture.id,
            HeldTexture {
                texture: gpu_texture,
                bind_group,
                texture_size,
                revision: texture.revision,
            },
        );
    }

    /// Draws `lists` into `target`, in order, on top of what the target
    /// already holds.
    ///
    /// `target_size` is the target's size in physical pixels. Each list comes
    /// with the scale factor of the monitor it was built for, which turns its
    /// logical pixels into physical ones. Call this once per surface with every
    /// list for that surface, because the buffers hold one surface's data and
    /// can be written only once per submission.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        target_size: (u32, u32),
        lists: &[(&DrawList, f32)],
    ) {
        if lists.iter().all(|(list, _)| list.is_empty()) {
            return;
        }

        // Step 1: Write the target size the vertex stage divides by.
        queue.write_buffer(
            &self.globals_buffer,
            0,
            bytemuck::cast_slice(&[target_size.0 as f32, target_size.1 as f32, 0.0, 0.0]),
        );
        // Step 2: Convert every list into one vertex run and one index run. A
        // list's vertices follow the previous list's, and its indices keep
        // their own values, so its first vertex is passed as the draw's base
        // vertex. The two vectors belong to this renderer, so drawing on four
        // surfaces reuses their allocation.
        let mut surface_vertices = std::mem::take(&mut self.scratch_vertices);
        surface_vertices.clear();
        let mut surface_indices = std::mem::take(&mut self.scratch_indices);
        surface_indices.clear();
        let mut list_starts = Vec::with_capacity(lists.len());
        for (list, scale_factor) in lists {
            list_starts.push((surface_vertices.len() as i32, surface_indices.len() as u32));
            surface_vertices.extend(
                list.vertices
                    .iter()
                    .map(|vertex| GpuVertex::scaled(vertex, *scale_factor)),
            );
            surface_indices.extend_from_slice(&list.indices);
        }
        let vertex_buffer = self
            .vertices
            .write(device, queue, bytemuck::cast_slice(&surface_vertices))
            .cloned();
        let index_buffer = self
            .indices
            .write(device, queue, bytemuck::cast_slice(&surface_indices))
            .cloned();
        self.scratch_vertices = surface_vertices;
        self.scratch_indices = surface_indices;
        let (Some(vertex_buffer), Some(index_buffer)) = (vertex_buffer, index_buffer) else {
            return;
        };

        // Step 3: Record one pass that draws every list of the surface, with the
        // clip that is in force at each draw.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pill_ui_frame"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.globals_bind_group, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        for ((list, scale_factor), (base_vertex, index_offset)) in lists.iter().zip(list_starts) {
            // A list's clips nest. The innermost one is the scissor in force, and
            // the clip stack gives the previous one back on every pop.
            let full_clip = Rect::new(
                Vector2f::ZERO,
                Vector2f::new(
                    target_size.0 as f32 / scale_factor,
                    target_size.1 as f32 / scale_factor,
                ),
            );
            let mut clip = full_clip;
            let mut clip_stack: Vec<Rect> = Vec::new();

            for command in &list.commands {
                match command {
                    DrawCommand::PushClip(rect) => {
                        clip_stack.push(clip);
                        clip = clip
                            .intersection(rect)
                            .unwrap_or(Rect::from_min_size(Vector2f::ZERO, Vector2f::ZERO));
                    }
                    DrawCommand::PopClip => {
                        clip = clip_stack.pop().unwrap_or(full_clip);
                    }
                    DrawCommand::Draw {
                        texture,
                        first_index,
                        index_count,
                    } => {
                        // A draw whose texture this renderer does not hold is
                        // skipped.
                        let Some(entry) = self.textures.get(texture) else {
                            continue;
                        };
                        let Some(scissor) = physical_scissor(&clip, *scale_factor, target_size)
                        else {
                            continue;
                        };
                        pass.set_scissor_rect(scissor[0], scissor[1], scissor[2], scissor[3]);
                        pass.set_bind_group(1, &entry.bind_group, &[]);
                        let first = index_offset + first_index;
                        pass.draw_indexed(first..first + index_count, base_vertex, 0..1);
                    }
                }
            }
        }
    }
}

// =============================================================================
// HeldTexture
// =============================================================================

/// One texture this renderer holds on the GPU: the image, the bind group that
/// binds it together with its sampler, its texel size and the revision it was
/// uploaded at.
struct HeldTexture {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    texture_size: (u32, u32),
    revision: u64,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Uploads `pixels` as the whole image of the 2D `texture`, which is
/// `texture_size` texels.
fn upload_whole_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    texture_size: (u32, u32),
    pixels: &[u8],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texture_size.0 * 4),
            rows_per_image: Some(texture_size.1),
        },
        wgpu::Extent3d {
            width: texture_size.0,
            height: texture_size.1,
            depth_or_array_layers: 1,
        },
    );
}

/// Creates an `Rgba8UnormSrgb` texture of `texture_size` texels that the shader
/// can sample and the queue can write. Sampling an sRGB texture decodes it to
/// linear light, which is what the shader expects.
fn create_srgb_texture(
    device: &wgpu::Device,
    label: &str,
    texture_size: (u32, u32),
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: texture_size.0,
            height: texture_size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Converts a `clip` in logical pixels into the scissor rectangle a render pass
/// takes: physical pixels, rounded outward and clamped to `target_size`.
/// Returns `None` when nothing would be drawn, because the clip is empty or
/// lies entirely outside the target.
fn physical_scissor(clip: &Rect, scale_factor: f32, target_size: (u32, u32)) -> Option<[u32; 4]> {
    let min_x = (clip.min.x * scale_factor).floor().max(0.0);
    let min_y = (clip.min.y * scale_factor).floor().max(0.0);
    let max_x = (clip.max.x * scale_factor).ceil().min(target_size.0 as f32);
    let max_y = (clip.max.y * scale_factor).ceil().min(target_size.1 as f32);
    if max_x <= min_x || max_y <= min_y {
        return None;
    }
    Some([
        min_x as u32,
        min_y as u32,
        (max_x - min_x) as u32,
        (max_y - min_y) as u32,
    ])
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_ui_frame::Color;

    #[test]
    fn physical_scissor_rounds_outward_and_clamps_to_the_target() {
        let target = (100u32, 80u32);
        let clip = Rect::new(Vector2f::new(1.2, 2.7), Vector2f::new(30.5, 40.1));
        assert_eq!(
            physical_scissor(&clip, 1.0, target),
            Some([1, 2, 30, 39]),
            "min floors, max ceils"
        );
        let clip_at_scale_two = Rect::new(Vector2f::new(1.2, 2.7), Vector2f::new(30.5, 40.1));
        assert_eq!(
            physical_scissor(&clip_at_scale_two, 2.0, target),
            Some([2, 5, 59, 75]),
            "the scale factor multiplies before rounding"
        );

        // The clip runs off the top-left and past the bottom-right.
        let outside = Rect::new(Vector2f::new(-10.0, -10.0), Vector2f::new(500.0, 500.0));
        assert_eq!(
            physical_scissor(&outside, 1.0, target),
            Some([0, 0, 100, 80])
        );
    }

    #[test]
    fn physical_scissor_is_none_for_empty_or_outside_clips() {
        let target = (100u32, 80u32);
        let empty = Rect::from_min_size(Vector2f::ZERO, Vector2f::ZERO);
        assert_eq!(physical_scissor(&empty, 1.0, target), None);
        let outside = Rect::new(Vector2f::new(200.0, 200.0), Vector2f::new(300.0, 300.0));
        assert_eq!(physical_scissor(&outside, 1.0, target), None);
        let zero_width = Rect::new(Vector2f::new(10.0, 10.0), Vector2f::new(10.0, 20.0));
        assert_eq!(
            physical_scissor(&zero_width, 1.0, target),
            None,
            "a clip with no area draws nothing"
        );
    }

    #[test]
    fn gpu_vertex_matches_the_declared_layout() {
        assert_eq!(std::mem::size_of::<GpuVertex>(), 32);
        let vertex = UiVertex {
            position: Vector2f::new(1.0, 2.0),
            uv: Vector2f::new(0.25, 0.75),
            color: Color::rgba(0.1, 0.2, 0.3, 0.4),
        };
        let gpu_vertex = GpuVertex::scaled(&vertex, 2.0);
        assert_eq!(
            gpu_vertex.position,
            [2.0, 4.0],
            "the position is scaled to physical pixels"
        );
        assert_eq!(gpu_vertex.uv, [0.25, 0.75]);
        assert_eq!(gpu_vertex.color, [0.1, 0.2, 0.3, 0.4]);
    }
}
