//! Asset previews: one small off-screen render, read back as pixels.
//!
//! # Responsibilities
//!
//! - Render one requested asset into a 128x128 RGBA8 target and copy it to a
//!   readback buffer ([`PreviewRenderer::record`],
//!   [`PreviewRenderer::record_mesh`]). At most one request is drawn per
//!   frame.
//! - Map that buffer after the frame that wrote it was submitted
//!   ([`PreviewRenderer::after_submit`]). Hand the pixels back once the GPU
//!   has finished ([`PreviewRenderer::collect`]).
//! - Decode and upload a texture file the first time a preview names it, and
//!   again when the file changes ([`PreviewRenderer::texture`]). Mesh files
//!   work the same way ([`PreviewRenderer::mesh`]).
//!
//! # Design
//!
//! A texture or material preview is one fullscreen triangle. The fragment
//! shader samples the asset's texture, or shades a sphere in a material's
//! colour, so these previews need no vertex buffer, camera or depth target.
//! A mesh preview uses a second pipeline that draws the decoded triangle list
//! through a camera fitted to the mesh's bounds. It has a depth target because
//! a solid seen from a three-quarter angle hides part of itself.
//!
//! The target and both pipelines are built once and reused. The readback is a
//! mapped buffer polled without waiting, the same shape the pick buffer uses.
//! Nothing is written to disk: the pixels reach the editor as data.

// Standard library
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::SystemTime;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_renderer_api::PreviewImage;

// Current crate
use crate::readback::{MappedReadback, Readback};

// =============================================================================
// Constants
// =============================================================================

/// Edge length of one preview, in texels.
pub const PREVIEW_SIZE: u32 = 128;

/// Bytes one preview's pixels take: RGBA8, tightly packed.
const PREVIEW_BYTES: u64 = (PREVIEW_SIZE as u64) * (PREVIEW_SIZE as u64) * 4;

/// How many GPU bytes the decoded texture files may hold.
///
/// The cache is there so that scrolling back to a tile costs no decode. It is
/// not a residency list, so it has a fixed limit instead of growing for the
/// whole session. A 4K texture is 64 MB, so this limit holds one of those or a
/// few dozen smaller ones.
const TEXTURE_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

/// How many GPU bytes the decoded mesh files may hold.
///
/// A mesh is far smaller than a texture, so the same limit covers a whole
/// browsing session.
const MESH_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

/// The mesh vertex attributes the mesh pipeline reads: position and normal,
/// at the offsets the asset crate's vertex struct gives them.
const MESH_ATTRIBUTES: [wgpu::VertexAttribute; 2] = [
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 0,
        shader_location: 0,
    },
    wgpu::VertexAttribute {
        format: wgpu::VertexFormat::Float32x3,
        offset: 20,
        shader_location: 1,
    },
];

// =============================================================================
// Preview Mode
// =============================================================================

/// What the fragment shader draws, as the mode the parameters carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PreviewMode {
    /// A texture, fitted into the square.
    Texture = 0,
    /// A sphere in a material's colour.
    Material = 1,
}

// =============================================================================
// Preview Parameters
// =============================================================================

/// The uniform a texture or material preview writes.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct PreviewParameters {
    /// The material's colour. A texture preview leaves it unused.
    color: [f32; 4],
    /// The [`PreviewMode`] value the shader compares against, stored in the
    /// first word of a vec4. The other three words keep the uniform a multiple
    /// of 16 bytes.
    mode: [u32; 4],
}

// =============================================================================
// Mesh Parameters
// =============================================================================

/// The uniform a mesh preview writes. It carries the camera framing and the
/// colour the mesh is shaded in.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshParameters {
    /// Projection times view, aimed at the centre of the mesh's bounds.
    transform: [[f32; 4]; 4],
    /// The grey a mesh preview shades in.
    color: [f32; 4],
}

// =============================================================================
// Mesh Source
// =============================================================================

