use crate::ecs::{Component, ComponentStorage};
use pill_core::{Direction, Matrix3f, PillTypeMapKey, Vector3f};
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

// Position, rotation (degrees) and scale only; the GPU builds the model matrix from these.
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
        self.rotation_matrix() * Vector3f::new(0.0, 0.0, -1.0)
    }

    pub fn get_backward_direction(&self) -> Vector3f {
        self.rotation_matrix() * Vector3f::new(0.0, 0.0, 1.0)
    }

    pub fn get_right_direction(&self) -> Vector3f {
        self.rotation_matrix() * Vector3f::new(1.0, 0.0, 0.0)
    }

    pub fn get_left_direction(&self) -> Vector3f {
        self.rotation_matrix() * Vector3f::new(-1.0, 0.0, 0.0)
    }

    pub fn get_up_direction(&self) -> Vector3f {
        self.rotation_matrix() * Vector3f::new(0.0, 1.0, 0.0)
    }

    pub fn get_down_direction(&self) -> Vector3f {
        self.rotation_matrix() * Vector3f::new(0.0, -1.0, 0.0)
    }

    /// Rx·Ry·Rz of `rotation` (degrees): the order used by the renderer and physics (glam EulerRot::XYZ).
    pub fn rotation_matrix(&self) -> Matrix3f {
        Matrix3f::from_rotation_x(self.rotation.x.to_radians())
            * Matrix3f::from_rotation_y(self.rotation.y.to_radians())
            * Matrix3f::from_rotation_z(self.rotation.z.to_radians())
    }

    // --- Rotation ---

    pub fn set_rotation(&mut self, rotation: Vector3f) {
        self.rotation = rotation;
    }

    /// Rotates (without roll) so that +Z, the camera view direction, points at `target`.
    pub fn look_at(&mut self, target: Vector3f) {
        let direction = (target - self.position).normalize_or_zero();
        if direction == Vector3f::ZERO {
            return;
        }
        // Rx(a)·Ry(b)·Z = (sin b, -sin a·cos b, cos a·cos b)
        let yaw = direction.x.clamp(-1.0, 1.0).asin();
        let pitch = (-direction.y).atan2(direction.z);
        self.rotation = Vector3f::new(pitch.to_degrees(), yaw.to_degrees(), 0.0);
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

impl PillTypeMapKey for TransformComponent {
    type Storage = ComponentStorage<TransformComponent>;
}

impl Component for TransformComponent {}

impl Default for TransformComponent {
    fn default() -> Self {
        Self::new()
    }
}
