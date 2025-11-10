use crate::renderer::{Pass, Renderer, WorldQuery};
use crate::resource_manager::ResourceManager;
use crate::resources::CameraUniform;
use crate::resources::RendererTexture;
use std::sync::Arc;

use anyhow::{Error, Result};
use glam::{Mat4, Vec3, Vec4};
use pill_core::{PillSlotMapKey, RendererError};
use pill_engine::internal::{
    get_renderer_resource_handle_from_camera_component, CameraComponent, ComponentStorage,
    EntityHandle, MaterialDesc, RenderQueueItem, RendererMaterialHandle, RendererMeshHandle,
    TransformComponent,
};
use pill_engine::internal::{BufferDesc, PillRenderer, PipelineV2, PipelineV2Desc, ShaderDesc};
use wgpu::util::DeviceExt;
use wgpu::CommandEncoder;

#[repr(C)]
#[derive(Copy, Clone)]
pub(crate) struct PerDrawStd140 {
    mvp: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
}
unsafe impl bytemuck::Zeroable for PerDrawStd140 {}
unsafe impl bytemuck::Pod for PerDrawStd140 {}

#[repr(C)]
#[derive(Copy, Clone)]
struct MaterialParamsStd140 {
    base_color_factor: [f32; 3],
    _pad0: f32,
    metallic: f32,
    roughness: f32,
    _pad1: [f32; 2],
    emissive_factor: [f32; 3],
    _pad2: f32,
}
unsafe impl bytemuck::Zeroable for MaterialParamsStd140 {}
unsafe impl bytemuck::Pod for MaterialParamsStd140 {}

// Static configuration for per-draw buffering (derive capacities, avoid magic numbers)
const MAX_EXPECTED_PER_DRAW_INSTANCES: usize = 100_000;
const UNIFORM_OFFSET_ALIGNMENT: usize = 256;
const PER_DRAW_STRIDE_BYTES: usize = ((std::mem::size_of::<PerDrawStd140>()
    + (UNIFORM_OFFSET_ALIGNMENT - 1))
    / UNIFORM_OFFSET_ALIGNMENT)
    * UNIFORM_OFFSET_ALIGNMENT;

pub const MATERIAL_BIND_GROUP_GLOBALS: usize = 0;
pub const MATERIAL_BIND_GROUP_TEXTURES: usize = 1;
pub const MATERIAL_BIND_GROUP_PARAMS: usize = 2;
pub const MATERIAL_BIND_GROUP_PERDRAW: usize = 3;

// Preallocated buffer structs for hot path optimization
pub(crate) struct VisiblePreDraw {
    pub(crate) material_handle: RendererMaterialHandle,
    pub(crate) mesh_handle: RendererMeshHandle,
    pub(crate) entity_index: u32,
    pub(crate) mvp: [[f32; 4]; 4],
    pub(crate) model: [[f32; 4]; 4],
}

// M3: Hello Mesh + per-draw MVP (dynamic offsets)
pub(crate) struct MeshBatch {
    pub(crate) mesh_handle: RendererMeshHandle,
    pub(crate) instances: Vec<PerDrawStd140>,
    pub(crate) base_offset_u32: u32, // offset into per-draw ring for first instance
}

pub(crate) struct GroupCmd {
    pub(crate) pipeline: *const wgpu::RenderPipeline,
    pub(crate) material_handle: RendererMaterialHandle,
    pub(crate) batches: Vec<MeshBatch>,
}

pub struct PassScene {
    label: String,
    // Resources
    offscreen_color_texture: Arc<RendererTexture>,
    depth_texture: Arc<RendererTexture>,
    // Per-frame dynamic UBO ring (Milestone 5)
    per_draw_stride: u64,
    per_draw_capacity: u64,
    // Working buffers
    visible_pre_draw_buffer: Vec<VisiblePreDraw>,
    groups_buffer: Vec<GroupCmd>,
    staging_buffer: Vec<u8>,
    // Pass-local state initialized in init(), read every draw
    state: Option<PassSceneState>,
}