/// One decoded mesh on the GPU, with the transform that frames it.
#[derive(Clone)]
pub(super) struct MeshSource {
    /// Interleaved vertices, in the asset crate's `MeshVertex` layout.
    vertices: wgpu::Buffer,
    /// Triangle indices into [`Self::vertices`].
    indices: wgpu::Buffer,
    /// How many indices to draw.
    index_count: u32,
    /// Projection times view, framing the mesh inside the square.
    transform: [[f32; 4]; 4],
}

// =============================================================================
// Preview Renderer
// =============================================================================

/// The GPU state one preview at a time needs: a 128x128 target, the two
/// pipelines that draw into it, and one readback slot.
pub(super) struct PreviewRenderer {
    /// The flat preview's bind group layout and pipeline: its parameters,
    /// texture and sampler.
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    /// The mesh pipeline and its camera uniform: its own vertex layout,
    /// depth test and framing.
    mesh_layout: wgpu::BindGroupLayout,
    mesh_pipeline: wgpu::RenderPipeline,
    mesh_parameters: wgpu::Buffer,
    /// The depth target a mesh preview tests against.
    depth: wgpu::TextureView,
    /// The 128x128 render target every preview draws into, and its view.
    target: wgpu::Texture,
    view: wgpu::TextureView,
    /// Where the drawn pixels are copied for mapping.
    readback: MappedReadback,
    /// The parameters buffer, rewritten per request.
    parameters: wgpu::Buffer,
    /// The sampler a texture preview reads through.
    sampler: wgpu::Sampler,
    /// A 1x1 white texture. A material preview binds it in place of the
    /// asset's texture, so one bind group layout serves both modes.
    white_texture_view: wgpu::TextureView,
    /// Texture views by path, with the modified time each was decoded from.
    texture_sources: BudgetedCache<wgpu::TextureView>,
    /// Mesh sources by path, with the modified time each was decoded from.
    mesh_sources: BudgetedCache<MeshSource>,
    /// The id of the request whose pixels are on their way.
    in_flight_request_id: Option<u64>,
    /// Finished previews waiting for the next [`Self::collect`].
    finished_previews: Vec<PreviewImage>,
}

