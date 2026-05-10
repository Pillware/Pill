use crate::config::{
    DEFAULT_BRDF_LUT_FALLBACK_PIXEL, DEFAULT_EQUIRECT_FALLBACK_PIXEL, DEFAULT_IBL_FALLBACK_PIXEL,
};
use crate::{
    ecs::{
        CameraComponent, ComponentStorage, PbrRenderableComponent, RenderStateComponent,
        TransformComponent,
    },
    graphics::{
        decompose_render_queue_key, BufferDesc, Pass, PillRenderer, PipelineV2, PipelineV2Desc,
        RendererMaterialHandle, RendererMeshHandle, RendererTextureHandle, ShaderDesc, WorldQuery,
    },
    renderer::resources::{RendererMaterial, RendererMesh},
};
use glam::{Mat3, Mat4, Vec3};
use pill_core::{PillSlotMapKey, Result};
use std::num::NonZeroU32;

/// Camera uniform layout: position (vec4) + view-projection matrix (mat4x4) + fog.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    position: [f32; 4],
    view_projection_matrix: [[f32; 4]; 4],
    fog_color: [f32; 3], // packed with fog_density for 16-byte alignment
    fog_density: f32,
}

impl CameraUniform {
    fn new() -> Self {
        Self {
            position: [0.0; 4],
            view_projection_matrix: Mat4::IDENTITY.to_cols_array_2d(),
            fog_color: [1.0; 3],
            fog_density: 0.0,
        }
    }

    /// Recomputes view and projection from ECS components; called once per frame before GPU upload.
    fn update_data(
        &mut self,
        camera_component: &CameraComponent,
        transform_component: &TransformComponent,
    ) {
        self.position = [
            transform_component.position.x,
            transform_component.position.y,
            transform_component.position.z,
            0.0,
        ];

        let eye = Vec3::new(
            transform_component.position.x,
            transform_component.position.y,
            transform_component.position.z,
        );
        let view = if let Some(t) = camera_component.look_at {
            Mat4::look_at_rh(eye, Vec3::new(t.x, t.y, t.z), Vec3::Y)
        } else {
            let roll_matrix = Mat3::from_rotation_z(transform_component.rotation.z.to_radians());
            let yaw_matrix = Mat3::from_rotation_y(transform_component.rotation.y.to_radians());
            let pitch_matrix = Mat3::from_rotation_x(transform_component.rotation.x.to_radians());
            let rotation_matrix = yaw_matrix * pitch_matrix * roll_matrix;
            let direction = rotation_matrix * Vec3::Z;
            Mat4::look_to_rh(eye, direction, Vec3::Y)
        };

        let proj = Mat4::perspective_rh(
            camera_component.fov.to_radians(),
            camera_component.aspect.get_value(),
            camera_component.range.start,
            camera_component.range.end,
        );

        self.view_projection_matrix = (proj * view).to_cols_array_2d();
    }
}

/// Per-draw storage entry: raw pos/rot/scale (48 B). GPU builds model = T·R·S and MVP.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct PerDrawStd140 {
    position: [f32; 4], // xyz
    rotation: [f32; 4], // xyz, radians
    scale: [f32; 4],    // xyz
}

// Storage buffer (StructuredBuffer / var<storage,read>) indexed by SV_InstanceID replaces
// dynamic-offset UBO: eliminates Metal's 256-B alignment tax and 120k per-instance API calls.
// [Lottes @NOTimothyLottes 2025-01-23; Aaltonen "Modern Mobile Rendering at HypeHype" GDC 2023]
const MAX_EXPECTED_PER_DRAW_INSTANCES: usize = 100_000;
const PER_DRAW_STRIDE_BYTES: usize = std::mem::size_of::<PerDrawStd140>();

pub const MATERIAL_BIND_GROUP_GLOBALS: usize = 0;
pub const MATERIAL_BIND_GROUP_TEXTURES: usize = 1;
pub const MATERIAL_BIND_GROUP_PARAMS: usize = 2;
pub const MATERIAL_BIND_GROUP_PERDRAW: usize = 3;

/// Mesh batch within a material group: same mesh, its per-instance transforms accumulated this
/// frame; `base_offset_u32` is its first index in the concatenated storage buffer.
pub(crate) struct MeshBatch {
    pub(crate) mesh_handle: RendererMeshHandle,
    pub(crate) instances: Vec<PerDrawStd140>,
    pub(crate) base_offset_u32: u32,
}

