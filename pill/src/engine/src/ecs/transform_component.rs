
use cgmath::Zero;

use crate::ecs::component::Component;
use crate::Scene;

use super::entity::Entity;

pub struct TransformComponent {
    pub position: cgmath::Vector3<f32>,
    pub rotation: cgmath::Quaternion<f32>,
    pub scale: cgmath::Vector3<f32>,
}

impl Component for TransformComponent {
    fn get_component_type(&self) -> String {
        "Transform".to_string()
    }

    fn new<'a>(scene: &'a mut Scene, entity: &Entity) -> &'a Self {
        let component = TransformComponent {
            position: cgmath::Vector3::<f32>::zero(),
            rotation: cgmath::Quaternion::<f32>::zero(),
            scale: cgmath::Vector3::<f32>::zero(),
        };

        scene.transform_components.insert(entity.index, component);

        //&component
        scene.transform_components.get(entity.index).unwrap()
    }
}

impl Default for TransformComponent {

    fn default() -> Self 
    { 
        let component = TransformComponent {
            position: cgmath::Vector3::<f32>::zero(),
            rotation: cgmath::Quaternion::<f32>::zero(),
            scale: cgmath::Vector3::<f32>::zero(),
        };
        component
    }
}