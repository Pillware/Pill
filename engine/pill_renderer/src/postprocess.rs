use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::resources::renderer_texture::RendererTexture;

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
pub struct PostprocessParams {
    pub vignette_strength: f32,
    pub vignette_extent: f32,
    pub screen_resolution: [f32; 2],
}

impl Default for PostprocessParams {
    fn default() -> Self {
        Self {
            vignette_strength: 0.7,
            vignette_extent: 0.3,
            screen_resolution: [1920.0, 1080.0],
        }
    }
}

pub struct PostprocessPass {
    pub render_pipeline: wgpu::RenderPipeline,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    pub params_bind_group_layout: wgpu::BindGroupLayout,
    pub params_buffer: wgpu::Buffer,
    pub params_bind_group: wgpu::BindGroup,
    pub scene_texture: Option<RendererTexture>,
    pub scene_texture_bind_group: Option<wgpu::BindGroup>,
}

impl PostprocessPass {
    pub fn new(
        device: &wgpu::Device,
        vertex_shader: wgpu::ShaderModule,
        fragment_shader: wgpu::ShaderModule,
        output_format: wgpu::TextureFormat,
        screen_width: u32,
        screen_height: u32,
    ) -> Result<Self> {
        // Create texture bind group layout
        let texture_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("postprocess_texture_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
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

        // Create parameters bind group layout
        let params_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("postprocess_params_bind_group_layout"),
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
            ],
        });

        // Create parameters buffer
        let initial_params = PostprocessParams {
            screen_resolution: [screen_width as f32, screen_height as f32],
            ..Default::default()
        };

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("postprocess_params_buffer"),
            contents: bytemuck::cast_slice(&[initial_params]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create parameters bind group
        let params_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("postprocess_params_bind_group"),
            layout: &params_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &params_buffer,
                        offset: 0,
                        size: None,
                    }),
                },
            ],
        });

        // Create pipeline layout
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("postprocess_pipeline_layout"),
            bind_group_layouts: &[&texture_bind_group_layout, &params_bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create render pipeline
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("postprocess_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex_shader,
                entry_point: "main",
                buffers: &[], // No vertex buffer needed for fullscreen triangle
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader,
                entry_point: "main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: output_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        Ok(Self {
            render_pipeline,
            texture_bind_group_layout,
            params_bind_group_layout,
            params_buffer,
            params_bind_group,
            scene_texture: None,
            scene_texture_bind_group: None,
        })
    }

    pub fn create_scene_texture(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<()> {
        // Create scene render target texture
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("postprocess_scene_texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Store the texture in our renderer texture format
        self.scene_texture = Some(RendererTexture {
            texture,
            texture_view,
            sampler,
        });

        // Create bind group for the scene texture
        if let Some(scene_texture) = &self.scene_texture {
            self.scene_texture_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("postprocess_scene_texture_bind_group"),
                layout: &self.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&scene_texture.texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&scene_texture.sampler),
                    },
                ],
            }));
        }

        Ok(())
    }

    pub fn update_params(&self, queue: &wgpu::Queue, params: &PostprocessParams) {
        queue.write_buffer(&self.params_buffer, 0, bytemuck::cast_slice(&[*params]));
    }

    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output_view: &wgpu::TextureView,
    ) -> Result<()> {
        if self.scene_texture_bind_group.is_none() {
            return Err(anyhow::anyhow!("Scene texture not initialized"));
        }

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("postprocess_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, self.scene_texture_bind_group.as_ref().unwrap(), &[]);
        render_pass.set_bind_group(1, &self.params_bind_group, &[]);
        render_pass.draw(0..3, 0..1); // Draw fullscreen triangle

        Ok(())
    }

    pub fn get_scene_texture_view(&self) -> Option<&wgpu::TextureView> {
        self.scene_texture.as_ref().map(|t| &t.texture_view)
    }
}