impl PreviewRenderer {
    /// Builds the target, both pipelines, the readback buffer and the caches.
    pub(super) fn new(device: &wgpu::Device) -> Self {
        // Step 1: The flat pipeline - one fullscreen triangle with a uniform, a
        // texture and a sampler.
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("preview.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview_pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("preview_vertex"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("preview_fragment"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        // Step 2: The target every preview draws into, and the buffer its
        // pixels are copied out to.
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview_target"),
            size: wgpu::Extent3d {
                width: PREVIEW_SIZE,
                height: PREVIEW_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let readback = MappedReadback::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview_readback"),
            size: PREVIEW_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));
        let parameters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview_parameters"),
            size: std::mem::size_of::<PreviewParameters>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Step 3: The mesh pipeline and the target it tests against. A mesh
        // needs its own shader, camera uniform and depth target, because it
        // needs perspective and occlusion where a quad and a sphere need
        // neither.
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview_mesh_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("preview_mesh.wgsl").into()),
        });
        let mesh_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview_mesh_bind_group_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let mesh_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview_mesh_pipeline_layout"),
            bind_group_layouts: &[Some(&mesh_layout)],
            immediate_size: 0,
        });
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview_mesh_pipeline"),
            layout: Some(&mesh_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &mesh_shader,
                entry_point: Some("preview_mesh_vertex"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<crate::assets::MeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &MESH_ATTRIBUTES,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &mesh_shader,
                entry_point: Some("preview_mesh_fragment"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::resources::RendererTexture::DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let mesh_parameters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview_mesh_parameters"),
            size: std::mem::size_of::<MeshParameters>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("preview_depth"),
                size: wgpu::Extent3d {
                    width: PREVIEW_SIZE,
                    height: PREVIEW_SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::resources::RendererTexture::DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());
        // Step 4: The sampler a texture preview reads through, and the white
        // texture a material binds in its place.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("preview_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        // A 1x1 white texture for the sampler slot a material leaves unused.
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview_white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let white = white_texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            layout,
            pipeline,
            mesh_layout,
            mesh_pipeline,
            mesh_parameters,
            depth,
            target,
            view,
            readback,
            parameters,
            sampler,
            white_texture_view: white,
            texture_sources: BudgetedCache::new(TEXTURE_BUDGET_BYTES),
            mesh_sources: BudgetedCache::new(MESH_BUDGET_BYTES),
            in_flight_request_id: None,
            finished_previews: Vec::new(),
        }
    }

    /// Returns whether a request can be drawn this frame: nothing is in flight
    /// and no finished preview is still waiting to be handed over.
    pub(super) fn ready(&self) -> bool {
        self.in_flight_request_id.is_none() && self.finished_previews.is_empty()
    }

    /// Answers a request this renderer will not draw, so the caller stops
    /// waiting for it.
    ///
    /// The refusal goes on the finished list, which [`Self::collect`] hands
    /// over, so the caller gets exactly one answer for every request it made.
    pub(super) fn refuse(&mut self, id: u64) {
        self.finished_previews.push(PreviewImage::refused(id));
    }

    /// Returns the view and sampler a texture preview samples.
    ///
    /// The file is decoded and uploaded on first use. Its modified time is
    /// remembered, and when the file changes it is decoded again, so a
    /// re-imported texture previews its new pixels. Returns `None` when the
    /// file cannot be read or decoded.
    pub(super) fn texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &str,
    ) -> Option<(wgpu::TextureView, wgpu::Sampler)> {
        let modified = std::fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        if let Some(view) = self.texture_sources.fresh_value(path, modified) {
            return Some((view, self.sampler.clone()));
        }
        let decoded = image::open(Path::new(path)).ok()?.to_rgba8();
        let size = wgpu::Extent3d {
            width: decoded.width(),
            height: decoded.height(),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("preview_source"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            decoded.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(decoded.width() * 4),
                rows_per_image: Some(decoded.height()),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bytes = u64::from(size.width) * u64::from(size.height) * 4;
        let dropped = self
            .texture_sources
            .insert(path, modified, view.clone(), bytes);
        self.report_trimmed("source", &dropped);
        Some((view, self.sampler.clone()))
    }

    /// Logs a trim of one cache: how many entries were dropped and how many
    /// bytes the cache holds now.
    fn report_trimmed(&self, cache_name: &str, dropped: &[String]) {
        if dropped.is_empty() {
            return;
        }
        let bytes = match cache_name {
            "mesh" => self.mesh_sources.bytes,
            _ => self.texture_sources.bytes,
        };
        pill_core::info!(
            target: telemetry_target::RENDERING,
            dropped = dropped.len(),
            bytes,
            cache = cache_name,
            "[render] preview cache trimmed"
        );
    }

    /// Records one texture or material preview into `encoder`.
    ///
    /// `source` is the texture to sample. When it is `None`, the shader draws a
    /// sphere in `color` instead. The caller submits the encoder, then calls
    /// [`Self::after_submit`] to start the readback's mapping, and
    /// [`Self::collect`] hands the pixels over.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn record(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        id: u64,
        mode: PreviewMode,
        color: [f32; 4],
        source: Option<(wgpu::TextureView, wgpu::Sampler)>,
    ) {
        // Step 1: Bind the texture the preview samples and write its uniform.
        let (texture_view, sampler) = match source {
            Some((view, sampler)) => (view, sampler),
            None => (self.white_texture_view.clone(), self.sampler.clone()),
        };
        let parameters = PreviewParameters {
            color,
            mode: [mode as u32, 0, 0, 0],
        };
        queue.write_buffer(&self.parameters, 0, bytemuck::bytes_of(&parameters));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview_bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.parameters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        // Step 2: Draw the fullscreen triangle into the target.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        // Step 3: Copy the square out and remember which request is on its way.
        self.copy_target_to_readback(encoder);
        self.in_flight_request_id = Some(id);
    }

    /// Returns the GPU mesh a mesh preview draws.
    ///
    /// The file is decoded and uploaded on first use, and again when its
    /// modified time changes. Returns `None` when the file cannot be read or
    /// decoded, when it holds no vertices or triangles, or when its bounds
    /// leave no room to frame it.
    pub(super) fn mesh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &str,
    ) -> Option<MeshSource> {
        let modified = std::fs::metadata(path)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        if let Some(mesh) = self.mesh_sources.fresh_value(path, modified) {
            return Some(mesh);
        }
        let file_bytes = std::fs::read(Path::new(path)).ok()?;
        let decoded = crate::assets::Mesh::from_obj_bytes(path, &file_bytes).ok()?;
        if decoded.indices.is_empty() || decoded.vertices.is_empty() {
            return None;
        }
        let transform = mesh_framing_transform(&decoded.vertices)?;
        let vertices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview_mesh_vertices"),
            size: std::mem::size_of_val(decoded.vertices.as_slice()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&vertices, 0, bytemuck::cast_slice(&decoded.vertices));
        let indices = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview_mesh_indices"),
            size: std::mem::size_of_val(decoded.indices.as_slice()) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&indices, 0, bytemuck::cast_slice(&decoded.indices));
        let source = MeshSource {
            vertices,
            indices,
            index_count: decoded.indices.len() as u32,
            transform,
        };
        let gpu_bytes = source.vertices.size() + source.indices.size();
        let dropped = self
            .mesh_sources
            .insert(path, modified, source.clone(), gpu_bytes);
        self.report_trimmed("mesh", &dropped);
        Some(source)
    }

    /// Records one mesh preview into `encoder`, then copies the square out for
    /// mapping. The caller submits the encoder, as it does for the other modes.
    pub(super) fn record_mesh(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        id: u64,
        mesh: &MeshSource,
    ) {
        // Step 1: Write the camera uniform the mesh is drawn through.
        let parameters = MeshParameters {
            transform: mesh.transform,
            color: [0.72, 0.73, 0.75, 1.0],
        };
        queue.write_buffer(&self.mesh_parameters, 0, bytemuck::bytes_of(&parameters));
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview_mesh_bind_group"),
            layout: &self.mesh_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.mesh_parameters.as_entire_binding(),
            }],
        });
        // Step 2: Draw the mesh with the depth test on.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview_mesh"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.mesh_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
        // Step 3: Copy the square out and remember which request is on its way.
        self.copy_target_to_readback(encoder);
        self.in_flight_request_id = Some(id);
    }

    /// Copies the target's pixels into the readback buffer. Both preview
    /// paths end with this step.
    fn copy_target_to_readback(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: self.readback.buffer(),
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    // One row of RGBA8 at 128 texels is 512 bytes, which is
                    // already a multiple of the 256-byte alignment the copy
                    // needs, so the rows are tightly packed.
                    bytes_per_row: Some(PREVIEW_SIZE * 4),
                    rows_per_image: Some(PREVIEW_SIZE),
                },
            },
            wgpu::Extent3d {
                width: PREVIEW_SIZE,
                height: PREVIEW_SIZE,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Starts mapping the readback buffer, which the last recorded preview
    /// wrote.
    ///
    /// Call this after the submission that holds the copy. Asking the buffer
    /// to map before its copy is submitted makes the submission fail.
    pub(super) fn after_submit(&mut self) {
        if self.in_flight_request_id.is_none() {
            return;
        }
        self.readback.start();
    }

    /// Returns every preview the GPU has finished, polling without waiting.
    ///
    /// When the readback fails, the request is answered with a refusal
    /// instead. A caller that waits for an answer that never comes stops
    /// asking, which would leave every later preview undrawn.
    pub(super) fn collect(&mut self, device: &wgpu::Device) -> Vec<PreviewImage> {
        if self.in_flight_request_id.is_some() {
            // Poll without waiting: an unfinished readback is answered on a
            // later call.
            let _ = device.poll(wgpu::PollType::Poll);
        }
        let Some(id) = self.in_flight_request_id else {
            return std::mem::take(&mut self.finished_previews);
        };
        let outcome = self.readback.poll(|pixels| PreviewImage {
            id,
            width: PREVIEW_SIZE,
            height: PREVIEW_SIZE,
            rgba: pixels.to_vec(),
        });
        match outcome {
            Readback::Done(image) => {
                self.finished_previews.push(image);
                self.in_flight_request_id = None;
            }
            Readback::Failed => {
                pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    request = id,
                    "[render] preview readback failed; the request is refused"
                );
                self.in_flight_request_id = None;
                self.refuse(id);
            }
            Readback::Pending => {}
        }
        std::mem::take(&mut self.finished_previews)
    }
}

