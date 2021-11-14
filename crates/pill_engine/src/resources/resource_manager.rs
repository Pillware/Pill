use std::collections::{HashMap};

use crate::ecs::*;
use crate::engine::Engine;

pub enum ResourceSource {
    Engine,
    Game,  
}

pub trait Resource {
    fn get_collection<T>(&self, resource_manager: &mut ResourceManager) -> HashMap<String, Box<T>>;
}


pub struct ResourceManager {
    resources: ComponentMap,

    //mesh_resources: HashMap<String, Box<MeshResource>>,
    //texture_resources: HashMap<String, Box<TextureResource>>,
    //audio_resources: HashMap<String, Box<AudioResource>>,
    //font_resources: HashMap<String, Box<FontResource>>,
    //shader_resources: HashMap<String, Box<ShaderResource>>,
}

impl ResourceManager {
    pub fn new() -> Self {
	    Self { 
            resources: ComponentMap::new(),
        }
    }

    // pub fn load_resource<T: Resource>(&mut self, t: T, path: String, source: ResourceSource) { // Trait bound technique
    //     let collection: HashMap<String, Box<T>> = t.get_collection(self);
    //     match collection.get(&path) {
    //         Some(resource) => {
    //             println!("[Resource Manager] Mesh resource already exists, increasing pointer reference count");
    //             //mesh_resource 
    //         },
    //         None => {
    //             println!("[Resource Manager] Mesh resource not found, creating new entry");

    //             // Create new mesh resource
    //             let new_mesh_resource: MeshResource = MeshResource {};
    //             self.mesh_resources.insert(path, Box::new(new_mesh_resource));
    //             //new_mesh_resource
    //         }
    //     };



        
    // }




    // pub fn create_resource_gameobject(&self, scene: Box<Scene>, name: String, file_path: Box<Path>) -> Box<GameObject> {
    //     println!("[Resource Manager] Creating GameObject resource from path: {}", file_path);
    //     let new_gameobject = Box::new(GameObject::new(self.engine, name, file_path));
    //     self.gameobjectCollection.append(new_gameobject);
    //     return new_gameobject;
    // }
}
