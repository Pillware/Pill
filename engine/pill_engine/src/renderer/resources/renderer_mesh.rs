use crate::graphics::RendererMeshHandle;
use crate::resources::{MeshData, MeshVertex, Resource, ResourceStorage};

use pill_core::{PillTypeMapKey, Result};
use wgpu::util::DeviceExt;

pub trait Vertex {
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a>;
}

pub struct RendererMesh {
    pub name: String,
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

impl RendererMesh {
    pub fn new(device: &wgpu::Device, name: &str, mesh_data: &MeshData) -> Result<Self> {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{:?}_vertex_buffer", name)),
            contents: bytemuck::cast_slice(&mesh_data.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{:?}_index_buffer", name)),
            contents: bytemuck::cast_slice(&mesh_data.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Ok(Self {
            name: name.to_string(),
            vertex_buffer,
            index_buffer,
            index_count: mesh_data.indices.len() as u32,
        })
    }
}

impl PillTypeMapKey for RendererMesh {
    type Storage = ResourceStorage<RendererMesh>;
}

impl Resource for RendererMesh {
    type Handle = RendererMeshHandle;

    fn get_name(&self) -> String {
        self.name.clone()
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
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 5]>() as wgpu::BufferAddress,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 11]>() as wgpu::BufferAddress,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}