/// Draw group: one pipeline + one material, containing one or more mesh batches.
pub(crate) struct GroupCmd {
    pub(crate) material_handle: RendererMaterialHandle,
    pub(crate) batches: Vec<MeshBatch>,
}

/// Camera uniform for the background sub-draw (view direction basis + fov).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct BgCameraUbo {
    right: [f32; 3],
    tan_half_fov: f32,
    up: [f32; 3],
    aspect: f32,
    fwd: [f32; 3],
    _pad: f32,
    bg_color: [f32; 3],
    _pad2: f32,
}

/// GPU-side state for the background sub-draw within the PBR render pass.
struct BgSubState {
    pipeline: PipelineV2,
    camera_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    // Equirect handle the current bind_group was built from; rebuild only when it changes.
    cached_bg: RendererTextureHandle,
}

/// GPU-side pass state initialized in `Pass::init`, read every `Pass::draw`.
struct PassPBROpaqueState {
    camera_uniform: CameraUniform,
    camera_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    pipeline: PipelineV2,
    ibl_sampler: wgpu::Sampler,
    prefilter_sampler: wgpu::Sampler,
    per_draw_buffer: wgpu::Buffer,
    per_draw_bind_group: wgpu::BindGroup,
    bg: BgSubState,
    // IBL handles the current globals_bind_group was built from; rebuild only when these change.
    cached_ibl: [RendererTextureHandle; 3],
}

/// Builds the globals bind group (camera UBO + IBL irradiance/prefilter/BRDF-LUT views).
/// Shared by init() and the per-frame rebuild-on-change path in draw().
#[allow(clippy::too_many_arguments)]
fn build_globals_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    camera_buffer: &wgpu::Buffer,
    irradiance_view: &wgpu::TextureView,
    prefilter_view: &wgpu::TextureView,
    brdf_lut_view: &wgpu::TextureView,
    ibl_sampler: &wgpu::Sampler,
    prefilter_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pass_pbr_opaque_globals_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(irradiance_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(ibl_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(prefilter_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(prefilter_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(brdf_lut_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::Sampler(ibl_sampler),
            },
        ],
    })
}

