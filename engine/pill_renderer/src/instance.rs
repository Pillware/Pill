use crate::resources::Vertex;

use glam::{Mat3A, Mat4};
use pill_engine::{ internal::{ TransformComponent, get_model_matrix, get_normal_matrix, update_transform_matrices }};

// --- Instance ---

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub(crate) model_matrix: Mat4,
    pub(crate) normal_matrix: Mat3A, // It is matrix3 because we only need the rotation component
    // though we store it as SIMD-friendly type
}

impl Instance {
    pub fn new(transform_component: &TransformComponent) -> Instance {
        Instance {
            transform: {
                [
                    [transform_component.position.x, transform_component.position.y, transform_component.position.z],
                    [transform_component.rotation.x, transform_component.rotation.y, transform_component.rotation.z],
                    [transform_component.scale.x, transform_component.scale.y, transform_component.scale.z],
                ]
            }
        }
    }
}

impl Vertex for Instance {
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Instance>() as wgpu::BufferAddress,
            // We need to switch from using a step mode of Vertex to Instance
            // This means that shaders will only change to use the next instance when the shader starts processing a new instance
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute { // Instance transform position
                    offset: 0,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute { // Instance transform rotation
                    offset: mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute { // Instance transform scale
                    offset: mem::size_of::<[f32; 6]>() as wgpu::BufferAddress,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32x3,
                }

                // Normal matrix
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 16]>() as wgpu::BufferAddress,
                    shader_location: 9,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 20]>() as wgpu::BufferAddress,
                    shader_location: 10,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 24]>() as wgpu::BufferAddress,
                    shader_location: 11,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}

