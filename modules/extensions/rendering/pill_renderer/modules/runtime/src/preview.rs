//! Asset previews: one small off-screen render, read back as pixels.
//!
//! # Responsibilities
//!
//! - Render one requested asset into a 128x128 RGBA8 target and copy it to a
//!   readback buffer ([`PreviewRenderer::record_texture`] for a texture; a
//!   material or a mesh is drawn by the renderer's own chain into the same
//!   target). At most one request is drawn per frame.
//! - Map that buffer after the frame that wrote it was submitted
//!   ([`PreviewRenderer::after_submit`]). Hand the pixels back once the GPU
//!   has finished ([`PreviewRenderer::collect`]).
//! - Decode and upload a texture file the first time a preview names it, and
//!   again when the file changes ([`PreviewRenderer::texture`]).
//! - Supply the sphere a material preview draws on ([`sphere_mesh`]) and the
//!   camera each preview draws through: one framed to a mesh's own bounds
//!   ([`mesh_framing`]), one framed to that sphere ([`sphere_framing`]).
//!
//! # Design
//!
//! A texture preview is one fullscreen triangle: the fragment shader samples the
//! decoded file, which is what an editor tile needs for a picture that is only a
//! file. It needs no vertex buffer, camera or depth target.
//!
//! A material or a mesh preview is drawn by the renderer's chain instead: the
//! asset becomes one instance of the frame and the geometry pass draws it
//! through the pipeline its material's shader carries - a material on the
//! generated sphere, a mesh with the default material and a camera fitted to its
//! bounds. A preview therefore shows the asset the way the game draws it, and a
//! material preview changes when the material or its shader does.
//!
//! The target, the texture pipeline and the readback are built once and reused.
//! The readback is a mapped buffer polled without waiting, the same shape the
//! pick buffer uses. Nothing is written to disk: the pixels reach the editor as
//! data.

// Standard library
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::SystemTime;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::common_components::TransformComponent;
use pill_renderer_api::editor::PreviewImage;

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

/// The name the renderer installs its preview sphere under.
///
/// The sphere is an asset like any other, so the chain draws a material on it
/// exactly as it draws a game's meshes: one instance, through its own material.
pub(super) const SPHERE_MESH_NAME: &str = "pill.preview.sphere";

/// The sphere a material preview draws on: how many segments around it, and how
/// many rings from pole to pole.
const SPHERE_SEGMENTS: u32 = 32;
const SPHERE_RINGS: u32 = 16;

/// How wide the preview camera's field of view is, in radians.
///
/// A preview is square, so the vertical and horizontal angles are the same.
const PREVIEW_FIELD_OF_VIEW: f32 = std::f32::consts::FRAC_PI_4;

// =============================================================================
// Preview Sphere
// =============================================================================

/// Builds the unit sphere a material preview draws on.
///
/// The sphere carries positions, normals and texture coordinates, so any shader
/// that reads those three draws it: a material whose shader samples maps shows
/// them wrapped around the ball. Its triangles wind the way the chain's culling
/// expects, so the faces turned away from the camera are the ones dropped.
pub(super) fn sphere_mesh() -> crate::assets::Mesh {
    let mut vertices = Vec::with_capacity(((SPHERE_RINGS + 1) * (SPHERE_SEGMENTS + 1)) as usize);
    for ring in 0..=SPHERE_RINGS {
        let polar = std::f32::consts::PI * (ring as f32 / SPHERE_RINGS as f32);
        let (sin_polar, cos_polar) = polar.sin_cos();
        for segment in 0..=SPHERE_SEGMENTS {
            let azimuth = std::f32::consts::TAU * (segment as f32 / SPHERE_SEGMENTS as f32);
            let (sin_azimuth, cos_azimuth) = azimuth.sin_cos();
            let normal = [sin_polar * cos_azimuth, cos_polar, sin_polar * sin_azimuth];
            vertices.push(crate::assets::MeshVertex {
                position: normal,
                texture_coordinates: [
                    segment as f32 / SPHERE_SEGMENTS as f32,
                    ring as f32 / SPHERE_RINGS as f32,
                ],
                normal,
                tangent: [-sin_azimuth, 0.0, cos_azimuth],
                bitangent: [cos_polar * cos_azimuth, -sin_polar, cos_polar * sin_azimuth],
            });
        }
    }
    let stride = SPHERE_SEGMENTS + 1;
    let mut indices = Vec::with_capacity((SPHERE_RINGS * SPHERE_SEGMENTS * 6) as usize);
    for ring in 0..SPHERE_RINGS {
        for segment in 0..SPHERE_SEGMENTS {
            let top = ring * stride + segment;
            let bottom = top + stride;
            // Wound so that the face turned toward the camera - the one the
            // chain keeps when it culls back faces - is the outside of the
            // ball. Swapping the two corners of a triangle flips which side is
            // the front, so the order here is the whole of it.
            indices.extend_from_slice(&[top, top + 1, bottom]);
            indices.extend_from_slice(&[top + 1, bottom + 1, bottom]);
        }
    }
    crate::assets::Mesh::from_data(SPHERE_MESH_NAME, vertices, indices)
}

