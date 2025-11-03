use crate::renderer::{Pass, Renderer, WorldQuery};
use crate::resource_manager::ResourceManager;
use crate::resources::CameraUniform;
use anyhow::{Error, Result};
use glam::{Mat4, Vec3, Vec4};
use pill_core::{PillSlotMapKey, RendererError};
use pill_engine::internal::{
    get_renderer_resource_handle_from_camera_component, CameraComponent, ComponentStorage,
    EntityHandle, RenderQueueItem, RendererMaterialHandle, RendererMeshHandle,
    RendererPipelineHandle, TransformComponent,
};
use pill_engine::internal::{PillRenderer, PipelineV2, PipelineV2Desc, ShaderDesc};
use wgpu::CommandEncoder;

// Preallocated buffer structs for hot path optimization
pub(crate) struct VisiblePreDraw {
    pub(crate) pipeline_handle: RendererPipelineHandle,
    pub(crate) material_handle: RendererMaterialHandle,
    pub(crate) mesh_handle: RendererMeshHandle,
    pub(crate) entity_index: u32,
    pub(crate) mvp: [[f32; 4]; 4],
}

// M3: Hello Mesh + per-draw MVP (dynamic offsets)
pub(crate) struct MeshBatch {
    pub(crate) mesh_handle: RendererMeshHandle,
    pub(crate) instances: Vec<[[f32; 4]; 4]>,
    pub(crate) base_offset_u32: u32, // offset into per-draw ring for first instance
}

pub(crate) struct GroupCmd {
    pub(crate) pipeline_handle: RendererPipelineHandle,
    pub(crate) material_handle: RendererMaterialHandle,
    pub(crate) batches: Vec<MeshBatch>,
}

pub struct PassScene {
    label: String,
    // Per-frame dynamic UBO ring (Milestone 5)
    per_draw_stride: u64,
    per_draw_capacity: u64,
    per_draw_buffer: Option<wgpu::Buffer>,
    per_draw_bind_group: Option<wgpu::BindGroup>,
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
}

