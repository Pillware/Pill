use crate::renderer::resources::Vertex;

use crate::ecs::TransformComponent;
use pill_core::{Matrix3f, Vector3f};

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub(crate) transform: Matrix3f,
}

impl Instance {
    /// Creates an instance from a transform component's position, rotation, and scale.
    pub fn new(transform_component: &TransformComponent) -> Instance {
        let rot_radians = Vector3f::new(
            transform_component.rotation.x.to_radians(),
            transform_component.rotation.y.to_radians(),
            transform_component.rotation.z.to_radians(),
        );
        Instance {
            transform: Matrix3f::from_cols(
                transform_component.position,
                rot_radians,
                transform_component.scale,
            ),
        }
    }
}

impl Vertex for Instance {
    fn data_layout_descriptor<'a>() -> wgpu::VertexBufferLayout<'a> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Instance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 3]>() as wgpu::BufferAddress,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 6]>() as wgpu::BufferAddress,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        }
    }
}
