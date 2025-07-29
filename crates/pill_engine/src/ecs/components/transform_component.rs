use crate::{
    ecs::{ 
        Component, ComponentStorage, DeferredUpdateComponent, DeferredUpdateComponentRequest, DeferredUpdateManagerPointer, EntityHandle, SceneHandle
    }, engine::Engine
};
use pill_core::{ 
    get_type_name, Direction, PillStyle, PillTypeMap, PillTypeMapKey, Vector3f, Vector3fExt 
};
use cgmath::{SquareMatrix, Zero};
use anyhow::{ Result, Context, Error };

// Coordinate system:
//
//     +Y (up)
//     |
//     |
//     |_______ +X (right)
//    /
//   /
//  +Z (forward)
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
            matrix_update_required: false,
        }
    }

    // --- Position ---

    pub fn set_position(&mut self, position: Vector3f) {
        self.position = position;
        self.matrix_update_required = true;
    }

    pub fn translate(&mut self, delta: Vector3f, direction: Direction) {
        match direction {
            Direction::Forward => self.position += self.get_forward_direction() * delta.z,
            Direction::Right => self.position += self.get_right_direction() * delta.x,
            Direction::Up => self.position += self.get_up_direction() * delta.y,
            Direction::WorldForward => self.position.z += delta.z,
            Direction::WorldRight => self.position.x += delta.x,
            Direction::WorldUp => self.position.y += delta.y
        }
        self.matrix_update_required = true;
    }

    // --- Directions ---

    pub fn get_forward_direction(&self) -> Vector3f {
        let pitch = self.rotation.x.to_radians();
        let yaw: f32 = self.rotation.y.to_radians();
        Vector3f::new(yaw.sin() * pitch.cos(), -pitch.sin(), yaw.cos() * pitch.cos())
    }

    pub fn get_right_direction(&self) -> Vector3f {
        let pitch = self.rotation.x.to_radians();
        let yaw = self.rotation.y.to_radians();
        Vector3f::new(yaw.cos(), 0.0, -yaw.sin())
    }

    pub fn get_up_direction(&self) -> Vector3f {
        let pitch = self.rotation.x.to_radians();
        let yaw = self.rotation.y.to_radians();
        Vector3f::new(0.0, pitch.cos(), 0.0)
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