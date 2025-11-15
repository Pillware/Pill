use crate::resources::{MeshData, MeshVertex};

use anyhow::*;
use wgpu::util::DeviceExt;

// --- Vertex ---

pub trait Vertex {
    // Defines how a data is layed out in memory (To specify how RenderPipeline needs to map the buffer in the shader)
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a>;
}

// --- Mesh ---

pub struct RendererMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

impl RendererMesh {
    pub fn new(device: &wgpu::Device, name: &str, mesh_data: &MeshData) -> Result<Self> {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{:?}_vertex_buffer", name)),
            contents: bytemuck::cast_slice(&mesh_data.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{:?}_vertex_buffer", name)),
            contents: bytemuck::cast_slice(&mesh_data.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let renderer_mesh = Self {
            vertex_buffer,
            index_buffer,
            index_count: mesh_data.indices.len() as u32,
            aabb_min: mesh_data.aabb_min,
            aabb_max: mesh_data.aabb_max,
        };

        Ok(renderer_mesh)
    }
}

impl Vertex for RendererMesh {
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a> {
        // Use a static attribute array to satisfy lifetime requirements
        const ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
            0 => Float32x3, // position
            1 => Float32x2, // uv
            2 => Float32x3, // normal
            3 => Float32x3, // tangent
            4 => Float32x3, // bitangent
        ];
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<MeshVertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        }
    }
}
