
pub use crate::ecs::{Component, ComponentStorage };
use cgmath::{Zero};

pub struct TransformComponent {
    pub position: cgmath::Vector3<f32>,
    pub rotation: cgmath::Quaternion<f32>,
    pub scale: cgmath::Vector3<f32>,
}

impl Component for TransformComponent {
    type Storage = ComponentStorage<TransformComponent>; 
}

impl Default for TransformComponent {
    fn default() -> Self {
        Self { 
            position: cgmath::Vector3::<f32>::zero(),
            rotation: cgmath::Quaternion::<f32>::zero(),
            scale: cgmath::Vector3::<f32>::zero(),
        }
    }
}