// =============================================================================
// Preview Material
// =============================================================================

/// The name the renderer installs the material a mesh preview draws with under.
///
/// A bare mesh has no material of its own, and the chain's default one is a
/// mirror - metal with nothing behind the camera reads as a flat dark shape -
/// so a preview of a mesh gets a surface that shows its form instead.
pub(super) const MESH_MATERIAL_NAME: &str = "pill.preview.material";

/// The base colour of the surface a mesh preview draws.
const PREVIEW_SURFACE_COLOR: [f32; 3] = [0.8, 0.8, 0.8];

/// How rough that surface is: dull enough to show shading, smooth enough to
/// keep a highlight that follows the shape.
const PREVIEW_SURFACE_ROUGHNESS: f32 = 0.55;

/// Builds the material a mesh preview draws with.
///
/// It names no shader, so it draws through the shader the chain would draw a
/// material that names none with - a project with its own chain sees its own
/// shading, exactly as its scenes do. Its parameters are the slots the shipped
/// PBR shader reads; a chain whose shader declares others ignores them.
pub(super) fn mesh_preview_material() -> crate::assets::Material {
    crate::assets::Material::builder(MESH_MATERIAL_NAME)
        .color_parameter("pbr_base", PREVIEW_SURFACE_COLOR)
        .scalar_parameter("pbr_roughness", PREVIEW_SURFACE_ROUGHNESS)
        .scalar_parameter("pbr_metallic", 0.0)
        .color_parameter("pbr_emissive", [0.0, 0.0, 0.0])
        .build()
}

// =============================================================================
// Mesh Framing
// =============================================================================

/// Where a preview's camera stands and what it draws through.
///
/// The camera looks at the centre of the object's bounds from a three-quarter
/// angle and stands back until the bounding sphere fits the field of view, so
/// any asset fills the tile and a solid reads as a solid rather than as a flat
/// outline. A material preview frames the unit sphere the same way.
pub(super) struct PreviewFraming {
    /// The camera to draw through.
    pub(crate) camera: pill_renderer_api::CameraComponent,
    /// Where it stands and which way it looks - the camera's own transform,
    /// never the drawn object's.
    pub(crate) transform: TransformComponent,
}

/// Frames a sphere of `radius` around `centre`, or returns `None` when the
/// bounds are unusable: a radius that is zero or not finite, or a centre that
/// is not finite.
pub(super) fn framing_for_bounds(centre: glam::Vec3, radius: f32) -> Option<PreviewFraming> {
    if !centre.is_finite() || !radius.is_finite() || radius <= 0.0 {
        return None;
    }
    let distance = radius / (PREVIEW_FIELD_OF_VIEW * 0.5).tan() * 1.15;
    let direction = glam::Vec3::new(0.45, 0.35, 1.0).normalize();
    let eye = centre + direction * distance;
    // The camera looks down its own -Z, so the rotation that aims it at the
    // object takes -Z onto the direction from the object to the eye.
    let rotation = glam::Quat::from_rotation_arc(glam::Vec3::NEG_Z, -direction);
    let camera = pill_renderer_api::CameraComponent {
        vertical_fov: PREVIEW_FIELD_OF_VIEW.to_degrees(),
        near: (distance - radius * 2.0).max(distance * 0.01),
        far: distance + radius * 4.0,
        ..Default::default()
    };
    Some(PreviewFraming {
        camera,
        transform: TransformComponent {
            translation: [eye.x, eye.y, eye.z],
            rotation: [rotation.x, rotation.y, rotation.z, rotation.w],
            scale: [1.0, 1.0, 1.0],
        },
    })
}

