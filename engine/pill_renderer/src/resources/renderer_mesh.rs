use pill_engine::internal::{MeshData, MeshVertex};

use pill_core::Result;
use wgpu::util::DeviceExt;

// --- Vertex ---

pub trait Vertex {
    // Defines how a data is layed out in memory (To specify how RenderPipeline needs to map the buffer in the shader)
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a>;
}

// --- Mesh ---

pub struct RendererMesh {
    pub name: String,
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

impl RendererMesh {
    pub fn new(device: &wgpu::Device, name: &str, mesh_data: &MeshData, ray_tracing_enabled: bool) -> Result<Self> {
        let mut vertex_usage = wgpu::BufferUsages::VERTEX;
        let mut index_usage = wgpu::BufferUsages::INDEX;

        // When hardware ray tracing is enabled, mark buffers for BLAS input
        // so they can be used as acceleration-structure geometry sources.
        if ray_tracing_enabled {
            vertex_usage |= wgpu::BufferUsages::BLAS_INPUT;
            index_usage |= wgpu::BufferUsages::BLAS_INPUT;
        }

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{:?}_vertex_buffer", name)),
            contents: bytemuck::cast_slice(&mesh_data.vertices),
            usage: vertex_usage,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{:?}_index_buffer", name)),
            contents: bytemuck::cast_slice(&mesh_data.indices),
            usage: index_usage,
        });

        let renderer_mesh = Self {
            name: name.to_string(),
            vertex_buffer,
            index_buffer,
            index_count: mesh_data.indices.len() as u32,
        };

        Ok(renderer_mesh)
    }
}

impl Vertex for RendererMesh {
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    // Vertex position
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    // Vertex texture coordinates
                    // slangc maps TEXCOORD0 → @location(4), not 1
                    offset: mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    // Vertex normal
                    // slangc maps NORMAL → @location(5)
                    offset: mem::size_of::<[f32; 5]>() as wgpu::BufferAddress,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    // Vertex tangent
                    // slangc maps TANGENT → @location(6)
                    offset: mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    // Vertex bitangent
                    // slangc maps BINORMAL → @location(7)
                    offset: mem::size_of::<[f32; 11]>() as wgpu::BufferAddress,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}
