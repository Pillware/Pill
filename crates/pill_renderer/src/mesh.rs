use anyhow::*;
use pill_engine::internal::{MeshData, MeshVertex};
use slotmap::new_key_type;
use std::ops::Range;
use std::path::Path;
use tobj::LoadOptions;
use wgpu::util::DeviceExt;
use cgmath::InnerSpace;

use crate::texture;

new_key_type! { 
    pub struct RendererMeshHandle;
}

pub trait Vertex {
     // Defines how a data is layed out in memory (To specify how render_pipeline needs to map the buffer in the shader)
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a>;
}

pub struct RendererMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub element_count: u32,
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
            element_count: mesh_data.element_count,
        };

        Ok(renderer_mesh)
    }
}

impl Vertex for RendererMesh {
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<MeshVertex>() as wgpu::BufferAddress, // Specifies how wide a vertex is
            step_mode: wgpu::VertexStepMode::Vertex, // Specifies how often render pipeline should move to the next vertex
            attributes: &[ // Describes the individual parts of the vertex (1:1 mapping with Vertex struct)
                wgpu::VertexAttribute {
                    offset: 0,  // Offset in bytes that this attribute starts
                    shader_location: 0, // Location to store this attribute at (this will be [[location(0)]])
                    format: wgpu::VertexFormat::Float32x3, // Tells the shader the shape of the attribute (Float32x3 is vec3<f32>)
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 5]>() as wgpu::BufferAddress,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x3,
                },
                // Tangent and bitangent
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 11]>() as wgpu::BufferAddress,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}