struct PassSceneState {
    camera_uniform: CameraUniform,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    pipeline: PipelineV2,
    // Per-draw resources
    per_draw_buffer: wgpu::Buffer,
    per_draw_bind_group: wgpu::BindGroup,
}

impl PassScene {
    pub fn new(
        label: &str,
        offscreen_color_texture: Arc<RendererTexture>,
        depth_texture: Arc<RendererTexture>,
    ) -> Self {
        Self {
            label: label.to_string(),
            offscreen_color_texture: offscreen_color_texture,
            depth_texture: depth_texture,
            per_draw_stride: PER_DRAW_STRIDE_BYTES as u64,
            per_draw_capacity: 0,
            visible_pre_draw_buffer: Vec::with_capacity(MAX_EXPECTED_PER_DRAW_INSTANCES),
            groups_buffer: Vec::with_capacity(2000),
            staging_buffer: Vec::with_capacity(
                MAX_EXPECTED_PER_DRAW_INSTANCES * PER_DRAW_STRIDE_BYTES,
            ),
            state: None,
        }
    }
}

fn get_state(pass: &mut PassScene) -> &mut PassSceneState {
    debug_assert!(pass.state.is_some());
    // SAFETY: initialized once in init(), read every draw
    unsafe { pass.state.as_mut().unwrap_unchecked() }
}

impl Pass for PassScene {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(&mut self, renderer: &mut Renderer) -> Result<()> {
        // [SIMILAR] Prebuilt PSO used; avoid hot-path pipeline creation per TALK
        // Shaders: must match bind group layout indices: 0(camera),1(material textures),2(material params),3(per-draw)
        let vertex_wgsl = r#"
            struct Camera {
              position: vec4<f32>,
              viewProjection: mat4x4<f32>,
            };
            @group(0) @binding(0) var<uniform> UCamera: Camera;
            struct PerDraw { mvp: mat4x4<f32>, model: mat4x4<f32>, };
            @group(3) @binding(0) var<uniform> UPerDraw: PerDraw;
            struct VSIn {
              @location(0) pos: vec3<f32>,
              @location(1) uv: vec2<f32>,
              @location(2) normal: vec3<f32>,
            };
            struct VSOut {
              @builtin(position) pos: vec4<f32>,
              @location(0) uv: vec2<f32>,
              @location(1) worldPos: vec3<f32>,
              @location(2) worldNormal: vec3<f32>,
            };
            @vertex fn main(input: VSIn) -> VSOut {
              var out: VSOut;
              let worldPos4 = UPerDraw.model * vec4<f32>(input.pos, 1.0);
              // TODO: Use a proper normal matrix (inverse-transpose of model) if non-uniform scaling is used.
              let n = normalize((UPerDraw.model * vec4<f32>(input.normal, 0.0)).xyz);
              out.pos = UPerDraw.mvp * vec4<f32>(input.pos, 1.0);
              out.uv = input.uv;
              out.worldPos = worldPos4.xyz;
              out.worldNormal = n;
              return out;
            }
        "#;
        let fragment_wgsl = r#"
            // PBR material textures (set 1)
            @group(1) @binding(0) var texBaseColor: texture_2d<f32>;
            @group(1) @binding(1) var smpBaseColor: sampler;
            @group(1) @binding(2) var texNormal: texture_2d<f32>;
            @group(1) @binding(3) var smpNormal: sampler;
            @group(1) @binding(4) var texMetallicRoughness: texture_2d<f32>;
            @group(1) @binding(5) var smpMetallicRoughness: sampler;
            @group(1) @binding(6) var texEmissive: texture_2d<f32>;
            @group(1) @binding(7) var smpEmissive: sampler;
            // PBR params UBO (set 2)
            struct MaterialParams {
              baseColorFactor: vec3<f32>, _pad0: f32,
              metallicFactor: f32, roughnessFactor: f32, _pad1: vec2<f32>,
              emissiveFactor: vec3<f32>, _pad2: f32,
            }
            @group(2) @binding(0) var<uniform> UMaterial: MaterialParams;
            // Per-draw
            struct PerDraw { mvp: mat4x4<f32>, model: mat4x4<f32>, };
            @group(3) @binding(0) var<uniform> UPerDraw: PerDraw;
            struct Camera {
              position: vec4<f32>,
              viewProjection: mat4x4<f32>,
            };
            @group(0) @binding(0) var<uniform> UCamera: Camera;

