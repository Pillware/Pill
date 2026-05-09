use anyhow::Result;
use indexmap::IndexMap;
use pill_core::{debug, LogContext, PillStyle};
use pill_engine::internal::{ShaderParameterSlot, ShaderTextureSlot};
use std::collections::HashMap;

pub struct RendererShader {
    pub name: String,
    pub render_pipeline: wgpu::RenderPipeline,

    pub parameter_slots: IndexMap<String, ShaderParameterSlot>,
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
        parameter_slots: &IndexMap<String, ShaderParameterSlot>,
        texture_slots: &HashMap<String, ShaderTextureSlot>,
        engine_bind_group_layout: &wgpu::BindGroupLayout,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
        pass_engine_parameters: bool,
        pass_camera_parameters: bool,
    ) -> Result<Self> {
        // Print shader information
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

            // Log with context
            debug!(LogContext::Rendering => "{}", shader_info);
        }

        debug!(LogContext::Rendering => "Shader modules created");

        // Create shader modules from WGSL strings — wgpu consumes WGSL natively;
        // no compile step at runtime, no naga linked into the binary.
        let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("master_vertex_shader"),
            source: wgpu::ShaderSource::Wgsl(vertex_wgsl.into()),
        });
        let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("master_fragment_shader"),
            source: wgpu::ShaderSource::Wgsl(fragment_wgsl.into()),
        });

        let parameters_bind_group_layout = {
            if !parameter_slots.is_empty() {
                let bind_group_layout_entry = wgpu::BindGroupLayoutEntry {
                    binding: 0, // (set = 2, binding = 0)
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                };

                Some(
                    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                        label: Some(&format!("{}_parameters_bind_group_layout", name)),
                        entries: &[bind_group_layout_entry],
                    }),
                )
            } else {
                None
            }
        };

        debug!(LogContext::Rendering => "Parameters bind group layout created");

        // Create bind group layout entries for textures - Bind group slot 1
        let textures_bind_group_layout = {
            if !texture_slots.is_empty() {
                let mut entries = Vec::new();

                for texture_slot in texture_slots.values() {
                    // Texture binding
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

                    // Sampler binding
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
            }
        };

        debug!(LogContext::Rendering => "Textures bind group layout created");

        // Create pipeline layout
        let pipeline_layout = {
            // Collect bind group layouts only if they exist
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

        // Create color target states that specifies what what color outputs wgpu should set up
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
                buffers: vertex_layouts, // Specifies structure of vertices that will be passed to the vertex shader
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader,
                entry_point: Some("fs_main"),
                targets: color_target_states,
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                // Specifies how to interpret vertices when converting them into triangles
                topology: wgpu::PrimitiveTopology::TriangleList, // Each three vertices will correspond to one triangle
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw, // Specifies how to determine whether a given triangle is facing forward or not (FrontFace::Ccw means that a triangle is facing forward if the vertices are arranged in a counter clockwise direction)
                cull_mode: Some(wgpu::Face::Back), // Triangles that are not considered facing forward are culled (not included in the render) as specified by CullMode::Back
                polygon_mode: wgpu::PolygonMode::Fill, // Setting this to anything other than Fill requires Features::NON_FILL_POLYGON_MODE
                conservative: false, // Requires Features::CONSERVATIVE_RASTERIZATION
                unclipped_depth: true, // Requires Features::DEPTH_CLAMPING
            },
            depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less, // Specifies when to discard a new pixel. Using LESS means pixels will be drawn front to back
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: 1, // Determines how many samples pipeline will use (Multisampling)
                mask: !0, // Specifies which samples should be active
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
            cache: None,
        };

        let render_pipeline = device.create_render_pipeline(&render_pipeline_descriptor);

        let pipeline = Self {
            name: name.to_string(),
            render_pipeline,
            parameter_slots: parameter_slots.clone(),
            textures_bind_group_layout,
            texture_slots: texture_slots.clone(),
            parameters_bind_group_layout,
            pass_engine_parameters,
            pass_camera_parameters,
        };

        debug!(LogContext::Rendering => "Render pipeline created");

        Ok(pipeline)
    }
}
