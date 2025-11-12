use pill_engine::internal::{
    TransformComponent,
    CameraComponent
};

use anyhow::{ Result };
use wgpu::util::DeviceExt;
use glam::{Mat3, Mat4, Vec3, Vec4};

use crate::config::{
    CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, 
    MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX
};

#[rustfmt::skip]
pub const OPENGL_TO_WGPU_MATRIX: Mat4 = Mat4::from_cols_array(&[
    1.0, 0.0, 0.0, 0.0, // column 1
    0.0, 1.0, 0.0, 0.0, // column 2
    0.0, 0.0, 0.5, 0.5, // column 3
    0.0, 0.0, 0.0, 1.0, // column 4
]);

// --- Camera Uniform ---

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraParametersData {
    pub position: [f32; 4], // Camera position
    pub view_projection_matrix: [[f32; 4]; 4], // Perspective manipulation
}

impl CameraParametersData {
    pub fn new() -> Self {
        Self {
            position: Vec4::ZERO.to_array(),
            view_projection_matrix: Mat4::IDENTITY.to_cols_array_2d(),
        }
    }

    pub fn update_data(&mut self, camera_component: &CameraComponent, transform_component: &TransformComponent) {
        // Update position
        self.position = Vec4::new(
            transform_component.position.x,
            transform_component.position.y,
            transform_component.position.z,
            0.0
        ).to_array();

        // Update view-projection
        self.view_projection_matrix = (CameraUniform::calculate_projection_matrix(camera_component) * CameraUniform::calculate_view_matrix(transform_component)).to_cols_array_2d();
    }

    fn calculate_view_matrix(transform_component: &TransformComponent) -> Mat4 {
        let roll_matrix  = Mat3::from_rotation_z(transform_component.rotation.z.to_radians());
        let yaw_matrix  = Mat3::from_rotation_y(transform_component.rotation.y.to_radians());
        let pitch_matrix  = Mat3::from_rotation_x(transform_component.rotation.x.to_radians());
        let rotation_matrix = yaw_matrix * pitch_matrix * roll_matrix;
        let direction  = rotation_matrix * Vec3::Z;

        Mat4::look_to_rh(
            transform_component.position,
            direction,
            Vec3::Y
        )
    }

    fn calculate_projection_matrix(camera_component: &CameraComponent) -> Mat4 {
        OPENGL_TO_WGPU_MATRIX * Mat4::perspective_rh(
            camera_component.fov.to_radians(),
            camera_component.aspect.get_value(),
            camera_component.range.start,
            camera_component.range.end
        )
    }
}

// --- Camera ---

#[derive(Debug)]
pub struct RendererCamera {
    pub parameters_data: CameraParametersData,
    pub parameters_uniform_buffer: wgpu::Buffer,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl RendererCamera {
    pub fn new(device: &wgpu::Device, camera_bind_group_layout: wgpu::BindGroupLayout) -> Result<Self> {

        let parameters_data = CameraParametersData::new();

        let parameters_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("camera_parameters_buffer"),
            contents: bytemuck::cast_slice(&[parameters_data]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0, // (set = X, binding = 0)
                resource: parameters_uniform_buffer.as_entire_binding(),
            }],
            label: Some("camera_parameters_bind_group"),
        });

        let camera = Self {
            parameters_data,
            parameters_uniform_buffer,
            bind_group_layout: camera_bind_group_layout,
            bind_group,
        };

        Ok(camera)
    }

    pub fn update(&mut self, queue: &wgpu::Queue, camera_component: &CameraComponent, transform_component: &TransformComponent) {
        self.parameters_data.update_data(camera_component, transform_component);
        queue.write_buffer(&self.parameters_uniform_buffer, 0, bytemuck::cast_slice(&[self.parameters_data]));
    }
}