            const PI: f32 = 3.14159265359;

            // TODO: Move lights to engine-provided uniforms; support directional/spot/area lights.
            const LIGHT_POS0: vec3<f32> = vec3<f32>(-10.0,  10.0, 10.0);
            const LIGHT_POS1: vec3<f32> = vec3<f32>( 10.0,  10.0, 10.0);
            const LIGHT_POS2: vec3<f32> = vec3<f32>(-10.0, -10.0, 10.0);
            const LIGHT_POS3: vec3<f32> = vec3<f32>( 10.0, -10.0, 10.0);
            const LIGHT_COL0: vec3<f32> = vec3<f32>(300.0, 300.0, 300.0);
            const LIGHT_COL1: vec3<f32> = vec3<f32>(300.0, 300.0, 300.0);
            const LIGHT_COL2: vec3<f32> = vec3<f32>(300.0, 300.0, 300.0);
            const LIGHT_COL3: vec3<f32> = vec3<f32>(300.0, 300.0, 300.0);

            fn DistributionGGX(N: vec3<f32>, H: vec3<f32>, roughness: f32) -> f32 {
              // Add epsilon to avoid singularities at very low roughness.
              let a = max(roughness * roughness, 0.0025);
              let a2 = a * a;
              let NdotH = max(dot(N, H), 0.0);
              let NdotH2 = NdotH * NdotH;
              let denom = (NdotH2 * (a2 - 1.0) + 1.0);
              return a2 / (PI * denom * denom + 1e-7);
            }

            fn GeometrySchlickGGX(NdotV: f32, roughness: f32) -> f32 {
              // Heitz's k for direct lighting approximation
              let r = roughness + 1.0;
              let k = (r * r) / 8.0;
              let denom = NdotV * (1.0 - k) + k;
              return NdotV / denom;
            }

            fn GeometrySmith(N: vec3<f32>, V: vec3<f32>, L: vec3<f32>, roughness: f32) -> f32 {
              let NdotV = max(dot(N, V), 0.0);
              let NdotL = max(dot(N, L), 0.0);
              let ggx2 = GeometrySchlickGGX(NdotV, roughness);
              let ggx1 = GeometrySchlickGGX(NdotL, roughness);
              return ggx1 * ggx2;
            }

            fn fresnelSchlick(cosTheta: f32, F0: vec3<f32>) -> vec3<f32> {
              return F0 + (vec3<f32>(1.0, 1.0, 1.0) - F0) * pow(1.0 - cosTheta, 5.0);
            }

            fn accumulateLight(
              WorldPos: vec3<f32>, N: vec3<f32>, V: vec3<f32>, F0: vec3<f32>,
              albedo: vec3<f32>, roughness: f32, metallic: f32,
              lightPos: vec3<f32>, lightColor: vec3<f32>
            ) -> vec3<f32> {
              let L = normalize(lightPos - WorldPos);
              let H = normalize(V + L);
              let distance = length(lightPos - WorldPos);
              let attenuation = 1.0 / (distance * distance);
              let radiance = lightColor * attenuation;

              let NDF = DistributionGGX(N, H, roughness);
              let G   = GeometrySmith(N, V, L, roughness);
              let F   = fresnelSchlick(max(dot(H, V), 0.0), F0);

              let kS = F;
              var kD = vec3<f32>(1.0, 1.0, 1.0) - kS;
              kD = kD * (1.0 - metallic);

              let numerator = NDF * G * F;
              let denominator = 4.0 * max(dot(N, V), 0.0) * max(dot(N, L), 0.0) + 0.0001;
              let specular = numerator / vec3<f32>(denominator, denominator, denominator);

              let NdotL = max(dot(N, L), 0.0);
              return (kD * (albedo / PI) + specular) * radiance * NdotL;
            }

