use std::path::Path;
use std::path::PathBuf;

use anyhow::{ Result, Context, Error };
use pill_core::na::SliceRange;
use pill_engine::internal::ResourceHandle;
use wgpu::Device;
use wgpu::RenderPipeline;

pub struct RendererPipelineHandle {
    pub index: u32,
}

impl ResourceHandle for RendererPipelineHandle
{
    fn get_index(&self) -> u32 {
        self.index
    }
}

pub struct Pipeline {
    pub render_pipeline: RenderPipeline,
}

impl Pipeline {
    pub fn new(
        device: &Device,
        color_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
        vertex_layouts: &[wgpu::VertexBufferLayout],
    ) -> Result<Self> {
        
        // Define shaders
        let vertex_shader = wgpu::ShaderModuleDescriptor {
            label: Some("shader.vert"),
            source: wgpu::util::make_spirv(include_bytes!("../res/shaders/built/master.vert.spv")),
        };

        let vertex_shader = device.create_shader_module(&vertex_shader);


        let fragment_shader = wgpu::ShaderModuleDescriptor {
            label: Some("shader.vert"),
            source: wgpu::util::make_spirv(include_bytes!("../res/shaders/built/master.frag.spv")),
        };

        let fragment_shader = device.create_shader_module(&fragment_shader);

        // Define texture bind group layout (Describes a set of resources and how they can be accessed by a shader)
        let texture_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { 
            label: Some("texture_bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { // Entry for the sampled texture at binding 0
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT, // Visible only to fragment shader
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { // Entry for the sampler at binding 1
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT, // Visible only to fragment shader
                    ty: wgpu::BindingType::Sampler {
                        comparison: false,
                        filtering: true,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { // Normal map
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler { 
                        comparison: false,
                        filtering: true, 
                    },
                    count: None,
                },
            ],
        });

        // Define camera bind group layout
        let camera_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false, // Specifies if this buffer will be changing size or not
                    min_binding_size: None,
                },
                count: None,
            }],
            label: Some("camera_bind_group_layout"),
        });

        // Define light bind group layout
        let light_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
            label: None,
        });

        // Create pipeline layout descriptor
        let pipeline_layout_descriptor = wgpu::PipelineLayoutDescriptor {
            label: Some("Render Pipeline Layout"),
            bind_group_layouts: &[
                &texture_bind_group_layout,
                &camera_bind_group_layout,
                &light_bind_group_layout,
            ],
            push_constant_ranges: &[],
        };

        // Create pipeline layout
        let layout = device.create_pipeline_layout(&pipeline_layout_descriptor);

        let render_pipeline_descriptor = wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { 
                module: &vertex_shader,
                entry_point: "main",
                buffers: vertex_layouts, // Specifies structure of vertices that will be passed to the vertex shader
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader,
                entry_point: "main",
                targets: &[wgpu::ColorTargetState { // Specifies what what color outputs wwgpu should set up
                    format: color_format,
                    blend: Some(wgpu::BlendState {
                        alpha: wgpu::BlendComponent::REPLACE,
                        color: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                }],
            }),
            primitive: wgpu::PrimitiveState { // Specifies how to interpret vertices when converting them into triangles
                topology: wgpu::PrimitiveTopology::TriangleList, // Each three vertices will correspond to one triangle
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw, // Specifies how to determine whether a given triangle is facing forward or not (FrontFace::Ccw means that a triangle is facing forward if the vertices are arranged in a counter clockwise direction)
                cull_mode: Some(wgpu::Face::Back), // Triangles that are not considered facing forward are culled (not included in the render) as specified by CullMode::Back            
                polygon_mode: wgpu::PolygonMode::Fill, // Setting this to anything other than Fill requires Features::NON_FILL_POLYGON_MODE     
                clamp_depth: false, // Requires Features::DEPTH_CLAMPING
                conservative: false, // Requires Features::CONSERVATIVE_RASTERIZATION
            },
            depth_stencil: depth_format.map(|format| wgpu::DepthStencilState { // [TODO: Investigate this map]
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
        };

        let render_pipeline = device.create_render_pipeline(&render_pipeline_descriptor);

        let pipeline = Self { 
            render_pipeline
        };

        Ok(pipeline)
    }
}
