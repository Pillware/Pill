use crate::graphics::RendererShaderHandle;
use crate::resources::{Resource, ResourceStorage, ShaderParameterSlot, ShaderTextureSlot};
use pill_core::{debug, LogContext, PillStyle, PillTypeMapKey, Result};
use std::collections::HashMap;

pub struct RendererShader {
    pub name: String,
    pub render_pipeline: wgpu::RenderPipeline,

    pub parameter_slots: Vec<(String, ShaderParameterSlot)>,
    pub parameters_bind_group_layout: Option<wgpu::BindGroupLayout>,

    pub texture_slots: HashMap<String, ShaderTextureSlot>,
    pub textures_bind_group_layout: Option<wgpu::BindGroupLayout>,

    pub pass_engine_parameters: bool,
    pub pass_camera_parameters: bool,
}

impl RendererShader {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
        vertex_layouts: &[wgpu::VertexBufferLayout],
        vertex_wgsl: &str,
        fragment_wgsl: &str,
        parameter_slots: &[(String, ShaderParameterSlot)],
        texture_slots: &HashMap<String, ShaderTextureSlot>,
        engine_bind_group_layout: &wgpu::BindGroupLayout,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        pass_engine_parameters: bool,
        pass_camera_parameters: bool,
    ) -> Result<Self> {
        {
            let mut shader_info = format!(
                "Creating shader {}:\n - Settings:\n   - Pass engine parameters: {}\n   - Pass camera parameters: {}",
                name.name_style(),
                pass_engine_parameters,
                pass_camera_parameters,
            );

            shader_info.push_str("\n - Parameter slots:");
            for (slot_name, slot) in parameter_slots {
                shader_info.push_str(&format!("\n   - {}: {:?}", slot_name, slot.parameter_type));
            }

            shader_info.push_str("\n - Texture slots:");
            for (slot_name, slot) in texture_slots {
                shader_info.push_str(&format!(
                    "\n   - {}: texture_binding={}, sampler_binding={}",
                    slot_name, slot.texture_binding, slot.sampler_binding
                ));
            }

            debug!(LogContext::Rendering => "{}", shader_info);
        }

        let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("master_vertex_shader"),
            source: wgpu::ShaderSource::Wgsl(vertex_wgsl.into()),
        });
        let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("master_fragment_shader"),
            source: wgpu::ShaderSource::Wgsl(fragment_wgsl.into()),
        });

        let parameters_bind_group_layout = if !parameter_slots.is_empty() {
            Some(
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some(&format!("{}_parameters_bind_group_layout", name)),
                    entries: &[wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    }],
                }),
            )
        } else {
            None
        };

        let textures_bind_group_layout = if !texture_slots.is_empty() {
            let mut entries = Vec::new();

            for texture_slot in texture_slots.values() {
                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: texture_slot.texture_binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                });

                entries.push(wgpu::BindGroupLayoutEntry {
                    binding: texture_slot.sampler_binding,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                });
            }

            Some(
                device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some(&format!("{}_textures_bind_group_layout", name)),
                    entries: &entries,
                }),
            )
        } else {
            None
        };

        let pipeline_layout = {
            let mut bind_group_layouts = Vec::new();

            if pass_engine_parameters {
                bind_group_layouts.push(engine_bind_group_layout);
            }
            if pass_camera_parameters {
                bind_group_layouts.push(camera_bind_group_layout);
            }
            if let Some(ref layout) = parameters_bind_group_layout {
                bind_group_layouts.push(layout);
            }
            if let Some(ref layout) = textures_bind_group_layout {
                bind_group_layouts.push(layout);
            }

            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(&format!("{}_pipeline_layout", name)),
                bind_group_layouts: &bind_group_layouts,
                push_constant_ranges: &[],
            })
        };

        let color_target_states = &[Some(wgpu::ColorTargetState {
            format: color_format,
            blend: Some(wgpu::BlendState {
                alpha: wgpu::BlendComponent::REPLACE,
                color: wgpu::BlendComponent::REPLACE,
            }),
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let render_pipeline_descriptor = wgpu::RenderPipelineDescriptor {
            label: Some(&format!("{}_render_pipeline", name)),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex_shader,
                entry_point: Some("vs_main"),
                buffers: vertex_layouts,
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader,
                entry_point: Some("fs_main"),
                targets: color_target_states,
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
                unclipped_depth: true,
            },
            depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        };

        let render_pipeline = device.create_render_pipeline(&render_pipeline_descriptor);

        debug!(LogContext::Rendering => "Render pipeline created");

        Ok(Self {
            name: name.to_string(),
            render_pipeline,
            parameter_slots: parameter_slots.to_vec(),
            textures_bind_group_layout,
            texture_slots: texture_slots.clone(),
            parameters_bind_group_layout,
            pass_engine_parameters,
            pass_camera_parameters,
        })
    }
}

impl PillTypeMapKey for RendererShader {
    type Storage = ResourceStorage<RendererShader>;
}

impl Resource for RendererShader {
    type Handle = RendererShaderHandle;

    fn get_name(&self) -> String {
        self.name.clone()
    }
}