            @fragment fn main(
              @location(0) uv: vec2<f32>,
              @location(1) WorldPos: vec3<f32>,
              @location(2) NormalIn: vec3<f32>
            ) -> @location(0) vec4<f32> {
              // Surface parameters
              var albedo = textureSample(texBaseColor, smpBaseColor, uv).rgb * UMaterial.baseColorFactor;
              let mr = textureSample(texMetallicRoughness, smpMetallicRoughness, uv).gb;
              var roughness = clamp(mr.x * UMaterial.roughnessFactor, 0.0, 1.0);
              // Robustness: keep roughness in a sane range to preserve highlight and stability.
              roughness = clamp(roughness, 0.045, 0.99);
              let metallic = clamp(mr.y * UMaterial.metallicFactor, 0.0, 1.0);
              // TODO: Support normal mapping (tangent space) and AO texture.
              let ao: f32 = 1.0;
              let N = normalize(NormalIn);
              let V = normalize(UCamera.position.xyz - WorldPos);

              var F0 = vec3<f32>(0.04, 0.04, 0.04);
              F0 = mix(F0, albedo, vec3<f32>(metallic, metallic, metallic));

              var Lo = vec3<f32>(0.0, 0.0, 0.0);
              Lo = Lo + accumulateLight(WorldPos, N, V, F0, albedo, roughness, metallic, LIGHT_POS0, LIGHT_COL0);
              Lo = Lo + accumulateLight(WorldPos, N, V, F0, albedo, roughness, metallic, LIGHT_POS1, LIGHT_COL1);
              Lo = Lo + accumulateLight(WorldPos, N, V, F0, albedo, roughness, metallic, LIGHT_POS2, LIGHT_COL2);
              Lo = Lo + accumulateLight(WorldPos, N, V, F0, albedo, roughness, metallic, LIGHT_POS3, LIGHT_COL3);

              var color = Lo;
              return vec4<f32>(color, 1.0);
            }
        "#;

        // Describe bind group layouts for pipeline creation
        let bind_groups: Vec<Vec<wgpu::BindGroupLayoutEntry>> = vec![
            // 0: camera
            vec![wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            // 1: material textures (PBR)
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
            // 2: material params (PBR)
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
            // 3: per-draw dynamic UBO (MVP)
            vec![wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: Some(
                        std::num::NonZeroU64::new(std::mem::size_of::<PerDrawStd140>() as u64)
                            .unwrap(),
                    ),
                },
                count: None,
            }],
        ];

