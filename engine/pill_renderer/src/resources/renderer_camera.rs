use cgmath::{EuclideanSpace, SquareMatrix, Zero};
use pill_core::{Color, RendererError, Vector3f};
use pill_engine::internal::{
    CameraComponent, RendererCameraHandle, TransformComponent
};

use anyhow::{ Result };
use wgpu::util::DeviceExt;
use std::{f32::consts::FRAC_PI_2, ops::Range};

use crate::{config::{
    CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, 
    MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX
}, resources::RendererResourceStorage};

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
pub struct CameraParametersData {
    pub position: [f32; 4], // Camera position
    pub view_projection_matrix: [[f32; 4]; 4], // Perspective manipulation
}

impl CameraParametersData {
    pub fn new() -> Self {
        Self {
            position: cgmath::Vector4::zero().into(),
            view_projection_matrix: cgmath::Matrix4::identity().into(),
        }
    }

    pub fn update_data(
        &mut self,
        position: Vector3f,
        rotation: Vector3f,
        fov: f32,
        aspect: f32,
        range: Range<f32>
    ) {
        // Update position
        self.position = cgmath::Vector4::<f32> { 
            x: position.x, 
            y: position.y, 
            z: position.z, 
            w: 0.0
        }.into();

        // Update view-projection
        let view_matrix = CameraParametersData::calculate_view_matrix(position, rotation);
        let projection_matrix = CameraParametersData::calculate_projection_matrix(fov, aspect, range);
        self.view_projection_matrix = (projection_matrix * view_matrix).into();
    }

    fn calculate_view_matrix(position: Vector3f, rotation: Vector3f) -> cgmath::Matrix4::<f32> {
        let position = cgmath::Point3::from_vec(position);

        let roll_matrix  = cgmath::Matrix3::from_angle_z(cgmath::Deg(rotation.z));
        let yaw_matrix  = cgmath::Matrix3::from_angle_y(cgmath::Deg(rotation.y));
        let pitch_matrix  = cgmath::Matrix3::from_angle_x(cgmath::Deg(rotation.x));
        let rotation_matrix = yaw_matrix * pitch_matrix * roll_matrix;
        let direction  = rotation_matrix * cgmath::Vector3::<f32>::unit_z();

        cgmath::Matrix4::look_to_rh(
            position,
            direction,
            cgmath::Vector3::unit_y()
        )
    }

    fn calculate_projection_matrix(fov: f32, aspect: f32, range: Range<f32>) -> cgmath::Matrix4::<f32> {
        OPENGL_TO_WGPU_MATRIX * cgmath::perspective(
            cgmath::Deg(fov), 
            aspect,
            range.start,
            range.end
        )
    }
}

// --- Camera ---

#[derive(Debug)]
pub struct RendererCamera {
    pub clear_color: Color,
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
            clear_color: Color::new(0.15, 0.15, 0.15),
            parameters_data,
            parameters_uniform_buffer,
            bind_group_layout: camera_bind_group_layout,
            bind_group,
        };

        Ok(camera)
    }

    pub fn update_parameters(
        device: &wgpu::Device, 
        queue: &wgpu::Queue, 
        renderer_camera_handle: RendererCameraHandle,
        rendering_resource_storage: &mut RendererResourceStorage,
        position: Vector3f,
        rotation: Vector3f,
        fov: f32,
        aspect: f32,
        range: Range<f32>,
        clear_color: Color
    ) -> Result<()> {
        let camera = rendering_resource_storage.cameras.get_mut(renderer_camera_handle)
            .ok_or(RendererError::RendererResourceNotFound)?;

        camera.parameters_data.update_data(
            position,
            rotation,
            fov,
            aspect,
            range
        );

        camera.clear_color = clear_color;
        queue.write_buffer(&camera.parameters_uniform_buffer, 0, bytemuck::cast_slice(&[camera.parameters_data]));

        Ok(())
    }
}