// =============================================================================
// Mesh Framing Transform
// =============================================================================

/// Builds the transform a mesh preview draws through.
///
/// The mesh's own bounds decide where the camera sits, so any asset fills the
/// square.
///
/// The camera looks at the bounds' centre and moves back until their
/// bounding sphere fits the field of view, from a three-quarter angle, so the
/// preview shows a solid shape instead of a flat outline.
///
/// Returns `None` when the vertices have no usable bounds: there are none, all
/// of them sit at one point, or their coordinates are not finite.
fn mesh_framing_transform(vertices: &[crate::assets::MeshVertex]) -> Option<[[f32; 4]; 4]> {
    let mut min = glam::Vec3::splat(f32::INFINITY);
    let mut max = glam::Vec3::splat(f32::NEG_INFINITY);
    for vertex in vertices {
        let position = glam::Vec3::from_array(vertex.position);
        min = min.min(position);
        max = max.max(position);
    }
    if !min.is_finite() || !max.is_finite() || min == max {
        return None;
    }
    let centre = (min + max) * 0.5;
    let radius = ((max - min) * 0.5).length();
    let field_of_view = 45.0_f32.to_radians();
    let distance = radius / (field_of_view * 0.5).tan() * 1.15;
    let eye = centre + glam::Vec3::new(0.45, 0.35, 1.0).normalize() * distance;
    let view = glam::camera::rh::view::look_at_mat4(eye, centre, glam::Vec3::Y);
    // wgpu's clip space takes depth in 0..1 and Y up, which is the DirectX
    // convention glam names.
    let projection = glam::camera::rh::proj::directx::perspective(
        field_of_view,
        1.0,
        (distance - radius * 2.0).max(distance * 0.01),
        distance + radius * 4.0,
    );
    Some((projection * view).to_cols_array_2d())
}