        let desc = PipelineV2Desc {
            label: Some("scene_pipeline_v2"),
            vs: ShaderDesc {
                source: vertex_wgsl,
                entry_func: "main",
            },
            ps: ShaderDesc {
                source: fragment_wgsl,
                entry_func: "main",
            },
            bind_groups,
            targets: &[Some(wgpu::ColorTargetState {
                format: self.offscreen_color_texture.texture.format(),
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: Some(wgpu::DepthStencilState {
                format: self.depth_texture.texture.format(),
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
        };

        let pipeline = renderer.create_pipeline_v2(desc)?;

        // Camera buffer and bind group using pipeline's camera layout (group 0)
        let device = &renderer.ctx.device;
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera_buffer"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera_bind_group"),
            layout: &pipeline.bind_group_layouts[MATERIAL_BIND_GROUP_GLOBALS],
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        // Pre-create per-draw dynamic UBO ring and its bind group with large preallocation
        // Creation happens in init; growth/recreation only when capacity is insufficient during draw
        self.per_draw_capacity = MAX_EXPECTED_PER_DRAW_INSTANCES as u64;
        let size = self.per_draw_stride * self.per_draw_capacity;
        let per_draw_buffer = renderer.create_buffer(BufferDesc {
            label: Some("per_draw_dynamic_ubo_ring"),
            byte_size: size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        })?;
        let layout_ptr: *const wgpu::BindGroupLayout =
            &pipeline.bind_group_layouts[MATERIAL_BIND_GROUP_PERDRAW as usize] as *const _;
        let per_draw_bind_group =
            renderer
                .ctx
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("per_draw_bind_group"),
                    layout: unsafe { &*layout_ptr },
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &per_draw_buffer,
                            offset: 0,
                            size: Some(
                                std::num::NonZeroU64::new(
                                    std::mem::size_of::<PerDrawStd140>() as u64
                                )
                                .unwrap(),
                            ),
                        }),
                    }],
                });

        self.state = Some(PassSceneState {
            camera_uniform: CameraUniform::new(),
            camera_buffer,
            camera_bind_group,
            pipeline,
            per_draw_buffer,
            per_draw_bind_group,
        });

        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut CommandEncoder,
        renderer: &mut Renderer,
        _frame: &wgpu::SurfaceTexture,
        _view: &wgpu::TextureView,
        world: &WorldQuery,
    ) -> Result<()> {
        let active_camera_entity_handle = world.active_camera;
        let camera_component_storage = world.camera_components;
        let transform_component_storage = world.transform_components;

        // TODO: Update all isDirty=true materials, for each:
        //    - Color3f -> Update material uniform
        //    - Texture2D -> Update material bind groups

        // Read active camera + transform
        let camera_storage = camera_component_storage
            .data
            .get(active_camera_entity_handle.data().index as usize)
            .unwrap();
        let active_camera_component = camera_storage.as_ref().unwrap();
        let camera_transform_storage = transform_component_storage
            .data
            .get(active_camera_entity_handle.data().index as usize)
            .unwrap();
        let active_camera_transform_component = camera_transform_storage.as_ref().unwrap();

        // Update camera uniform and write to GPU buffer (no allocations)
        let vp_mat_arr: [[f32; 4]; 4] = {
            let state = get_state(self);
            state
                .camera_uniform
                .update_data(active_camera_component, active_camera_transform_component);
            renderer.ctx.queue.write_buffer(
                &state.camera_buffer,
                0,
                bytemuck::bytes_of(&state.camera_uniform),
            );
            state.camera_uniform.view_projection_matrix
        };
        let clear_color = active_camera_component.clear_color;

        // View-projection matrix
        let vp_mat: Mat4 = Mat4::from_cols_array_2d(&vp_mat_arr);

        // Extract frustum planes
        let row3 = vp_mat.row(3);
        let row0 = vp_mat.row(0);
        let row1 = vp_mat.row(1);
        let row2 = vp_mat.row(2);
        let make_plane = |plane_vec: Vec4| -> (Vec3, f32) {
            let normal = Vec3::new(plane_vec.x, plane_vec.y, plane_vec.z);
            let len = normal.length();
            if len > 0.0 {
                let normalized = normal / len;
                (normalized, plane_vec.w / len)
            } else {
                (normal, plane_vec.w)
            }
        };
        let planes = [
            make_plane(row3 + row0),
            make_plane(row3 - row0),
            make_plane(row3 + row1),
            make_plane(row3 - row1),
            make_plane(row3 + row2),
            make_plane(row3 - row2),
        ];

        // Build visible set
        self.visible_pre_draw_buffer.clear();
        for render_queue_item in world.render_queue.iter() {
            let key = pill_engine::internal::decompose_render_queue_key(render_queue_item.key);
            let mesh_handle =
                RendererMeshHandle::from_parts(key.mesh_index as u32, key.mesh_version as u32);
            let material_handle = RendererMaterialHandle::from_parts(
                key.material_index as u32,
                key.material_version as u32,
            );

            // Transform
            let entity_index = render_queue_item.entity_index as usize;
            let transform = transform_component_storage
                .data
                .get(entity_index)
                .unwrap()
                .as_ref()
                .unwrap();
            let model_arr = pill_engine::internal::get_model_matrix(transform);
            let model: Mat4 = Mat4::from_cols_array_2d(&model_arr);

            // Mesh AABB -> world AABB
            let mesh = renderer
                .state
                .resource_manager
                .meshes
                .get(mesh_handle)
                .unwrap();
            let local_min = Vec3::new(mesh.aabb_min[0], mesh.aabb_min[1], mesh.aabb_min[2]);
            let local_max = Vec3::new(mesh.aabb_max[0], mesh.aabb_max[1], mesh.aabb_max[2]);
            let corners = [
                Vec3::new(local_min.x, local_min.y, local_min.z),
                Vec3::new(local_max.x, local_min.y, local_min.z),
                Vec3::new(local_min.x, local_max.y, local_min.z),
                Vec3::new(local_max.x, local_max.y, local_min.z),
                Vec3::new(local_min.x, local_min.y, local_max.z),
                Vec3::new(local_max.x, local_min.y, local_max.z),
                Vec3::new(local_min.x, local_max.y, local_max.z),
                Vec3::new(local_max.x, local_max.y, local_max.z),
            ];
            let mut world_min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
            let mut world_max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
            for c in &corners {
                let p4 = model * Vec4::new(c.x, c.y, c.z, 1.0);
                let p = Vec3::new(p4.x, p4.y, p4.z);
                world_min = world_min.min(p);
                world_max = world_max.max(p);
            }
            // Frustum culling
            let mut outside = false;
            for (normal, d) in &planes {
                let p = Vec3::new(
                    if normal.x >= 0.0 {
                        world_max.x
                    } else {
                        world_min.x
                    },
                    if normal.y >= 0.0 {
                        world_max.y
                    } else {
                        world_min.y
                    },
                    if normal.z >= 0.0 {
                        world_max.z
                    } else {
                        world_min.z
                    },
                );
                let dist = normal.dot(p) + *d;
                if dist < 0.0 {
                    outside = true;
                    break;
                }
            }
            if outside {
                continue;
            }

            let view_proj: Mat4 = vp_mat;
            let mvp: [[f32; 4]; 4] = (view_proj * model).to_cols_array_2d();
            self.visible_pre_draw_buffer.push(VisiblePreDraw {
                material_handle,
                mesh_handle,
                entity_index: render_queue_item.entity_index,
                mvp,
                model: model.to_cols_array_2d(),
            });
        }

        // Sort and group (Pipeline -> Material -> Mesh)
        self.visible_pre_draw_buffer.sort_by_key(|v| {
            ((v.material_handle.generation() as u64) << 32) | (v.material_handle.index() as u64)
        });
        self.groups_buffer.clear();
        // Resolve pipeline pointer for this frame (single pipeline for now)
        let pipeline_ptr: *const wgpu::RenderPipeline = {
            let state = get_state(self);
            &state.pipeline.pipeline as *const _
        };
        for v in &self.visible_pre_draw_buffer {
            let need_new_group = self
                .groups_buffer
                .last()
                .map(|g| g.material_handle != v.material_handle)
                .unwrap_or(true);
            if need_new_group {
                self.groups_buffer.push(GroupCmd {
                    pipeline: pipeline_ptr,
                    material_handle: v.material_handle,
                    batches: Vec::new(),
                });
            }
            let g = self.groups_buffer.last_mut().unwrap();
            if let Some(batch) = g
                .batches
                .iter_mut()
                .find(|b| b.mesh_handle == v.mesh_handle)
            {
                batch.instances.push(PerDrawStd140 {
                    mvp: v.mvp,
                    model: v.model,
                });
            } else {
                g.batches.push(MeshBatch {
                    mesh_handle: v.mesh_handle,
                    instances: vec![PerDrawStd140 {
                        mvp: v.mvp,
                        model: v.model,
                    }],
                    base_offset_u32: 0,
                });
            }
        }

        // Per-draw ring buffer setup
        let needed: u64 = self
            .groups_buffer
            .iter()
            .map(|g| {
                g.batches
                    .iter()
                    .map(|b| b.instances.len() as u64)
                    .sum::<u64>()
            })
            .sum();
        if self.per_draw_capacity < needed {
            #[cfg(debug_assertions)]
            {
                log::error!(
                    "Per-draw capacity exceeded: needed={} capacity={}",
                    needed,
                    self.per_draw_capacity
                );
            }
            // Release: proceed; only first (capacity) entries will be used by draws
        }

        self.staging_buffer.clear();
        let mut next_offset_u32: u32 = 0;
        for g in self.groups_buffer.iter_mut() {
            for b in g.batches.iter_mut() {
                b.base_offset_u32 = next_offset_u32;
                for pd in &b.instances {
                    self.staging_buffer
                        .extend_from_slice(bytemuck::bytes_of(pd));
                    let pad =
                        (self.per_draw_stride as usize) - std::mem::size_of::<PerDrawStd140>();
                    self.staging_buffer.extend(std::iter::repeat(0u8).take(pad));
                    next_offset_u32 = next_offset_u32.wrapping_add(self.per_draw_stride as u32);
                }
            }
        }
        {
            let state_ref: &PassSceneState = unsafe { self.state.as_ref().unwrap_unchecked() };
            renderer
                .ctx
                .queue
                .write_buffer(&state_ref.per_draw_buffer, 0, &self.staging_buffer);
        }

        // Encode offscreen pass
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("m6_offscreen_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.offscreen_color_texture.texture_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear_color.x as f64,
                        g: clear_color.y as f64,
                        b: clear_color.z as f64,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_texture.texture_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        // Immutable view of pass state for binding (no aliasing with groups_buffer immutable borrow)
        let state_ref: &PassSceneState = unsafe { self.state.as_ref().unwrap_unchecked() };
        for group in &self.groups_buffer {
            rpass.set_pipeline(unsafe { &*group.pipeline });
            rpass.set_bind_group(
                MATERIAL_BIND_GROUP_GLOBALS as u32,
                &state_ref.camera_bind_group,
                &[],
            );
            // Bind group-specific material (HOT-ish path; debug-validated, unchecked in release)
            let material = unsafe {
                let m = renderer
                    .state
                    .resource_manager
                    .materials
                    .get(group.material_handle);
                debug_assert!(
                    m.is_some(),
                    "stale RendererMaterialHandle {:?} in render queue",
                    group.material_handle
                );
                m.unwrap_unchecked()
            };
            rpass.set_bind_group(
                MATERIAL_BIND_GROUP_TEXTURES as u32,
                &material.texture_bind_group,
                &[],
            );
            rpass.set_bind_group(
                MATERIAL_BIND_GROUP_PARAMS as u32,
                &material.param_bind_group,
                &[],
            );
            for batch in &group.batches {
                // Hot path: assume valid handles (render queue built from alive resources).
                // Debug-only validation; no branches in release.
                let mesh = unsafe {
                    let m = renderer
                        .state
                        .resource_manager
                        .meshes
                        .get(batch.mesh_handle);
                    debug_assert!(
                        m.is_some(),
                        "stale RendererMeshHandle {:?} in render queue",
                        batch.mesh_handle
                    );
                    m.unwrap_unchecked()
                };
                rpass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                rpass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for i in 0..batch.instances.len() {
                    let offset = batch
                        .base_offset_u32
                        .wrapping_add((i as u32) * (self.per_draw_stride as u32));
                    rpass.set_bind_group(
                        MATERIAL_BIND_GROUP_PERDRAW as u32,
                        &state_ref.per_draw_bind_group,
                        &[offset],
                    );
                    rpass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }
        }

        Ok(())
    }
}
