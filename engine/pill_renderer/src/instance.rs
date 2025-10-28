use crate::resources::Vertex;

use pill_engine::{ internal::{ TransformComponent, get_model_matrix, get_normal_matrix, update_transform_matrices }};

// --- Instance ---

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub(crate) model_matrix: [[f32; 4]; 4], // It is not possible to use cgmath with bytemuck directly. Conversion from Quaternion into a 4x4 f32 array (matrix) needed
    // TODO: double check if we can use glam types directly right now?
    pub(crate) normal_matrix: [[f32; 3]; 3], // It is matrix3 because we only need the rotation componen
}

impl Instance {
    pub fn new(transform_component: &TransformComponent) -> Instance {
        Instance {
            model_matrix: get_model_matrix(transform_component),
            normal_matrix: get_normal_matrix(transform_component),
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
                // Model matrix (mat4 takes up 4 vertex slots as it is technically 4 vec4s. We need to define a slot for each vec4)
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 4]>() as wgpu::BufferAddress,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 8]>() as wgpu::BufferAddress,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 12]>() as wgpu::BufferAddress,
                    shader_location: 8,
                    format: wgpu::VertexFormat::Float32x4,
                },

                // Normal matrix
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 16]>() as wgpu::BufferAddress,
                    shader_location: 9,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 19]>() as wgpu::BufferAddress,
                    shader_location: 10,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 22]>() as wgpu::BufferAddress,
                    shader_location: 11,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}