// =============================================================================
// Budgeted Cache
// =============================================================================

/// A cache of GPU data by file path, limited by bytes rather than by a count.
///
/// The preview has two of these, and both hold the same thing: a file's
/// contents on the GPU, which go stale once the file changes. Using an entry
/// makes it the newest, so the limit drops what a person scrolled past instead
/// of what they are looking at. The newest entry is never dropped, even when it
/// is alone over the limit, so a source bigger than the whole cache still
/// previews.
struct BudgetedCache<V> {
    /// The cached values by path, each with the modified time it was built
    /// from.
    cached_entries: HashMap<String, (Option<SystemTime>, V)>,
    /// The paths, oldest first, each with the bytes it holds. This is the
    /// order entries are dropped in.
    eviction_order: VecDeque<(String, u64)>,
    /// The bytes every entry holds together.
    bytes: u64,
    /// How many bytes the cache may hold.
    budget: u64,
}

impl<V: Clone> BudgetedCache<V> {
    /// An empty cache that may hold `budget` bytes.
    fn new(budget: u64) -> Self {
        Self {
            cached_entries: HashMap::new(),
            eviction_order: VecDeque::new(),
            bytes: 0,
            budget,
        }
    }

    /// Returns the cached value for `path` if it was built from `modified`.
    ///
    /// Using the value makes its entry the newest one.
    fn fresh_value(&mut self, path: &str, modified: Option<SystemTime>) -> Option<V> {
        let (cached, value) = self.cached_entries.get(path)?;
        if *cached != modified {
            return None;
        }
        let value = value.clone();
        if let Some(index) = self.eviction_order.iter().position(|(key, _)| key == path) {
            if let Some(entry) = self.eviction_order.remove(index) {
                self.eviction_order.push_back(entry);
            }
        }
        Some(value)
    }

