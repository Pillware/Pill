use crate::{
    ecs::{
        Component, ComponentStorage, DeferredUpdateComponent, DeferredUpdateComponentRequest, DeferredUpdateManagerPointer, EntityHandle, SceneHandle
    }, engine::Engine
};
use pill_core::{
    get_type_name, Direction, PillStyle, PillTypeMap, PillTypeMapKey, Vector3f
};
use cgmath::{Deg, Matrix3, SquareMatrix, Zero};
use anyhow::{ Result, Context, Error };
use serde::{ Serialize, Deserialize };

// Coordinate system:
//
//     +Y (up)
//     |
//     |
//     |_______ +X (right)
//    /
//   /
//  +Z (backward)
//

// --- Builder ---

pub struct TransformComponentBuilder {
    component: TransformComponent,
}

impl TransformComponentBuilder {
    pub fn default() -> Self {
        Self {
            component: TransformComponent::new(),
        }
    }

    pub fn position(mut self, position: Vector3f) -> Self {
        self.component.position = position;
        self
    }

    pub fn rotation(mut self, rotation: Vector3f) -> Self {
        self.component.rotation = rotation;
        self
    }

    pub fn scale(mut self, scale: Vector3f) -> Self {
        self.component.scale = scale;
        self
    }

    pub fn build(self) -> TransformComponent {
        self.component
    }
}

// --- Transform Component ---

// NOTE: Setting position/rotation/scale directly is not possible since we need to update matrices after each change
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[readonly::make]
pub struct TransformComponent {
    #[readonly]
    pub position: Vector3f,
    #[readonly]
    pub rotation: Vector3f,
    #[readonly]
    pub scale: Vector3f,

    model_matrix: [[f32; 4]; 4],
    normal_matrix: [[f32; 3]; 3],

    // There may me multiple updates of the position/rotation/scale in the single frame.
    // Not to calculate matrices multiple times, we will update them only once per frame
    // The update happens in the rendering system
    pub matrix_update_required: bool,
    pub net_dirty: bool,
}

impl TransformComponent {
    pub fn builder() -> TransformComponentBuilder {
        TransformComponentBuilder::default()
    }

    pub fn new() -> Self {
        Self {
            position: Vector3f::zero(),
            rotation: Vector3f::zero(),
            scale: Vector3f::new(1.0, 1.0, 1.0),
            model_matrix: cgmath::Matrix4::identity().into(),
            normal_matrix: cgmath::Matrix3::identity().into(),
            matrix_update_required: true,
            net_dirty: false,
        }
    }

    // --- Position ---

    pub fn set_position(&mut self, position: Vector3f) {
        self.position = position;
        self.matrix_update_required = true;
    }

    pub fn translate(&mut self, delta: f32, direction: Direction) {
        match direction {
            Direction::Forward => self.position += self.get_forward_direction() * delta,
            Direction::Backward => self.position += self.get_backward_direction() * delta,
            Direction::Right => self.position += self.get_right_direction() * delta,
            Direction::Left => self.position += self.get_left_direction() * delta,
            Direction::Up => self.position += self.get_up_direction() * delta,
            Direction::Down => self.position += self.get_down_direction() * delta,
            Direction::WorldForward => self.position.z -= delta,
            Direction::WorldBackward => self.position.z += delta,
            Direction::WorldRight => self.position.x += delta,
            Direction::WorldLeft => self.position.x -= delta,
            Direction::WorldUp => self.position.y += delta,
            Direction::WorldDown => self.position.y -= delta
        }
        self.matrix_update_required = true;
    }

    pub fn translate_world(&mut self, delta: Vector3f) {
        self.position += delta;
        self.matrix_update_required = true;
    }

    pub fn translate_local(&mut self, delta: Vector3f) {
        self.position += self.get_forward_direction() * delta.z +
                        self.get_right_direction() * delta.x +
                        self.get_up_direction() * delta.y;
        self.matrix_update_required = true;
    }

    // --- Directions ---

    pub fn get_forward_direction(&self) -> Vector3f {
        self.get_rotation_matrix() * Vector3f::new(0.0, 0.0, -1.0)
    }

    pub fn get_backward_direction(&self) -> Vector3f {
        self.get_rotation_matrix() * Vector3f::new(0.0, 0.0, 1.0)
    }

    pub fn get_right_direction(&self) -> Vector3f {
        self.get_rotation_matrix() * Vector3f::new(1.0, 0.0, 0.0)
    }

    pub fn get_left_direction(&self) -> Vector3f {
        self.get_rotation_matrix() * Vector3f::new(-1.0, 0.0, 0.0)
    }

    pub fn get_up_direction(&self) -> Vector3f {
        self.get_rotation_matrix() * Vector3f::new(0.0, 1.0, 0.0)
    }

    pub fn get_down_direction(&self) -> Vector3f {
        self.get_rotation_matrix() * Vector3f::new(0.0, -1.0, 0.0)
    }

    fn get_rotation_matrix(&self) -> Matrix3<f32> {
        let roll = Matrix3::from_angle_z(Deg(self.rotation.z));
        let yaw = Matrix3::from_angle_y(Deg(self.rotation.y));
        let pitch = Matrix3::from_angle_x(Deg(self.rotation.x));
        yaw * pitch * roll
    }

    // --- Rotation ---

    pub fn set_rotation(&mut self, rotation: Vector3f) {
        self.rotation = rotation;
        self.matrix_update_required = true;
    }

    // TODO: Implement quaternion rotation
    pub fn rotate_around_axis(&mut self, angle: f32, axis: Vector3f) {
        self.rotation += angle * axis;
        self.matrix_update_required = true;
    }

    // --- Scale ---

    pub fn set_scale(&mut self, scale: Vector3f) {
        self.scale = scale;
        self.matrix_update_required = true;
    }

}

pub fn update_transform_matrices(transform_component: &mut TransformComponent) {
    transform_component.model_matrix = cgmath::Matrix4::model(transform_component.position, transform_component.rotation, transform_component.scale).into();
    transform_component.normal_matrix = cgmath::Matrix3::from_euler_angles(transform_component.rotation).into();
}

pub fn get_model_matrix(transform_component: &TransformComponent) -> [[f32; 4]; 4] {
    transform_component.model_matrix
}

pub fn get_normal_matrix(transform_component: &TransformComponent) -> [[f32; 3]; 3] {
    transform_component.normal_matrix
}

impl PillTypeMapKey for TransformComponent {
    type Storage = ComponentStorage<TransformComponent>;
}

impl Component for TransformComponent {

}

impl Default for TransformComponent {
        fn default() -> Self {
                    Self::new()
                            }
}

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