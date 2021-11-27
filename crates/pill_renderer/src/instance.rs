use cgmath::SquareMatrix;
use pill_engine::game::TransformComponent;

use crate::mesh::Vertex;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub(crate) model: [[f32; 4]; 4], // It is not possible to use cgmath with bytemuck directly. Conversion from Quaternion into a 4x4 f32 array (matrix) needed
    pub(crate) normal: [[f32; 3]; 3], // It is matrix3 because we only need the rotation componen
}

impl Instance {
    pub fn new(transform_component: &TransformComponent) -> Instance {
        Instance {
            model: cgmath::Matrix4::identity().into(),
            normal: cgmath::Matrix3::identity().into(),
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
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x4,
                },
                // A mat4 takes up 4 vertex slots as it is technically 4 vec4s. We need to define a slot for each vec4. We don't have to do this in code though.
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

// [TODO] Move this to core
pub trait MatrixAngleExt<S: cgmath::BaseFloat> {
    fn from_euler_angles(v: cgmath::Vector3<S>) -> Self;
}

pub trait MatrixModelExt<S: cgmath::BaseFloat> {
    fn model(position: cgmath::Vector3<S>, rotation: cgmath::Vector3<S>, scale: cgmath::Vector3<S>) -> Self;
}

impl<S: cgmath::BaseFloat> MatrixAngleExt<S> for cgmath::Matrix4<S> {
    fn from_euler_angles(v: cgmath::Vector3<S>) -> Self {
        #[cfg_attr(rustfmt, rustfmt_skip)]
        cgmath::Matrix4::<S>::from(
            cgmath::Matrix3::from_angle_z(cgmath::Deg(v.z)) *
            cgmath::Matrix3::from_angle_y(cgmath::Deg(v.y)) * 
            cgmath::Matrix3::from_angle_x(cgmath::Deg(v.x)))
    } 
}

impl<S: cgmath::BaseFloat> MatrixModelExt<S> for cgmath::Matrix4<S> {
    fn model(position: cgmath::Vector3<S>, rotation: cgmath::Vector3<S>, scale: cgmath::Vector3<S>) -> Self {
        cgmath::Matrix4::from_translation(position) * 
        cgmath::Matrix4::from_euler_angles(rotation) * 
        cgmath::Matrix4::from_nonuniform_scale(scale.x, scale.y, scale.z)
    }   
}

impl<S: cgmath::BaseFloat> MatrixAngleExt<S> for cgmath::Matrix3<S> {
    fn from_euler_angles(v: cgmath::Vector3<S>) -> Self {
        cgmath::Matrix3::from_angle_z(cgmath::Deg(v.z)) *
        cgmath::Matrix3::from_angle_y(cgmath::Deg(v.y)) * 
        cgmath::Matrix3::from_angle_x(cgmath::Deg(v.x))
    }
}