    /// Stores `value` as the newest entry, drops entries until the cache fits
    /// its limit again, and returns the paths it dropped.
    fn insert(
        &mut self,
        path: &str,
        modified: Option<SystemTime>,
        value: V,
        bytes: u64,
    ) -> Vec<String> {
        self.forget(path);
        self.cached_entries
            .insert(path.to_owned(), (modified, value));
        self.eviction_order.push_back((path.to_owned(), bytes));
        self.bytes += bytes;
        let mut dropped = Vec::new();
        while self.bytes > self.budget && self.eviction_order.len() > 1 {
            let Some((path, bytes)) = self.eviction_order.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(bytes);
            self.cached_entries.remove(&path);
            dropped.push(path);
        }
        dropped
    }

    /// Removes `path`'s bytes from the total, for the entry an insert
    /// replaces.
    fn forget(&mut self, path: &str) {
        if let Some(index) = self.eviction_order.iter().position(|(key, _)| key == path) {
            if let Some((_, bytes)) = self.eviction_order.remove(index) {
                self.bytes = self.bytes.saturating_sub(bytes);
            }
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A vertex at `position`, with the fields a preview never reads zeroed.
    fn vertex(position: [f32; 3]) -> crate::assets::MeshVertex {
        crate::assets::MeshVertex {
            position,
            texture_coordinates: [0.0, 0.0],
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 0.0, 1.0],
        }
    }

    /// The corners of an axis-aligned box from `-1` to `1`.
    fn unit_box() -> Vec<crate::assets::MeshVertex> {
        let mut corners = Vec::with_capacity(8);
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    corners.push(vertex([x, y, z]));
                }
            }
        }
        corners
    }

    #[test]
    fn a_mesh_preview_frames_the_whole_mesh_inside_the_square() {
        let matrix = glam::Mat4::from_cols_array_2d(
            &mesh_framing_transform(&unit_box()).expect("a box has bounds to frame"),
        );
        let mut widest: f32 = 0.0;
        for corner in unit_box() {
            let clip = matrix
                * glam::Vec4::new(
                    corner.position[0],
                    corner.position[1],
                    corner.position[2],
                    1.0,
                );
            assert!(clip.w > 0.0, "a corner landed behind the camera");
            let ndc = clip.truncate() / clip.w;
            assert!(
                ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0 && (0.0..=1.0).contains(&ndc.z),
                "a corner fell outside the clip volume: {ndc:?}"
            );
            widest = widest.max(ndc.x.abs().max(ndc.y.abs()));
        }
        assert!(
            widest >= 0.5,
            "the mesh filled only {widest:.2} of the square, so the preview would be mostly empty"
        );
    }

    #[test]
    fn a_mesh_with_no_extent_is_not_drawn() {
        assert!(mesh_framing_transform(&[]).is_none());
        let point = vertex([0.5, 0.5, 0.5]);
        assert!(mesh_framing_transform(&[point, point]).is_none());
        // A vertex with no usable position gives no bound to frame. The other
        // vertices still do, but a mesh made only of such vertices does not.
        assert!(mesh_framing_transform(&[vertex([f32::NAN, 0.0, 0.0])]).is_none());
        let unplaced_then_placed = [vertex([f32::NAN, 0.0, 0.0]), vertex([1.0, 1.0, 1.0])];
        assert!(mesh_framing_transform(&unplaced_then_placed).is_some());
    }

    /// A 4K texture is 64 MB, so browsing a folder of them must not keep every
    /// one resident.
    #[test]
    fn browsing_many_textures_keeps_the_cache_under_its_budget() {
        let texture_bytes = 4096 * 4096 * 4;
        let mut cache = BudgetedCache::new(TEXTURE_BUDGET_BYTES);
        for index in 0..100u32 {
            let path = format!("textures/t{index}.png");
            let dropped = cache.insert(&path, None, index, texture_bytes);
            // The cache drops as it grows, so what it holds never climbs past
            // the budget.
            assert!(
                cache.bytes <= TEXTURE_BUDGET_BYTES || cache.eviction_order.len() == 1,
                "{} bytes held after {index} textures",
                cache.bytes
            );
            assert_eq!(dropped.len(), usize::from(index > 0));
        }
        // The tile a person is on is the one that stays.
        assert_eq!(
            cache.eviction_order.back().map(|(path, _)| path.as_str()),
            Some("textures/t99.png")
        );
        assert_eq!(cache.eviction_order.len(), 1);
        assert_eq!(cache.fresh_value("textures/t99.png", None), Some(99));
    }

    /// A source larger than the whole cache still previews, because it is the
    /// newest entry and the limit never drops that one.
    #[test]
    fn a_source_larger_than_the_budget_is_still_kept() {
        let mut cache = BudgetedCache::new(TEXTURE_BUDGET_BYTES);

        let dropped = cache.insert("huge.png", None, 1, TEXTURE_BUDGET_BYTES * 4);

        assert!(dropped.is_empty());
        assert_eq!(cache.eviction_order.len(), 1);
        assert_eq!(cache.bytes, TEXTURE_BUDGET_BYTES * 4);
        assert_eq!(cache.fresh_value("huge.png", None), Some(1));
    }

    /// The mesh cache drops entries by the same rule. An entry a person is
    /// looking at survives a browse that would otherwise push it out.
    #[test]
    fn a_touched_entry_outlives_the_ones_around_it() {
        let mut cache = BudgetedCache::new(2 * MESH_BUDGET_BYTES);
        cache.insert("meshes/a.obj", None, 1, MESH_BUDGET_BYTES);
        cache.insert("meshes/b.obj", None, 2, MESH_BUDGET_BYTES);
        // Using `a` makes it the newest, so `b` is the one the budget drops.
        assert_eq!(cache.fresh_value("meshes/a.obj", None), Some(1));

        let dropped = cache.insert("meshes/c.obj", None, 3, MESH_BUDGET_BYTES);

        assert_eq!(dropped, vec!["meshes/b.obj".to_owned()]);
        assert_eq!(cache.fresh_value("meshes/a.obj", None), Some(1));
        assert_eq!(cache.fresh_value("meshes/b.obj", None), None);
    }

    /// A file whose modified time moved is not the cached entry: the preview
    /// decodes and uploads it again.
    #[test]
    fn a_file_that_moved_in_time_is_not_fresh() {
        let mut cache = BudgetedCache::new(MESH_BUDGET_BYTES);
        let before = Some(SystemTime::UNIX_EPOCH);
        cache.insert("meshes/a.obj", before, 1, 16);

        assert_eq!(cache.fresh_value("meshes/a.obj", before), Some(1));
        assert_eq!(cache.fresh_value("meshes/a.obj", None), None);
    }
}
