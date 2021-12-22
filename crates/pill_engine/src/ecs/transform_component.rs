use pill_core::Vector3f;
use cgmath::Zero;

pub use crate::ecs::{Component, ComponentStorage };

pub struct TransformComponent {
    pub position: Vector3f,
    pub rotation: Vector3f,
    pub scale: Vector3f,
}

impl Component for TransformComponent {
    type Storage = ComponentStorage<TransformComponent>; 
}

impl TransformComponent {
    pub fn new(position: Vector3f, rotation: Vector3f, scale: Vector3f) -> Self {  
        return Self { 
            position,
            rotation,
            scale,
        };
    }
}

impl Default for TransformComponent {
    fn default() -> Self {
        Self { 
            position: Vector3f::zero(),
            rotation: Vector3f::zero(),
            scale: Vector3f::new(1.0, 1.0, 1.0),
        }
    }
}
