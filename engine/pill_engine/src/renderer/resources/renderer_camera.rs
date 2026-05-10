use crate::ecs::{CameraComponent, TransformComponent};

use pill_core::Result;
use pill_core::{Matrix3f, Matrix4f, Vector3f, Vector4f};
use wgpu::util::DeviceExt;

#[rustfmt::skip]
pub const OPENGL_TO_WGPU_MATRIX: Matrix4f = Matrix4f::from_cols_array(&[
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 0.5, 0.5,
    0.0, 0.0, 0.0, 1.0,
]);

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CameraParametersData {
    pub position: Vector4f,
    pub view_projection_matrix: Matrix4f,
}

impl Default for CameraParametersData {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraParametersData {
    pub fn new() -> Self {
        Self {
            position: Vector4f::ZERO,
            view_projection_matrix: Matrix4f::IDENTITY,
        }
    }

    /// Writes the latest camera position and view-projection matrix from ECS components into the CPU-side struct.
    pub fn update_data(
        &mut self,
        camera_component: &CameraComponent,
        transform_component: &TransformComponent,
    ) {
        self.position = Vector4f::new(
            transform_component.position.x,
            transform_component.position.y,
            transform_component.position.z,
            0.0,
        );

        self.view_projection_matrix =
            CameraParametersData::calculate_projection_matrix(camera_component)
                * CameraParametersData::calculate_view_matrix(transform_component);
    }

    fn calculate_view_matrix(transform_component: &TransformComponent) -> Matrix4f {
        let roll_matrix = Matrix3f::from_rotation_z(transform_component.rotation.z.to_radians());
        let yaw_matrix = Matrix3f::from_rotation_y(transform_component.rotation.y.to_radians());
        let pitch_matrix = Matrix3f::from_rotation_x(transform_component.rotation.x.to_radians());
        let rotation_matrix = yaw_matrix * pitch_matrix * roll_matrix;
        let direction = rotation_matrix * Vector3f::Z;

        Matrix4f::look_to_rh(transform_component.position, direction, Vector3f::Y)
    }

    fn calculate_projection_matrix(camera_component: &CameraComponent) -> Matrix4f {
        OPENGL_TO_WGPU_MATRIX
            * Matrix4f::perspective_rh(
                camera_component.fov.to_radians(),
                camera_component.aspect.get_value(),
                camera_component.range.start,
                camera_component.range.end,
            )
    }
}

#[derive(Debug)]
pub struct RendererCamera {
    pub parameters_data: CameraParametersData,
    pub parameters_uniform_buffer: wgpu::Buffer,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl RendererCamera {
    /// Allocates the camera uniform buffer and bind group using the provided layout.
    pub fn new(
        device: &wgpu::Device,
        camera_bind_group_layout: wgpu::BindGroupLayout,
    ) -> Result<Self> {
        let parameters_data = CameraParametersData::new();

        let parameters_uniform_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("camera_parameters_buffer"),
                contents: bytemuck::cast_slice(&[parameters_data]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &camera_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: parameters_uniform_buffer.as_entire_binding(),
            }],
            label: Some("camera_parameters_bind_group"),
        });

        Ok(Self {
            parameters_data,
            parameters_uniform_buffer,
            bind_group_layout: camera_bind_group_layout,
            bind_group,
        })
    }

    /// Recomputes view-projection from ECS components and uploads the result to the GPU buffer.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        camera_component: &CameraComponent,
        transform_component: &TransformComponent,
    ) {
        self.parameters_data
            .update_data(camera_component, transform_component);
        queue.write_buffer(
            &self.parameters_uniform_buffer,
            0,
            bytemuck::cast_slice(&[self.parameters_data]),
        );
    }
}
