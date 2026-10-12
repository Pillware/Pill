use pill_engine::{define_component, game::*};

const VERTEX_SHADER: &str = include_str!("../res/shaders/cartoon_vertex.wgsl");
const FRAGMENT_SHADER: &str = include_str!("../res/shaders/cartoon_fragment.wgsl");

const MAX_CARTOON_ENTITIES: usize = 64;

const BIND_GROUP_FRAME: usize = 0;
const BIND_GROUP_TEXTURE: usize = 1;

// Entities with this component are drawn by CartoonPass instead of the engine PBR pass
define_component!(CartoonComponent {
    mesh: MeshHandle,
    texture: TextureHandle,
    posterize_level: f32,
});

/// Mirrors `PerDraw` in cartoon_vertex.hlsl: position, rotation (radians), scale, posterize_level
type PerDraw = [[f32; 4]; 4];

struct CartoonPassState {
    pipeline: Pipeline,
    camera_buffer: wgpu::Buffer,
    per_draw_buffer: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
}

/// Game-defined scene pass: draws `CartoonComponent` entities with a posterized texture into the
/// engine's HDR and depth targets, between the opaque PBR pass and tonemapping.
pub struct CartoonPass {
    state: Option<CartoonPassState>,
    // Texture bind groups, built on first use per texture
    texture_bind_groups: Vec<(TextureHandle, wgpu::BindGroup)>,
    per_draw_data: Vec<PerDraw>,
    draw_list: Vec<(MeshHandle, TextureHandle)>,
}

impl CartoonPass {
    pub fn new() -> Self {
        Self {
            state: None,
            texture_bind_groups: Vec::new(),
            per_draw_data: Vec::with_capacity(MAX_CARTOON_ENTITIES),
            draw_list: Vec::with_capacity(MAX_CARTOON_ENTITIES),
        }
    }
}

impl Pass for CartoonPass {
    fn get_label(&self) -> &str {
        "cartoon_pass"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        let bind_groups = vec![
            // 0: camera uniform + per-draw storage
            vec![
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
            // 1: color texture + sampler
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
            ],
        ];

        let pipeline = renderer.create_pipeline(PipelineDesc {
            label: Some("cartoon_pass_pipeline"),
            vs: ShaderDesc {
                source: VERTEX_SHADER,
                entry_func: "vs_main",
            },
            ps: ShaderDesc {
                source: FRAGMENT_SHADER,
                entry_func: "fs_main",
            },
            vertex_buffers: &[RendererMesh::data_layout_descriptor()],
            bind_groups,
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_TARGET_FORMAT,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_TARGET_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
        })?;

        let camera_buffer = renderer.create_buffer(BufferDesc {
            label: Some("cartoon_pass_camera"),
            byte_size: std::mem::size_of::<[[f32; 4]; 4]>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        })?;
        let per_draw_buffer = renderer.create_buffer(BufferDesc {
            label: Some("cartoon_pass_per_draw"),
            byte_size: (std::mem::size_of::<PerDraw>() * MAX_CARTOON_ENTITIES) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        })?;
        let frame_bind_group =
            renderer
                .get_device()
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("cartoon_pass_frame_bind_group"),
                    layout: &pipeline.bind_group_layouts[BIND_GROUP_FRAME],
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: camera_buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: per_draw_buffer.as_entire_binding(),
                        },
                    ],
                });

        self.state = Some(CartoonPassState {
            pipeline,
            camera_buffer,
            per_draw_buffer,
            frame_bind_group,
        });
        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        _frame_target: &FrameTarget,
        world: &WorldQuery<'_>,
    ) -> Result<()> {
        let state = self
            .state
            .as_ref()
            .expect("CartoonPass: init() must run before draw()");

        // Collect draws (TransformComponent rotation is in degrees, the shader expects radians)
        self.per_draw_data.clear();
        self.draw_list.clear();
        let transforms = &world.query::<TransformComponent>()?.data;
        let cartoons = &world.query::<CartoonComponent>()?.data;
        for (transform, cartoon) in transforms.iter().zip(cartoons.iter()) {
            let (Some(transform), Some(cartoon)) = (transform, cartoon) else {
                continue;
            };
            if self.per_draw_data.len() == MAX_CARTOON_ENTITIES {
                break;
            }
            let rotation = transform.rotation;
            self.per_draw_data.push([
                transform.position.extend(0.0).to_array(),
                [
                    rotation.x.to_radians(),
                    rotation.y.to_radians(),
                    rotation.z.to_radians(),
                    0.0,
                ],
                transform.scale.extend(0.0).to_array(),
                [cartoon.posterize_level, 0.0, 0.0, 0.0],
            ]);
            self.draw_list.push((cartoon.mesh, cartoon.texture));
        }
        if self.draw_list.is_empty() {
            return Ok(());
        }

        // Upload camera and per-draw data
        let (camera, camera_transform) = world.active_camera()?;
        let view_projection = camera_view_projection(camera, camera_transform).to_cols_array_2d();
        let queue = renderer.get_queue();
        queue.write_buffer(
            &state.camera_buffer,
            0,
            bytemuck::bytes_of(&view_projection),
        );
        queue.write_buffer(
            &state.per_draw_buffer,
            0,
            bytemuck::cast_slice(&self.per_draw_data),
        );

        // Build texture bind groups on first use
        for (_, texture_handle) in &self.draw_list {
            let cached = self
                .texture_bind_groups
                .iter()
                .any(|(handle, _)| handle == texture_handle);
            if !cached {
                let texture = world.renderer_texture(texture_handle)?;
                let bind_group =
                    renderer
                        .get_device()
                        .create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("cartoon_pass_texture_bind_group"),
                            layout: &state.pipeline.bind_group_layouts[BIND_GROUP_TEXTURE],
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(
                                        &texture.texture_view,
                                    ),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::Sampler(&texture.sampler),
                                },
                            ],
                        });
                self.texture_bind_groups.push((*texture_handle, bind_group));
            }
        }

        // Draw on top of the PBR pass output (load, not clear)
        let render_state = world.get_global::<RenderStateComponent>()?;
        let color_view = renderer
            .get_render_target_view(render_state.hdr_target())
            .expect("CartoonPass: HDR target missing");
        let depth_view = renderer
            .get_render_target_view(render_state.depth_target())
            .expect("CartoonPass: depth target missing");
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cartoon_pass_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        render_pass.set_pipeline(&state.pipeline.pipeline);
        render_pass.set_bind_group(BIND_GROUP_FRAME as u32, &state.frame_bind_group, &[]);

        for (instance, (mesh_handle, texture_handle)) in self.draw_list.iter().enumerate() {
            let mesh = world.renderer_mesh(mesh_handle)?;
            let (_, texture_bind_group) = self
                .texture_bind_groups
                .iter()
                .find(|(handle, _)| handle == texture_handle)
                .expect("CartoonPass: texture bind group built above");
            render_pass.set_bind_group(BIND_GROUP_TEXTURE as u32, texture_bind_group, &[]);
            render_pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            render_pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            let instance = instance as u32;
            render_pass.draw_indexed(0..mesh.index_count, 0, instance..instance + 1);
        }

        Ok(())
    }
}
