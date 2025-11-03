use crate::{
    ecs::{
        Component, ComponentStorage, DeferredUpdateComponent, DeferredUpdateComponentRequest, DeferredUpdateManagerPointer, EntityHandle, SceneHandle
    }, engine::Engine
};
use pill_core::{
    get_type_name, Direction, PillStyle, PillTypeMap, PillTypeMapKey
};
use glam::{Vec3, Mat3, Mat4};
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

    pub fn position(mut self, position: Vec3) -> Self {
        self.component.position = position;
        self
    }

    pub fn rotation(mut self, rotation: Vec3) -> Self {
        self.component.rotation = rotation;
        self
    }

    pub fn scale(mut self, scale: Vec3) -> Self {
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
    pub position: Vec3,
    #[readonly]
    pub rotation: Vec3,
    #[readonly]
    pub scale: Vec3,

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
            position: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::new(1.0, 1.0, 1.0),
            model_matrix: Mat4::IDENTITY.to_cols_array_2d(),
            normal_matrix: Mat3::IDENTITY.to_cols_array_2d(),
            matrix_update_required: true,
            net_dirty: false,
        }
    }

    // --- Position ---

    pub fn set_position(&mut self, position: Vec3) {
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

    pub fn translate_world(&mut self, delta: Vec3) {
        self.position += delta;
        self.matrix_update_required = true;
    }

    pub fn translate_local(&mut self, delta: Vec3) {
        self.position += self.get_forward_direction() * delta.z +
                        self.get_right_direction() * delta.x +
                        self.get_up_direction() * delta.y;
        self.matrix_update_required = true;
    }

    // --- Directions ---

    pub fn get_forward_direction(&self) -> Vec3 {
        self.get_rotation_matrix() * Vec3::new(0.0, 0.0, -1.0)
    }

    pub fn get_backward_direction(&self) -> Vec3 {
        self.get_rotation_matrix() * Vec3::new(0.0, 0.0, 1.0)
    }

    pub fn get_right_direction(&self) -> Vec3 {
        self.get_rotation_matrix() * Vec3::new(1.0, 0.0, 0.0)
    }

    pub fn get_left_direction(&self) -> Vec3 {
        self.get_rotation_matrix() * Vec3::new(-1.0, 0.0, 0.0)
    }

    pub fn get_up_direction(&self) -> Vec3 {
        self.get_rotation_matrix() * Vec3::new(0.0, 1.0, 0.0)
    }

    pub fn get_down_direction(&self) -> Vec3 {
        self.get_rotation_matrix() * Vec3::new(0.0, -1.0, 0.0)
    }

    fn get_rotation_matrix(&self) -> Mat3 {
        let roll = Mat3::from_rotation_z(self.rotation.z.to_radians());
        let yaw = Mat3::from_rotation_y(self.rotation.y.to_radians());
        let pitch = Mat3::from_rotation_x(self.rotation.x.to_radians());
        yaw * pitch * roll
    }

    // --- Rotation ---

    pub fn set_rotation(&mut self, rotation: Vec3) {
        self.rotation = rotation;
        self.matrix_update_required = true;
    }

    // TODO: Implement quaternion rotation
    pub fn rotate_around_axis(&mut self, angle: f32, axis: Vec3) {
        self.rotation += angle * axis;
        self.matrix_update_required = true;
    }

    // --- Scale ---

    pub fn set_scale(&mut self, scale: Vec3) {
        self.scale = scale;
        self.matrix_update_required = true;
    }

}

pub fn update_transform_matrices(transform_component: &mut TransformComponent) {
    let model = Mat4::model(transform_component.position, transform_component.rotation, transform_component.scale);
    let normal = Mat3::from_euler_angles(transform_component.rotation);

    transform_component.model_matrix = model.to_cols_array_2d();
    transform_component.normal_matrix = normal.to_cols_array_2d();
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

pub trait Mat3AngleExt {
    fn from_euler_angles(rotation_deg: Vec3) -> Mat3;
}

pub trait Mat4ModelExt {
    fn model(position: Vec3, rotation_deg: Vec3, scale: Vec3) -> Mat4;
    fn from_euler_angles(rotation_deg: Vec3) -> Mat4;
}

impl Mat3AngleExt for Mat3 {
    fn from_euler_angles(rotation_deg: Vec3) -> Mat3 {
        let rz = Mat3::from_angle(rotation_deg.z.to_radians());
        let ry = Mat3::from_angle(rotation_deg.y.to_radians());
        let rx = Mat3::from_angle(rotation_deg.x.to_radians());
        rz * ry * rx
    }
}

impl Mat4ModelExt for Mat4 {
    fn model(position: Vec3, rotation_deg: Vec3, scale: Vec3) -> Mat4 {
        let rz = Mat3::from_angle(rotation_deg.z.to_radians());
        let ry = Mat3::from_angle(rotation_deg.y.to_radians());
        let rx = Mat3::from_angle(rotation_deg.x.to_radians());
        let rot3 = rz * ry * rx;

        let t = Mat4::from_translation(position);
        let r = Mat4::from_mat3(rot3);
        let s = Mat4::from_scale(scale);

        t * r * s
    }

    fn from_euler_angles(rotation_deg: Vec3) -> Mat4 {
        let rz = Mat3::from_angle(rotation_deg.z.to_radians());
        let ry = Mat3::from_angle(rotation_deg.y.to_radians());
        let rx = Mat3::from_angle(rotation_deg.x.to_radians());
        let rot3 = rz * ry * rx;
        Mat4::from_mat3(rot3)
    }
}