impl PassScene {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            per_draw_stride: 256,
            per_draw_capacity: 0,
            per_draw_buffer: None,
            per_draw_bind_group: None,
            visible_pre_draw_buffer: Vec::with_capacity(100_000),
            groups_buffer: Vec::with_capacity(2000),
            staging_buffer: Vec::with_capacity(100_000 * 144),
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
        // TODO:
        //  Problem:
        //      Material is coupled with Pipeline binds, now has handle to update i.e. texture via bind
        //      User creates Material and sets the params (floats, textures, etc.) that trigger update via Pipeline bind
        //      User can access Resources only via ResourcePool handles
        //      Material has to stay in sync with Pipeline and Shader
        //  Solution?:
        //      User can access via ResourcesPool only Material or MaterialInstance handles.
        //      Move PassScene to Engine. Pipeline can be local, Material too? Expose Material by Enum?
        //      User can create MaterialInstance and select Material/Pipeline via enum? i.e. PBR, UNLIT etc.
        //      Decouple Material params data from Implementation
        // [SIMILAR] Prebuilt PSO used; avoid hot-path pipeline creation per TALK
        // Shaders: must match bind group layout indices: 0(camera),1(material textures),2(material params),3(per-draw)
        let vertex_wgsl = r#"
            @group(0) @binding(0) var<uniform> UCamera: mat4x4<f32>;
            struct PerDraw { mvp: mat4x4<f32>, tint: vec4<f32>, };
            @group(3) @binding(0) var<uniform> UPerDraw: PerDraw;
            struct VSIn { @location(0) pos: vec3<f32>, @location(1) uv: vec2<f32>, };
            struct VSOut { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, };
            @vertex fn main(input: VSIn) -> VSOut {
              var out: VSOut;
              out.pos = UPerDraw.mvp * vec4<f32>(input.pos, 1.0);
              out.uv = input.uv;
              return out;
            }
        "#;
        let fragment_wgsl = r#"
            @group(1) @binding(0) var tex_diffuse: texture_2d<f32>;
            @group(1) @binding(1) var smp_diffuse: sampler;
            @group(1) @binding(2) var tex_normal: texture_2d<f32>;
            @group(1) @binding(3) var smp_normal: sampler;
            struct MaterialParams { tint_spec: vec4<f32>, }
            @group(2) @binding(0) var<uniform> UMaterial: MaterialParams;
            struct PerDraw { mvp: mat4x4<f32>, tint: vec4<f32>, };
            @group(3) @binding(0) var<uniform> UPerDraw: PerDraw;
            @fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
              let albedo = textureSample(tex_diffuse, smp_diffuse, uv);
              let tinted = vec4<f32>(UMaterial.tint_spec.rgb, 1.0) * UPerDraw.tint;
              let spec_boost = 0.5 + 0.5 * UMaterial.tint_spec.a;
              let color = albedo * tinted * spec_boost;
              return vec4<f32>(color.rgb, 1.0);
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
            // 1: material textures
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
            // 3: per-draw dynamic UBO
            vec![wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: Some(std::num::NonZeroU64::new(144).unwrap()),
                },
                count: None,
            }],
        ];

        let color_target = wgpu::ColorTargetState {
            format: renderer.state.color_format,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        };
        let depth_stencil = Some(wgpu::DepthStencilState {
            format: renderer.state.depth_format,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        });

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
            targets: &[Some(color_target)],
            depth_stencil,
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
            layout: &pipeline.bind_group_layouts[0],
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });

        self.state = Some(PassSceneState {
            camera_uniform: CameraUniform::new(),
            camera_buffer,
            camera_bind_group,
            pipeline,
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

        // Update camera uniform and write to GPU buffer (no allocations),
        // then release the mutable borrow and keep only cheap copies/ptrs.
        let (vp_mat_arr, camera_bg_ptr): ([[f32; 4]; 4], *const wgpu::BindGroup) = {
            let state = get_state(self);
            state
                .camera_uniform
                .update_data(active_camera_component, active_camera_transform_component);
            renderer.ctx.queue.write_buffer(
                &state.camera_buffer,
                0,
                bytemuck::bytes_of(&state.camera_uniform),
            );
            (
                state.camera_uniform.view_projection_matrix,
                &state.camera_bind_group as *const _,
            )
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
            let material_for_pipeline = renderer
                .state
                .resource_manager
                .materials
                .get(material_handle)
                .unwrap();
            let pipeline_handle: RendererPipelineHandle = material_for_pipeline.pipeline_handle;

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
                pipeline_handle,
                material_handle,
                mesh_handle,
                entity_index: render_queue_item.entity_index,
                mvp,
            });
        }

        // Sort and group
        self.visible_pre_draw_buffer.sort_by_key(|v| {
            (
                v.pipeline_handle.index(),
                v.material_handle.index(),
                v.mesh_handle.index(),
            )
        });
        self.groups_buffer.clear();
        for v in &self.visible_pre_draw_buffer {
            if self
                .groups_buffer
                .last()
                .map(|g| {
                    g.pipeline_handle != v.pipeline_handle || g.material_handle != v.material_handle
                })
                .unwrap_or(true)
            {
                self.groups_buffer.push(GroupCmd {
                    pipeline_handle: v.pipeline_handle,
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
                batch.instances.push(v.mvp);
            } else {
                g.batches.push(MeshBatch {
                    mesh_handle: v.mesh_handle,
                    instances: vec![v.mvp],
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
            self.per_draw_capacity = needed.next_power_of_two().max(1);
            let size = self.per_draw_stride * self.per_draw_capacity;
            self.per_draw_buffer =
                Some(renderer.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("per_draw_dynamic_ubo_ring"),
                    size,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            let layout_ptr: *const wgpu::BindGroupLayout = {
                let state = get_state(self);
                &state.pipeline.bind_group_layouts[3] as *const _
            };
            let buf_ptr: *const wgpu::Buffer = self.per_draw_buffer.as_ref().unwrap();
            let new_bg = renderer
                .ctx
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("per_draw_bind_group"),
                    layout: unsafe { &*layout_ptr },
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: unsafe { &*buf_ptr },
                            offset: 0,
                            size: Some(std::num::NonZeroU64::new(144).unwrap()),
                        }),
                    }],
                });
            self.per_draw_bind_group = Some(new_bg);
        }
        #[repr(C)]
        #[derive(Copy, Clone)]
        struct PerDrawStd140 {
            mvp: [[f32; 4]; 4],
            tint: [f32; 4],
        }
        unsafe impl bytemuck::Zeroable for PerDrawStd140 {}
        unsafe impl bytemuck::Pod for PerDrawStd140 {}
        let tint: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
        self.staging_buffer.clear();
        let mut next_offset_u32: u32 = 0;
        for g in self.groups_buffer.iter_mut() {
            for b in g.batches.iter_mut() {
                b.base_offset_u32 = next_offset_u32;
                for mvp in &b.instances {
                    let pd = PerDrawStd140 { mvp: *mvp, tint };
                    self.staging_buffer
                        .extend_from_slice(bytemuck::bytes_of(&pd));
                    let pad =
                        (self.per_draw_stride as usize) - std::mem::size_of::<PerDrawStd140>();
                    self.staging_buffer.extend(std::iter::repeat(0u8).take(pad));
                    next_offset_u32 = next_offset_u32.wrapping_add(self.per_draw_stride as u32);
                }
            }
        }
        if let Some(buf) = &self.per_draw_buffer {
            renderer
                .ctx
                .queue
                .write_buffer(buf, 0, &self.staging_buffer);
        }

        // Encode offscreen pass
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("m6_offscreen_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &renderer.state.offscreen_color_texture.texture_view,
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
                view: &renderer.state.depth_texture.texture_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        let pipeline_ptr: *const wgpu::RenderPipeline = {
            let state = get_state(self);
            &state.pipeline.pipeline as *const _
        };
        for group in &self.groups_buffer {
            rpass.set_pipeline(unsafe { &*pipeline_ptr });
            let material = renderer
                .state
                .resource_manager
                .materials
                .get(group.material_handle)
                .unwrap();
            // SAFETY: camera_bg_ptr points to self.state.camera_bind_group, valid for the duration of draw
            rpass.set_bind_group(0, unsafe { &*camera_bg_ptr }, &[]);
            rpass.set_bind_group(1, &material.texture_bind_group, &[]);
            rpass.set_bind_group(2, &material.parameter_bind_group, &[]);
            for batch in &group.batches {
                let mesh = renderer
                    .state
                    .resource_manager
                    .meshes
                    .get(batch.mesh_handle)
                    .unwrap();
                rpass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                rpass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for i in 0..batch.instances.len() {
                    let offset = batch
                        .base_offset_u32
                        .wrapping_add((i as u32) * (self.per_draw_stride as u32));
                    rpass.set_bind_group(3, self.per_draw_bind_group.as_ref().unwrap(), &[offset]);
                    rpass.draw_indexed(0..mesh.index_count, 0, 0..1);
                }
            }
        }

        Ok(())
    }
}