/// The framing a material preview draws through: the unit sphere at the
/// origin, whose bounds are known before a single vertex is read.
pub(super) fn sphere_framing() -> PreviewFraming {
    framing_for_bounds(glam::Vec3::ZERO, 1.0).expect("a unit sphere has usable bounds")
}

/// Frames `vertices` with a camera, or returns `None` when the mesh has no
/// usable bounds: no vertices, all of them at one point, or coordinates that are
/// not finite.
pub(super) fn mesh_framing(vertices: &[crate::assets::MeshVertex]) -> Option<PreviewFraming> {
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
    framing_for_bounds((min + max) * 0.5, ((max - min) * 0.5).length())
}

// =============================================================================
// Preview Renderer
// =============================================================================

/// The GPU state one preview at a time needs: a 128x128 target, the texture
/// pipeline that draws into it, a depth buffer for the assets the chain draws,
/// and one readback slot.
pub(super) struct PreviewRenderer {
    /// The texture preview's bind group layout and pipeline: a texture and its
    /// sampler.
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    /// The 128x128 render target every preview draws into, and the view of it.
    target: wgpu::Texture,
    view: wgpu::TextureView,
    /// The depth buffer an asset drawn by the chain tests against.
    depth: wgpu::TextureView,
    /// Where the drawn pixels are copied for mapping.
    readback: MappedReadback,
    /// The sampler a texture preview reads through.
    sampler: wgpu::Sampler,
    /// Texture views by path, with the modified time each was decoded from.
    texture_sources: BudgetedCache<wgpu::TextureView>,
    /// The id of the request whose pixels are on their way.
    in_flight_request_id: Option<u64>,
    /// Finished previews waiting for the next [`Self::collect`].
    finished_previews: Vec<PreviewImage>,
}