/// Builds the background bind group (camera UBO + equirect view + sampler).
fn build_bg_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    camera_buffer: &wgpu::Buffer,
    equirect_view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pass_pbr_opaque_bg_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(equirect_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// Default PBR pass: GGX shading with IBL, one instanced draw per material/mesh batch.
/// Per-frame state (IBL handles, background equirect, bg_color, fog) is read from
/// RenderStateComponent each frame via WorldQuery::get_global — nothing is baked at construction.
pub struct PassPBROpaque {
    color_target: Option<RendererTextureHandle>,
    depth_texture: Option<RendererTextureHandle>,
    groups_buffer: Vec<GroupCmd>,
    staging_buffer: Vec<u8>,
    state: Option<PassPBROpaqueState>,
}

impl PassPBROpaque {
    /// Creates the pass; `Pass::init` must run before the first frame.
    pub fn new(color_target: Option<RendererTextureHandle>) -> Self {
        Self {
            color_target,
            depth_texture: None,
            groups_buffer: Vec::with_capacity(2000),
            staging_buffer: Vec::with_capacity(
                MAX_EXPECTED_PER_DRAW_INSTANCES * PER_DRAW_STRIDE_BYTES,
            ),
            state: None,
        }
    }
}

/// Returns the initialized pass state; panics in debug if `init` was not called.
fn get_state(pass: &mut PassPBROpaque) -> &mut PassPBROpaqueState {
    debug_assert!(pass.state.is_some());
    pass.state
        .as_mut()
        .expect("PassPBROpaque: state not initialized — call init() before draw()")
}

impl Pass for PassPBROpaque {
    fn get_label(&self) -> &str {
        "pass_pbr_opaque"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        let vertex_wgsl = include_str!("../../res/shaders/pbr_opaque_vertex.wgsl");

        let fragment_wgsl = include_str!("../../res/shaders/pbr_opaque_fragment.wgsl");

        let bind_groups: Vec<Vec<wgpu::BindGroupLayoutEntry>> = vec![
            // 0: globals (camera + IBL irradiance + prefilter + BRDF LUT)
            vec![
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
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
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
            // 1: material textures (base_color, normal, metallic_roughness, emissive)
            vec![
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
            // 2: material params
            vec![wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            // 3: per-draw storage array — no dynamic offset, no 256-B Metal alignment (Apple Metal Spec §4.1.1).
            vec![wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        ];

        let desc = PipelineV2Desc {
            label: Some("pass_pbr_opaque_pipeline"),
            vs: ShaderDesc {
                source: vertex_wgsl,
                entry_func: "vs_main",
            },
            ps: ShaderDesc {
                source: fragment_wgsl,
                entry_func: "fs_main",
            },
            vertex_buffers: &[
                <crate::renderer::resources::RendererMesh as crate::renderer::resources::Vertex>::data_layout_descriptor(),
            ],
            bind_groups,
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba16Float,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
                unclipped_depth: false,
            },
        };

        let pipeline = renderer.create_pipeline_v2(desc)?;

        let camera_buffer = {
            let device = renderer.get_device();
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("pass_pbr_opaque_camera_buffer"),
                size: std::mem::size_of::<CameraUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };

        let per_draw_buffer = renderer.create_buffer(BufferDesc {
            label: Some("pass_pbr_opaque_per_draw"),
            byte_size: (PER_DRAW_STRIDE_BYTES * MAX_EXPECTED_PER_DRAW_INSTANCES) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        })?;
        let per_draw_bind_group = {
            let layout = &pipeline.bind_group_layouts[MATERIAL_BIND_GROUP_PERDRAW];
            let device = renderer.get_device();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("pass_pbr_opaque_per_draw_bind_group"),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: per_draw_buffer.as_entire_binding(),
                }],
            })
        };

        let ibl_sampler = {
            let device = renderer.get_device();
            device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Nearest,
                ..Default::default()
            })
        };

        // Register 1px neutral-gray fallbacks for any missing IBL handle,
        // then get owned views (texture lifetime managed by renderer's pass_textures).
        // Initial bind group is built from neutral fallbacks; draw() rebuilds it from
        // RenderStateComponent's IBL handles the first frame (and whenever they change).
        let irr_h = renderer.create_texture_from_pixels(
            "diffuse_ibl_fallback",
            &[DEFAULT_IBL_FALLBACK_PIXEL],
            1,
            1,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let pre_h = renderer.create_texture_from_pixels(
            "specular_ibl_fallback",
            &[DEFAULT_IBL_FALLBACK_PIXEL],
            1,
            1,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let lut_h = renderer.create_texture_from_pixels(
            "brdf_lut_fallback",
            &[DEFAULT_BRDF_LUT_FALLBACK_PIXEL],
            1,
            1,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let irradiance_view = renderer
            .get_texture_view(irr_h)
            .expect("ibl_diffuse fallback invalid");
        let prefilter_view = renderer
            .get_texture_view(pre_h)
            .expect("ibl_specular fallback invalid");
        let brdf_lut_view = renderer
            .get_texture_view(lut_h)
            .expect("ibl_brdf_lut fallback invalid");

        let prefilter_sampler = {
            let device = renderer.get_device();
            device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Linear,
                lod_min_clamp: 0.0,
                lod_max_clamp: 16.0,
                ..Default::default()
            })
        };

        let globals_bind_group = build_globals_bind_group(
            renderer.get_device(),
            &pipeline.bind_group_layouts[MATERIAL_BIND_GROUP_GLOBALS],
            &camera_buffer,
            &irradiance_view,
            &prefilter_view,
            &brdf_lut_view,
            &ibl_sampler,
            &prefilter_sampler,
        );
        let cached_ibl = [irr_h, pre_h, lut_h];

        self.depth_texture = Some(renderer.create_depth_texture("pass_pbr_opaque_depth")?);

        // Background sub-draw is always created; its equirect comes from RenderStateComponent.background
        // each frame (draw() rebuilds the bind group when the handle changes). Initial bind group uses
        // a 1px fallback equirect.
        let bg = {
            let bg_vs = include_str!("../../res/shaders/background_vertex.wgsl");
            let bg_fs = include_str!("../../res/shaders/background_fragment.wgsl");
            let bg_bind_groups = vec![vec![
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
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ]];
            let bg_pipeline = renderer.create_pipeline_v2(PipelineV2Desc {
                label: Some("pass_pbr_opaque_bg_pipeline"),
                vs: ShaderDesc {
                    source: bg_vs,
                    entry_func: "vs_main",
                },
                ps: ShaderDesc {
                    source: bg_fs,
                    entry_func: "fs_main",
                },
                vertex_buffers: &[],
                bind_groups: bg_bind_groups,
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba16Float,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                // LessEqual + write disabled: fills only pixels where depth == 1.0 (far plane).
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                    unclipped_depth: false,
                },
            })?;
            let equirect_fallback_h = renderer.create_texture_from_pixels(
                "bg_equirect_fallback",
                &[DEFAULT_EQUIRECT_FALLBACK_PIXEL],
                1,
                1,
                wgpu::TextureFormat::Rgba32Float,
            );
            let equirect_view = renderer
                .get_texture_view(equirect_fallback_h)
                .expect("bg equirect fallback invalid");
            let bg_sampler = renderer
                .get_device()
                .create_sampler(&wgpu::SamplerDescriptor {
                    address_mode_u: wgpu::AddressMode::Repeat, // equirect wraps in U
                    address_mode_v: wgpu::AddressMode::ClampToEdge, // poles clamp in V
                    address_mode_w: wgpu::AddressMode::ClampToEdge,
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    mipmap_filter: wgpu::FilterMode::Nearest,
                    ..Default::default()
                });
            let bg_camera_buf = renderer
                .get_device()
                .create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pass_pbr_opaque_bg_camera"),
                    size: std::mem::size_of::<BgCameraUbo>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            let bg_bind_group = build_bg_bind_group(
                renderer.get_device(),
                &bg_pipeline.bind_group_layouts[0],
                &bg_camera_buf,
                &equirect_view,
                &bg_sampler,
            );
            BgSubState {
                pipeline: bg_pipeline,
                camera_buf: bg_camera_buf,
                bind_group: bg_bind_group,
                sampler: bg_sampler,
                cached_bg: equirect_fallback_h,
            }
        };

        self.state = Some(PassPBROpaqueState {
            camera_uniform: CameraUniform::new(),
            camera_buffer,
            globals_bind_group,
            pipeline,
            ibl_sampler,
            prefilter_sampler,
            per_draw_buffer,
            per_draw_bind_group,
            bg,
            cached_ibl,
        });

        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        world: &WorldQuery<'_>,
    ) -> Result<()> {
        // Query per-entity storages + the global render state (bg/IBL/fog) this pass needs.
        let camera_components = world.query::<CameraComponent>()?;
        let transform_components = world.query::<TransformComponent>()?;
        let pbr_renderable_components = world.query::<PbrRenderableComponent>()?;
        let render_state = world.get_global::<RenderStateComponent>()?;

        // Rebuild the IBL globals bind group only when the component's handles change (steady state: 0).
        {
            let ibl = [
                render_state.ibl_diffuse,
                render_state.ibl_specular,
                render_state.ibl_brdf_lut,
            ];
            if get_state(self).cached_ibl != ibl {
                let irradiance_view = renderer
                    .get_texture_view(ibl[0])
                    .expect("ibl_diffuse handle invalid");
                let prefilter_view = renderer
                    .get_texture_view(ibl[1])
                    .expect("ibl_specular handle invalid");
                let brdf_lut_view = renderer
                    .get_texture_view(ibl[2])
                    .expect("ibl_brdf_lut handle invalid");
                let device = renderer.get_device();
                let state = get_state(self);
                state.globals_bind_group = build_globals_bind_group(
                    device,
                    &state.pipeline.bind_group_layouts[MATERIAL_BIND_GROUP_GLOBALS],
                    &state.camera_buffer,
                    &irradiance_view,
                    &prefilter_view,
                    &brdf_lut_view,
                    &state.ibl_sampler,
                    &state.prefilter_sampler,
                );
                state.cached_ibl = ibl;
            }
        }

        // Rebuild the background bind group only when the component's equirect handle changes.
        {
            let bg_h = render_state.background;
            if get_state(self).bg.cached_bg != bg_h {
                let equirect_view = renderer
                    .get_texture_view(bg_h)
                    .expect("background handle invalid");
                let device = renderer.get_device();
                let state = get_state(self);
                state.bg.bind_group = build_bg_bind_group(
                    device,
                    &state.bg.pipeline.bind_group_layouts[0],
                    &state.bg.camera_buf,
                    &equirect_view,
                    &state.bg.sampler,
                );
                state.bg.cached_bg = bg_h;
            }
        }

        // Read active camera and transform.
        let active_camera_index = world.active_camera.data().index as usize;
        let active_camera_component = camera_components
            .data
            .get(active_camera_index)
            .unwrap()
            .as_ref()
            .unwrap();
        let active_camera_transform = transform_components
            .data
            .get(active_camera_index)
            .unwrap()
            .as_ref()
            .unwrap();

        // Update camera uniform and write to GPU buffer. MVP = viewProjection·model is computed
        // in the vertex shader, so the CPU never multiplies per-entity matrices.
        // Fog fades to the background color (no separate fog_color); read from the component.
        let fog_color = render_state.bg_color;
        let fog_density = render_state.fog_density;
        {
            let state = get_state(self);
            state
                .camera_uniform
                .update_data(active_camera_component, active_camera_transform);
            state.camera_uniform.fog_color = fog_color;
            state.camera_uniform.fog_density = fog_density;
            renderer.get_queue().write_buffer(
                &state.camera_buffer,
                0,
                bytemuck::bytes_of(&state.camera_uniform),
            );
        }

        // Data-driven: this pass builds its OWN opaque draw list by querying the world. Iterate the
        // contiguous transform + pbr_renderable arrays, group by material+mesh (find-or-create, since
        // ECS order is unsorted) to minimize bind-group/vertex-buffer switches — the grouping exists
        // to cut GPU state changes, not for visual order. Group order is irrelevant (opaque, depth-
        // tested). [Aaltonen "HypeHype" GDC 2023]
        for group in self.groups_buffer.iter_mut() {
            for batch in group.batches.iter_mut() {
                batch.instances.clear(); // reuse capacity across frames
            }
        }
        let transforms = &transform_components.data;
        let pbrs = &pbr_renderable_components.data;
        for (slot_transform, slot_pbr) in transforms.iter().zip(pbrs.iter()) {
            let Some(transform) = slot_transform else {
                continue;
            };
            let Some(pbr) = slot_pbr else {
                continue;
            };
            let Some(key) = pbr.render_queue_key else {
                continue;
            };
            let key_fields = decompose_render_queue_key(key);
            let mesh_handle = RendererMeshHandle::new(
                key_fields.mesh_index.into(),
                NonZeroU32::new(key_fields.mesh_version.into()).unwrap(),
            );
            let material_handle = RendererMaterialHandle::new(
                key_fields.material_index.into(),
                NonZeroU32::new(key_fields.material_version.into()).unwrap(),
            );
            let per_draw = PerDrawStd140 {
                position: [
                    transform.position.x,
                    transform.position.y,
                    transform.position.z,
                    0.0,
                ],
                rotation: [
                    transform.rotation.x,
                    transform.rotation.y,
                    transform.rotation.z,
                    0.0,
                ],
                scale: [transform.scale.x, transform.scale.y, transform.scale.z, 0.0],
            };
            let group_index = match self
                .groups_buffer
                .iter()
                .position(|group| group.material_handle == material_handle)
            {
                Some(index) => index,
                None => {
                    self.groups_buffer.push(GroupCmd {
                        material_handle,
                        batches: Vec::new(),
                    });
                    self.groups_buffer.len() - 1
                }
            };
            let group = &mut self.groups_buffer[group_index];
            match group
                .batches
                .iter_mut()
                .find(|batch| batch.mesh_handle == mesh_handle)
            {
                Some(batch) => batch.instances.push(per_draw),
                None => group.batches.push(MeshBatch {
                    mesh_handle,
                    instances: vec![per_draw],
                    base_offset_u32: 0,
                }),
            }
        }

        // Concatenate per-batch instances into the storage buffer (sequential — cache-friendly) and
        // assign each batch its first-instance offset. Single upload per pass. [Aaltonen GDC 2023]
        self.staging_buffer.clear();
        let mut next_instance: u32 = 0;
        for group in self.groups_buffer.iter_mut() {
            for batch in group.batches.iter_mut() {
                batch.base_offset_u32 = next_instance;
                for per_draw in &batch.instances {
                    self.staging_buffer
                        .extend_from_slice(bytemuck::bytes_of(per_draw));
                    next_instance = next_instance.wrapping_add(1);
                }
            }
        }
        if next_instance as usize > MAX_EXPECTED_PER_DRAW_INSTANCES {
            log::error!(
                "PassPBROpaque: per-draw capacity exceeded (needed={}, capacity={})",
                next_instance,
                MAX_EXPECTED_PER_DRAW_INSTANCES
            );
        }
        {
            let state_ref = self
                .state
                .as_ref()
                .expect("PassPBROpaque: state not initialized — call init() before draw()");
            renderer
                .get_queue()
                .write_buffer(&state_ref.per_draw_buffer, 0, &self.staging_buffer);
        }

        let depth_view = renderer
            .get_render_target_view(self.depth_texture.unwrap())
            .unwrap();
        let color_view = self
            .color_target
            .and_then(|h| renderer.get_render_target_view(h))
            .unwrap_or(view);

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pass_pbr_opaque_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        let state_ref = self
            .state
            .as_ref()
            .expect("PassPBROpaque: state not initialized — call init() before draw()");
        render_pass.set_pipeline(&state_ref.pipeline.pipeline);
        // Per-draw array bound once; instance_index selects per-entity data — 5 draws instead of 60k.
        // [Lottes @NOTimothyLottes 2025-01-23; WebGPU W3C 2024 §drawIndexed firstInstance]
        render_pass.set_bind_group(
            MATERIAL_BIND_GROUP_PERDRAW as u32,
            &state_ref.per_draw_bind_group,
            &[],
        );
        for group in &self.groups_buffer {
            render_pass.set_bind_group(
                MATERIAL_BIND_GROUP_GLOBALS as u32,
                &state_ref.globals_bind_group,
                &[],
            );

            let mat = world
                .resources
                .get_resource::<RendererMaterial>(&group.material_handle)
                .expect("PassPBROpaque: RendererMaterial missing for draw group");

            // Skip materials that don't have PBR-compatible bind groups.
            let (Some(textures_bg), Some(params_bg)) = (
                mat.textures_bind_group.as_ref(),
                mat.parameters_bind_group.as_ref(),
            ) else {
                continue;
            };
            render_pass.set_bind_group(MATERIAL_BIND_GROUP_TEXTURES as u32, textures_bg, &[]);
            render_pass.set_bind_group(MATERIAL_BIND_GROUP_PARAMS as u32, params_bg, &[]);

            for batch in &group.batches {
                let mesh = world
                    .resources
                    .get_resource::<RendererMesh>(&batch.mesh_handle)
                    .expect("PassPBROpaque: RendererMesh missing for batch");
                render_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                render_pass
                    .set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                let first = batch.base_offset_u32;
                let count = batch.instances.len() as u32;
                if count == 0 {
                    continue; // material/mesh with no instances this frame
                }
                // One instanced draw per batch: instance_index in [first, first+count) selects per-draw data.
                render_pass.draw_indexed(0..mesh.index_count, 0, first..(first + count));
            }
        }

        // Background sub-draw: within the same render pass, after opaque geometry.
        // LessEqual depth test + write_enabled=false: fills only pixels where depth == 1.0 (far plane = no geometry).
        {
            let bg = &state_ref.bg;
            let eye = Vec3::new(
                active_camera_transform.position.x,
                active_camera_transform.position.y,
                active_camera_transform.position.z,
            );
            let fwd = if let Some(t) = active_camera_component.look_at {
                (Vec3::new(t.x, t.y, t.z) - eye).normalize()
            } else {
                let roll = Mat3::from_rotation_z(active_camera_transform.rotation.z.to_radians());
                let yaw = Mat3::from_rotation_y(active_camera_transform.rotation.y.to_radians());
                let pitch = Mat3::from_rotation_x(active_camera_transform.rotation.x.to_radians());
                (yaw * pitch * roll) * Vec3::Z
            };
            let right = fwd.cross(Vec3::Y).normalize();
            let up = right.cross(fwd);
            let ubo = BgCameraUbo {
                right: right.to_array(),
                tan_half_fov: (active_camera_component.fov.to_radians() / 2.0).tan(),
                up: up.to_array(),
                aspect: active_camera_component.aspect.get_value(),
                fwd: fwd.to_array(),
                _pad: 0.0,
                bg_color: render_state.bg_color,
                _pad2: 0.0,
            };
            renderer
                .get_queue()
                .write_buffer(&bg.camera_buf, 0, bytemuck::bytes_of(&ubo));
            render_pass.set_pipeline(&bg.pipeline.pipeline);
            render_pass.set_bind_group(0, &bg.bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }

        Ok(())
    }
}
