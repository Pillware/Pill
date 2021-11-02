use crate::resources::resource_manager::ResourceManager;
use crate::{ecs::component::Component};
use crate::{Engine, Scene};

use super::entity::{Entity, EntityHandle};

pub struct MeshRenderingComponent {
    //pub resource_id: Option<usize>,
}

impl Component for MeshRenderingComponent {
    fn get_component_type(&self) -> String {
        "MeshRendering".to_string()
    }

    fn new<'a>(scene: &'a mut Scene, entity_handle: EntityHandle) -> &'a mut Self {

        // Register resource
        //engine.load_resource(MeshRenderingComponent, )
       // register_resource();


        let component = MeshRenderingComponent {
            // position: cgmath::Vector3::<f32>::zero(),
            // rotation: cgmath::Quaternion::<f32>::zero(),
            // scale: cgmath::Vector3::<f32>::zero(),
        };

        scene.mesh_rendering_components.insert(entity_handle, component);
        scene.mesh_rendering_components.get_mut(entity_handle).unwrap()
    }
}








