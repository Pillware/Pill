use cgmath::{EuclideanSpace, SquareMatrix, Zero};
use pill_engine::internal::{
    TransformComponent,
    CameraComponent
};

use anyhow::{ Result };
use wgpu::util::DeviceExt;
use std::f32::consts::FRAC_PI_2;

#[rustfmt::skip]
pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::new(
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 0.5, 0.0,
    0.0, 0.0, 0.5, 1.0,
);

// --- Camera Uniform ---

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct CameraUniform {
    pub(crate) position: [f32; 4], // Camera position
    pub(crate) view_projection_matrix: [[f32; 4]; 4], // Perspective manipulation
}

impl CameraUniform {
    pub fn new() -> Self {
        Self {
            position: cgmath::Vector4::zero().into(),
            view_projection_matrix: cgmath::Matrix4::identity().into(),
        }
    }

    pub fn update_data(&mut self, camera_component: &CameraComponent, transform_component: &TransformComponent) {
        // Update position
        self.position = cgmath::Vector4::<f32> { 
            x: transform_component.position.x, 
            y: transform_component.position.y, 
            z: transform_component.position.z, 
            w: 0.0
        }.into();

        // Update view-projection
        self.view_projection_matrix = (CameraUniform::calculate_projection_matrix(camera_component) * CameraUniform::calculate_view_matrix(transform_component)).into();
    }

    fn calculate_view_matrix(transform_component: &TransformComponent) -> cgmath::Matrix4::<f32> {
        let position = cgmath::Point3::from_vec(transform_component.position);

        let roll_matrix  = cgmath::Matrix3::from_angle_z(cgmath::Deg(transform_component.rotation.z));
        let yaw_matrix  = cgmath::Matrix3::from_angle_y(cgmath::Deg(transform_component.rotation.y));
        let pitch_matrix  = cgmath::Matrix3::from_angle_x(cgmath::Deg(transform_component.rotation.x));
        let rotation_matrix = yaw_matrix * pitch_matrix * roll_matrix;
        let direction  = rotation_matrix * cgmath::Vector3::<f32>::unit_z();

        cgmath::Matrix4::look_to_rh(
            position,
            direction,
            cgmath::Vector3::unit_y()
        )
    }

    fn calculate_projection_matrix(camera_component: &CameraComponent) -> cgmath::Matrix4::<f32> {
        OPENGL_TO_WGPU_MATRIX * cgmath::perspective(
            cgmath::Deg(camera_component.fov), 
            camera_component.aspect.get_value(), 
            camera_component.range.start,
            camera_component.range.end
        )
    }
}

// --- Camera ---

#[derive(Debug)]
pub struct RendererCamera {
    pub(crate) uniform: CameraUniform,
    buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
}

impl RendererCamera {
    pub fn new(device: &wgpu::Device, camera_bind_group_layout: &wgpu::BindGroupLayout) -> Result<Self> {

        let uniform = CameraUniform::new();

        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera_buffer"),
            contents: bytemuck::cast_slice(&[uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
            label: Some("camera_bind_group"),
        });

        let camera = Self {
            uniform,
            buffer,
            bind_group,
        };

        Ok(camera)
    }

    pub fn update(&mut self, queue: &wgpu::Queue, camera_component: &CameraComponent, transform_component: &TransformComponent) {
        self.uniform.update_data(camera_component, transform_component);
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&[self.uniform]));
    }
}
