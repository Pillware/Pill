use crate::{
    ecs::{
        Component, ComponentStorage, DeferredUpdateComponent, DeferredUpdateComponentRequest, DeferredUpdateManagerPointer, EntityHandle, SceneHandle
    }, engine::Engine
};
use pill_core::{
    get_type_name, Direction, Matrix3f, Matrix3fA, Matrix4f, PillTypeMap, PillTypeMapKey, Vector3f
};
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

    model_matrix: Matrix4f,
    normal_matrix: Matrix3fA,

    // There may me multiple updates of the position/rotation/scale in the single frame.
    // Not to calculate matrices multiple times, we will update them only once per frame
    // The update happens in the rendering system
    pub matrix_update_required: bool,
}

impl TransformComponent {
    pub fn builder() -> TransformComponentBuilder {
        TransformComponentBuilder::default()
    }

    pub fn new() -> Self {
        Self {
            position: Vector3f::ZERO,
            rotation: Vector3f::ZERO,
            scale: Vector3f::new(1.0, 1.0, 1.0),
            model_matrix: Matrix4f::IDENTITY,
            normal_matrix: Matrix3fA::IDENTITY,
            matrix_update_required: true,
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

    fn get_rotation_matrix(&self) -> Matrix3f {
        let roll = Matrix3f::from_rotation_z(self.rotation.z.to_radians());
        let yaw = Matrix3f::from_rotation_y(self.rotation.y.to_radians());
        let pitch = Matrix3f::from_rotation_x(self.rotation.x.to_radians());
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
    let model = Matrix4f::model(transform_component.position, transform_component.rotation, transform_component.scale);
    let normal = Matrix3f::from_euler_angles(transform_component.rotation);

    transform_component.model_matrix = model;
    transform_component.normal_matrix = normal.into();
}

pub fn get_model_matrix(transform_component: &TransformComponent) -> Matrix4f {
    transform_component.model_matrix
}

pub fn get_normal_matrix(transform_component: &TransformComponent) -> Matrix3fA {
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

pub trait Matrix3fAngleExt {
    fn from_euler_angles(rotation_deg: Vector3f) -> Matrix3f ;
}

pub trait Matrix4fModelExt {
    fn model(position: Vector3f, rotation_deg: Vector3f, scale: Vector3f) -> Matrix4f;
    fn from_euler_angles(rotation_deg: Vector3f) -> Matrix4f;
}

impl Matrix3fAngleExt for Matrix3f {
    fn from_euler_angles(rotation_deg: Vector3f) -> Matrix3f {
        let rz = Matrix3f::from_angle(rotation_deg.z.to_radians());
        let ry = Matrix3f::from_angle(rotation_deg.y.to_radians());
        let rx = Matrix3f::from_angle(rotation_deg.x.to_radians());
        rz * ry * rx
    }
}

impl Matrix4fModelExt for Matrix4f {
    fn model(position: Vector3f, rotation_deg: Vector3f, scale: Vector3f) -> Matrix4f {
        let rz = Matrix3f::from_angle(rotation_deg.z.to_radians());
        let ry = Matrix3f::from_angle(rotation_deg.y.to_radians());
        let rx = Matrix3f::from_angle(rotation_deg.x.to_radians());
        let rot3 = rz * ry * rx;

        let t = Matrix4f::from_translation(position);
        let r = Matrix4f::from_mat3(rot3);
        let s = Matrix4f::from_scale(scale);

        t * r * s
    }

    fn from_euler_angles(rotation_deg: Vector3f) -> Matrix4f {
        let rz = Matrix3f::from_angle(rotation_deg.z.to_radians());
        let ry = Matrix3f::from_angle(rotation_deg.y.to_radians());
        let rx = Matrix3f::from_angle(rotation_deg.x.to_radians());
        let rot3 = rz * ry * rx;
        Matrix4f::from_mat3(rot3)
    }
}