impl PreviewRenderer {
    /// Builds the target, the depth buffer, the texture pipeline, the readback
    /// buffer and the decode cache.
    pub(super) fn new(device: &wgpu::Device) -> Self {
        // Step 1: The texture pipeline - one fullscreen triangle with a texture
        // and a sampler.
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
        // Step 2: The target every preview draws into, the depth buffer an
        // asset drawn by the chain tests against, and the buffer the pixels are
        // copied out to.
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
        let readback = MappedReadback::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("preview_readback"),
            size: PREVIEW_BYTES,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        }));
        // Step 3: The sampler a texture preview reads through.
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
        Self {
            layout,
            pipeline,
            target,
            view,
            depth,
            readback,
            sampler,
            texture_sources: BudgetedCache::new(TEXTURE_BUDGET_BYTES),
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

    /// The view a preview draws into.
    pub(super) fn target_view(&self) -> &wgpu::TextureView {
        &self.view
    }

    /// The depth buffer an asset drawn by the chain tests against.
    pub(super) fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth
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
        self.report_trimmed(&dropped);
        Some((view, self.sampler.clone()))
    }

    /// Logs a trim of the decode cache: how many entries were dropped and how
    /// many bytes it holds now.
    fn report_trimmed(&self, dropped: &[String]) {
        if dropped.is_empty() {
            return;
        }
        pill_core::info!(
            target: telemetry_target::RENDERING,
            dropped = dropped.len(),
            bytes = self.texture_sources.bytes,
            cache = "source",
            "[render] preview cache trimmed"
        );
    }

    /// Records one texture preview into `encoder`.
    ///
    /// The caller submits the encoder, then calls [`Self::after_submit`] to
    /// start the readback's mapping, and [`Self::collect`] hands the pixels
    /// over.
    pub(super) fn record_texture(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        id: u64,
        source: (wgpu::TextureView, wgpu::Sampler),
    ) {
        // Step 1: Bind the texture the preview samples.
        let (texture_view, sampler) = source;
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("preview_bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
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

    /// Copies the target's pixels into the readback buffer. Every preview path
    /// ends with this step.
    pub(super) fn copy_target_to_readback(&self, encoder: &mut wgpu::CommandEncoder) {
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

    /// Remembers that `id` is the request whose pixels the next readback
    /// carries.
    pub(super) fn mark_in_flight(&mut self, id: u64) {
        self.in_flight_request_id = Some(id);
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
// Budgeted Cache
// =============================================================================

/// A cache of GPU data by file path, limited by bytes rather than by a count.
///
/// It holds a file's contents on the GPU, which go stale once the file changes.
/// Using an entry makes it the newest, so the limit drops what a person scrolled
/// past instead of what they are looking at. The newest entry is never dropped,
/// even when it is alone over the limit, so a source bigger than the whole cache
/// still previews.
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

    /// The sphere a material preview draws on is a closed, unit-sized ball with
    /// the attributes a material's shader reads.
    #[test]
    fn the_preview_sphere_is_a_unit_ball_with_its_normals() {
        let mesh = sphere_mesh();
        assert!(!mesh.vertices.is_empty() && !mesh.indices.is_empty());
        assert_eq!(mesh.indices.len() % 3, 0);
        for vertex in &mesh.vertices {
            let length = glam::Vec3::from_array(vertex.position).length();
            assert!(
                (length - 1.0).abs() < 1e-4,
                "a sphere vertex sits {length} from the centre"
            );
        }
        assert!(
            mesh.indices
                .iter()
                .all(|index| (*index as usize) < mesh.vertices.len()),
            "an index points past the vertices"
        );
    }

    /// Every triangle of the preview sphere turns its front toward the outside
    /// of the ball, so the chain's back-face culling drops the far side and the
    /// camera sees the near side. Wound the other way round, the sphere's
    /// visible pixels would be the inside of the far side, lit by nothing.
    #[test]
    fn the_preview_spheres_faces_turn_outward() {
        let mesh = sphere_mesh();
        let mut checked = 0;
        for triangle in mesh.indices.chunks_exact(3) {
            let corner = |index: usize| {
                glam::Vec3::from_array(mesh.vertices[triangle[index] as usize].position)
            };
            let (first, second, third) = (corner(0), corner(1), corner(2));
            let face_normal = (second - first).cross(third - first);
            if face_normal.length() < 1e-6 {
                // The quad touching a pole has one triangle of zero area,
                // because a pole vertex and the one beside it are the same
                // point.
                continue;
            }
            let centroid = (first + second + third) / 3.0;
            assert!(
                face_normal.dot(centroid) > 0.0,
                "a face at {centroid} turns away from the ball: {face_normal}"
            );
            checked += 1;
        }
        assert!(
            checked >= (SPHERE_RINGS * SPHERE_SEGMENTS * 2 - SPHERE_SEGMENTS * 2) as usize,
            "only {checked} faces had any surface to check"
        );
    }

    /// The material a mesh preview draws with is a mid-grey dielectric, not the
    /// chain's mirror-like default: metal with no environment behind it takes
    /// no diffuse light and reads as an unlit blob.
    #[test]
    fn the_mesh_preview_material_is_a_neutral_dielectric() {
        let material = mesh_preview_material();
        assert_eq!(material.name, MESH_MATERIAL_NAME);
        assert_eq!(
            material.shader,
            pill_engine::Handle::INVALID,
            "the chain's own shader is what draws it"
        );
        assert_eq!(
            material.parameters.get("pbr_base"),
            Some(&crate::assets::MaterialParameter::Color(
                PREVIEW_SURFACE_COLOR
            ))
        );
        assert_eq!(
            material.parameters.get("pbr_metallic"),
            Some(&crate::assets::MaterialParameter::Scalar(0.0)),
            "a dielectric shows shading; a metal shows only the lights"
        );
        assert_eq!(
            material.parameters.get("pbr_emissive"),
            Some(&crate::assets::MaterialParameter::Color([0.0, 0.0, 0.0])),
            "a preview surface gives off no light of its own"
        );
    }

    /// A material preview frames the unit sphere it draws on, from outside it:
    /// the eye stands clear of the ball, looks at its centre, and the ball fits
    /// inside the field of view.
    #[test]
    fn the_material_preview_frames_the_unit_sphere_from_outside() {
        let framing = sphere_framing();
        let eye = glam::Vec3::from(framing.transform.translation);
        let distance = eye.length();
        assert!(
            distance > 1.5,
            "the eye must stand clear of the unit sphere, not inside it: {distance}"
        );
        let rotation = glam::Quat::from_array(framing.transform.rotation);
        let forward = rotation * glam::Vec3::NEG_Z;
        assert!(
            (eye + forward * distance).length() < 1e-3,
            "the camera must look at the sphere's centre"
        );
        assert!(
            framing.camera.near < distance - 1.0,
            "the sphere's near side must be past the near plane"
        );
        let half_angle = (framing.camera.vertical_fov * 0.5).to_radians();
        assert!(
            (1.0 / distance).asin() < half_angle,
            "the whole ball must fit inside the field of view"
        );
    }

    /// A mesh preview's camera frames the whole mesh inside the square.
    #[test]
    fn a_mesh_preview_frames_the_whole_mesh_inside_the_square() {
        let framing = mesh_framing(&unit_box()).expect("a box has bounds to frame");
        let eye = glam::Vec3::from(framing.transform.translation);
        let rotation = glam::Quat::from_array(framing.transform.rotation);
        let view = glam::Mat4::from_rotation_translation(rotation, eye).inverse();
        let projection = glam::camera::rh::proj::directx::perspective(
            PREVIEW_FIELD_OF_VIEW,
            1.0,
            framing.camera.near,
            framing.camera.far,
        );
        let matrix = projection * view;
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
        assert!(mesh_framing(&[]).is_none());
        let point = vertex([0.5, 0.5, 0.5]);
        assert!(mesh_framing(&[point, point]).is_none());
        // A vertex with no usable position gives no bound to frame. The other
        // vertices still do, but a mesh made only of such vertices does not.
        assert!(mesh_framing(&[vertex([f32::NAN, 0.0, 0.0])]).is_none());
        let unplaced_then_placed = [vertex([f32::NAN, 0.0, 0.0]), vertex([1.0, 1.0, 1.0])];
        assert!(mesh_framing(&unplaced_then_placed).is_some());
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

    /// An entry a person is looking at survives a browse that would otherwise
    /// push it out.
    #[test]
    fn a_touched_entry_outlives_the_ones_around_it() {
        let mut cache = BudgetedCache::new(2 * TEXTURE_BUDGET_BYTES);
        cache.insert("textures/a.png", None, 1, TEXTURE_BUDGET_BYTES);
        cache.insert("textures/b.png", None, 2, TEXTURE_BUDGET_BYTES);
        // Using `a` makes it the newest, so `b` is the one the budget drops.
        assert_eq!(cache.fresh_value("textures/a.png", None), Some(1));

        let dropped = cache.insert("textures/c.png", None, 3, TEXTURE_BUDGET_BYTES);

        assert_eq!(dropped, vec!["textures/b.png".to_owned()]);
        assert_eq!(cache.fresh_value("textures/a.png", None), Some(1));
        assert_eq!(cache.fresh_value("textures/b.png", None), None);
    }

    /// A file whose modified time moved is not the cached entry: the preview
    /// decodes and uploads it again.
    #[test]
    fn a_file_that_moved_in_time_is_not_fresh() {
        let mut cache = BudgetedCache::new(TEXTURE_BUDGET_BYTES);
        let before = Some(SystemTime::UNIX_EPOCH);
        cache.insert("textures/a.png", before, 1, 16);

        assert_eq!(cache.fresh_value("textures/a.png", before), Some(1));
        assert_eq!(cache.fresh_value("textures/a.png", None), None);
    }
}
