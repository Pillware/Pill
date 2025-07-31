use anyhow::Result;
use pill_engine::internal::{ShaderParameterSlot, ShaderTextureSlot};
use std::collections::HashMap;

use crate::{CAMERA_PARAMETERS_BINDING_INDEX, ENGINE_PARAMETERS_BINDING_INDEX, MATERIAL_PARAMETERS_BINDING_INDEX};

pub struct RendererShader {
    pub render_pipeline: wgpu::RenderPipeline,
    pub bind_group_layouts: Vec<wgpu::BindGroupLayout>,
    pub parameter_slots: HashMap<String, ShaderParameterSlot>,
    pub texture_slots: HashMap<String, ShaderTextureSlot>,
}

impl RendererShader {
    pub fn new(
        name: &str,
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
        vertex_layouts: &[wgpu::VertexBufferLayout],
        vertex_shader_bytes: &[u8],
        fragment_shader_bytes: &[u8],
        parameter_slots: &HashMap<String, ShaderParameterSlot>,
        texture_slots: &HashMap<String, ShaderTextureSlot>,
        enable_engine_binding: bool,
        enable_camera_binding: bool,
    ) -> Result<Self> {
        let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&format!("{}_vertex", name)),
            source: wgpu::util::make_spirv(vertex_shader_bytes),
        });

        let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&format!("{}_fragment", name)),
            source: wgpu::util::make_spirv(fragment_shader_bytes),
        });

        let mut parameters_bind_group_layout_entries = Vec::new();

        if enable_engine_binding {
            parameters_bind_group_layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: ENGINE_PARAMETERS_BINDING_INDEX as u32, // (set = 0, binding = 0)
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false, // Specifies if this buffer will be changing size or not
                    min_binding_size: None,
                },
                count: None,
            });
        }

        if enable_camera_binding {
            parameters_bind_group_layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: CAMERA_PARAMETERS_BINDING_INDEX as u32, // (set = 0, binding = 1)
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false, // Specifies if this buffer will be changing size or not
                    min_binding_size: None,
                },
                count: None,
            });
        }

        if !parameter_slots.is_empty() {
            parameters_bind_group_layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: MATERIAL_PARAMETERS_BINDING_INDEX as u32, // (set = 0, binding = 2)
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false, // Specifies if this buffer will be changing size or not
                    min_binding_size: None,
                },
                count: None,
            });
        }

        // Create bind group layout entries for parameter slots - Bind group slot 0
        let parameters_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{}_parameters_bind_group_layout", name)),
            entries: &parameters_bind_group_layout_entries,
        });


        // Create bind group layout entries for textures - Bind group slot 1
        let mut textures_bind_group_layout_entries = Vec::new();

        for texture_slot in texture_slots.values() {
            // Create texture binding
            textures_bind_group_layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: texture_slot.texture_binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                },
                count: None,
            });

            // Create sampler binding
            textures_bind_group_layout_entries.push(wgpu::BindGroupLayoutEntry {
                binding: texture_slot.sampler_binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }


        // Print shader information
        {
            println!(
                "Creating shader '{}':\n - engine bindings {}\n - camera bindings {}",
                name,
                enable_engine_binding,
                enable_camera_binding,
            );
            println!(" - parameter slots:");
            for (key, slot) in parameter_slots {
                println!("   - {}: {:?} {:?}", key, slot.name, slot.parameter_type);
            }
            println!(" - texture slots:");
            for (key, slot) in texture_slots {
                println!(
                "   - {}: texture_binding={}, sampler_binding={}",
                key, slot.texture_binding, slot.sampler_binding
                );
            }
        }
        



        let textures_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{}_textures_bind_group_layout", name)),
            entries: &textures_bind_group_layout_entries,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(&format!("{}_pipeline_layout", name)),
            bind_group_layouts: &[&parameters_bind_group_layout, &textures_bind_group_layout],
            push_constant_ranges: &[],
        });

        // Create color target states that specifies what what color outputs wgpu should set up
        let color_target_states = &[Some(wgpu::ColorTargetState { 
            format: color_format,
            blend: Some(wgpu::BlendState {
                alpha: wgpu::BlendComponent::REPLACE,
                color: wgpu::BlendComponent::REPLACE,
            }),
            write_mask: wgpu::ColorWrites::ALL,
        })];

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(name),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex_shader,
                entry_point: "main",
                buffers: vertex_layouts, // Specifies structure of vertices that will be passed to the vertex shader
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader,
                entry_point: "main",
                targets: color_target_states,
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { // Specifies how to interpret vertices when converting them into triangles
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
        });

        Ok(Self {
            render_pipeline,
            bind_group_layouts: vec![parameters_bind_group_layout, textures_bind_group_layout],
            parameter_slots: parameter_slots.clone(),
            texture_slots: texture_slots.clone(),
        })
    }
}
