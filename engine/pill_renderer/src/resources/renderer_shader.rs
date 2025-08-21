use pill_core::{debug, EngineError, LogContext, PillStyle, RendererError};
use pill_engine::internal::{ShaderParameterSlot, ShaderTextureSlot};
use std::collections::HashMap;
use anyhow::{Error, Result};

use crate::config::{
    ENGINE_PARAMETERS_BINDING_INDEX, 
    CAMERA_PARAMETERS_BINDING_INDEX, 
    MATERIAL_PARAMETERS_BINDING_INDEX
};

pub struct RendererShader {
    pub render_pipeline: wgpu::RenderPipeline,
    pub bind_group_layouts: Vec<wgpu::BindGroupLayout>,
    pub parameter_slots: HashMap<String, ShaderParameterSlot>,
    pub texture_slots: HashMap<String, ShaderTextureSlot>,
}

use naga::front::glsl;
use naga::back::wgsl;

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

        // Print shader information
        {
            let mut shader_info = format!(
                "Creating shader {}:\n - Settings: engine bindings: {}, camera bindings: {}",
                name.name_style(),
                enable_engine_binding,
                enable_camera_binding,
            );

            shader_info.push_str("\n - Parameter slots:");
            for (key, slot) in parameter_slots {
                shader_info.push_str(&format!(
                    "\n   - {}: {:?} {:?}",
                    key, slot.name, slot.parameter_type
                ));
            }

            shader_info.push_str("\n - Texture slots:");
            for (key, slot) in texture_slots {
                shader_info.push_str(&format!(
                    "\n   - {}: texture_binding={}, sampler_binding={}",
                    key, slot.texture_binding, slot.sampler_binding
                ));
            }

            // Log with context
            debug!(LogContext::Rendering => "{}", shader_info);
        }

        debug!(LogContext::Rendering => "Creating shader modules");

        // Convert bytes to string
        let vertex_shader_source = std::str::from_utf8(vertex_shader_bytes)
            .map_err(|e| Error::new(RendererError::InvalidShaderData("Vertex".to_string(), name.to_string(), e.to_string())))?;
        let fragment_shader_source = std::str::from_utf8(fragment_shader_bytes)
            .map_err(|e| Error::new(RendererError::InvalidShaderData("Fragment".to_string(), name.to_string(), e.to_string())))?;

        // Convert GLSL to WGSL
        let vertex_wgsl = compile_glsl_to_wgsl(vertex_shader_source, naga::ShaderStage::Vertex)
            .map_err(|e| Error::new(RendererError::ShaderCompilationFailed("Vertex".to_string(), name.to_string(), e.to_string())))?;
        let fragment_wgsl = compile_glsl_to_wgsl(fragment_shader_source, naga::ShaderStage::Fragment)
            .map_err(|e| Error::new(RendererError::ShaderCompilationFailed("Fragment".to_string(), name.to_string(), e.to_string())))?;

        // Create shader modules with WGSL
        let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("master_vertex_shader"),
            source: wgpu::ShaderSource::Wgsl(vertex_wgsl.into()),
        });
        let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("master_fragment_shader"),
            source: wgpu::ShaderSource::Wgsl(fragment_wgsl.into()),
        });

        debug!(LogContext::Rendering => "Creating parameters bind group layout");

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

        debug!(LogContext::Rendering => "Creating textures bind group layout");

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

        let textures_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(&format!("{}_textures_bind_group_layout", name)),
            entries: &textures_bind_group_layout_entries,
        });



        // Create pipeline layout
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

        let render_pipeline_descriptor = wgpu::RenderPipelineDescriptor {
            label: Some("render_pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { 
                module: &vertex_shader,
                entry_point: Some("main"),
                buffers: vertex_layouts, // Specifies structure of vertices that will be passed to the vertex shader
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader,
                entry_point: Some("main"),
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
            cache: None,
        };

        debug!(LogContext::Rendering => "Creating render pipeline");

        let render_pipeline = device.create_render_pipeline(&render_pipeline_descriptor);

        debug!(LogContext::Rendering => "Render pipeline created");

        let pipeline = Self { 
            render_pipeline,
            bind_group_layouts: vec![parameters_bind_group_layout, textures_bind_group_layout],
            parameter_slots: parameter_slots.clone(),
            texture_slots: texture_slots.clone(),
        };

        Ok(pipeline)
    }
}

fn compile_glsl_to_wgsl(source: &str, stage: naga::ShaderStage) -> Result<String> {
    let mut frontend = glsl::Frontend::default();
    let options = glsl::Options::from(stage);
    let module = frontend.parse(&options, source).unwrap();
    
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    
    let info = validator.validate(&module)?;
    
    let mut output = String::new();
    let mut writer = wgsl::Writer::new(&mut output, wgsl::WriterFlags::empty());
    writer.write(&module, &info)?;
    
    Ok(output)
}