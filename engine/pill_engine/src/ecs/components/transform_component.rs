use crate::ecs::{Component, ComponentStorage};
use pill_core::{Direction, Matrix3f, Matrix3fA, Matrix4f, PillTypeMapKey, Vector3f};
use serde::{Deserialize, Serialize};

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

// 36 bytes hot (pos+rot+scale only); model_matrix/normal_matrix were dead weight — GPU
// computes matrices from pos/rot/scale sent as instance data. [Acton "Data-Oriented Design
// and C++" CppCon 2014 — hot/cold struct split reduces cache lines per entity 152→36 B]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[readonly::make]
pub struct TransformComponent {
    #[readonly]
    pub position: Vector3f,
    #[readonly]
    pub rotation: Vector3f,
    #[readonly]
    pub scale: Vector3f,
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
        }
    }

    // --- Position ---

    pub fn set_position(&mut self, position: Vector3f) {
        self.position = position;
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
            Direction::WorldDown => self.position.y -= delta,
        }
    }

    pub fn translate_world(&mut self, delta: Vector3f) {
        self.position += delta;
    }

    pub fn translate_local(&mut self, delta: Vector3f) {
        self.position += self.get_forward_direction() * delta.z
            + self.get_right_direction() * delta.x
            + self.get_up_direction() * delta.y;
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
    }

    // TODO: Implement quaternion rotation
    pub fn rotate_around_axis(&mut self, angle: f32, axis: Vector3f) {
        self.rotation += angle * axis;
    }

    // --- Scale ---

    pub fn set_scale(&mut self, scale: Vector3f) {
        self.scale = scale;
    }
}

// Kept for backward compatibility — no longer needed since the renderer
// computes matrices on the GPU from pos/rot/scale instance data.
pub fn update_transform_matrices(_transform_component: &mut TransformComponent) {}

pub fn get_model_matrix(_transform_component: &TransformComponent) -> Matrix4f {
    Matrix4f::IDENTITY
}

pub fn get_normal_matrix(_transform_component: &TransformComponent) -> Matrix3fA {
    Matrix3fA::IDENTITY
}

impl PillTypeMapKey for TransformComponent {
    type Storage = ComponentStorage<TransformComponent>;
}

impl Component for TransformComponent {}

impl Default for TransformComponent {
    fn default() -> Self {
        Self::new()
    }
}

pub trait Matrix3fAngleExt {
    fn from_euler_angles(rotation_deg: Vector3f) -> Matrix3f;
}

pub trait Matrix4fModelExt {
    fn model(position: Vector3f, rotation_deg: Vector3f, scale: Vector3f) -> Matrix4f;
    fn from_euler_angles(rotation_deg: Vector3f) -> Matrix4f;
}

impl Matrix3fAngleExt for Matrix3f {
    fn from_euler_angles(rotation_deg: Vector3f) -> Matrix3f {
        let rz = Matrix3f::from_rotation_z(rotation_deg.z.to_radians());
        let ry = Matrix3f::from_rotation_y(rotation_deg.y.to_radians());
        let rx = Matrix3f::from_rotation_x(rotation_deg.x.to_radians());
        rz * ry * rx
    }
}

impl Matrix4fModelExt for Matrix4f {
    fn model(position: Vector3f, rotation_deg: Vector3f, scale: Vector3f) -> Matrix4f {
        let rz = Matrix3f::from_rotation_z(rotation_deg.z.to_radians());
        let ry = Matrix3f::from_rotation_y(rotation_deg.y.to_radians());
        let rx = Matrix3f::from_rotation_x(rotation_deg.x.to_radians());
        let rot3 = rz * ry * rx;

        let t = Matrix4f::from_translation(position);
        let r = Matrix4f::from_mat3(rot3);
        let s = Matrix4f::from_scale(scale);

        t * r * s
    }

    fn from_euler_angles(rotation_deg: Vector3f) -> Matrix4f {
        let rz = Matrix3f::from_rotation_z(rotation_deg.z.to_radians());
        let ry = Matrix3f::from_rotation_y(rotation_deg.y.to_radians());
        let rx = Matrix3f::from_rotation_x(rotation_deg.x.to_radians());
        let rot3 = rz * ry * rx;
        Matrix4f::from_mat3(rot3)
    }
}
