use std::collections::{HashMap};
use std::env;


use pill_core::{EngineError, get_type_name};

use crate::ecs::*;
use crate::engine::Engine;

use crate::graphics::Renderer;
use crate::resources::resource_map::{ Resource, ResourceMap };
use crate::resources::resource_storage::ResourceStorage;
use anyhow::{Result, Context, Error};

use super::{Material, Mesh, Texture, TextureHandle, TextureType};

// pub struct RendererMaterialHandle {
//     index: u32,
// }

// pub struct RendererMeshHandle {
//     index: u32,
// }

// pub struct RendererPipelineHandle {
//     index: u32,
// }

// pub struct RendererCameraHandle {
//     index: u32,
// }

// pub struct RendererTextureHandle {
//     index: u32,
// }


pill_core::define_new_pill_slotmap_key! { 
    pub struct RendererMaterialHandle;
}

pill_core::define_new_pill_slotmap_key! { 
    pub struct RendererMeshHandle;
}

pill_core::define_new_pill_slotmap_key! { 
    pub struct RendererPipelineHandle;
}

pill_core::define_new_pill_slotmap_key! { 
    pub struct RendererCameraHandle;
}

pill_core::define_new_pill_slotmap_key! { 
    pub struct RendererTextureHandle;
}

pub enum ResourceSource {
    Engine,
    Game,  
}

pub trait ResourceHandle {
    fn get_index(&self) -> u32; 
}


// pub trait Resource {
//     fn get_collection<T>(&self, resource_manager: &mut ResourceManager) -> HashMap<String, Box<T>>;
// }


pub struct ResourceManager {
    resources: ResourceMap,
    //mesh_resources: HashMap<String, Box<MeshResource>>,
    //texture_resources: HashMap<String, Box<TextureResource>>,
    //audio_resources: HashMap<String, Box<AudioResource>>,
    //font_resources: HashMap<String, Box<FontResource>>,
    //shader_resources: HashMap<String, Box<ShaderResource>>,
}

impl ResourceManager {
    pub fn new() -> Self {
	    let resource_manager = Self { 
            resources: ResourceMap::new(),
        };

        resource_manager
    }

    pub fn get_resource<T: Resource<Storage = ResourceStorage::<T>>, H: ResourceHandle>(&self, resource_handle: &H) -> Result<&T> {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage::<T>()?;
        
        // Get resource
        let index = resource_handle.get_index();
        let resource = &resource_storage.data[index as usize]; // [TODO] Check if such conversion is safe and efficient

        Ok(&resource)
    }

    // pub fn add_resource<T: Resource<Storage = ResourceStorage::<T>>, H: ResourceHandle>(&self, resource: T) -> Result<&H> {
    //     // Get resource storage from scene
    //     let resource_storage = self.get_resource_storage::<T>()?;

    //     // [TODO] Check if that resource already exists (same name)
    //     //resource_storage.data.insert(resource.path, resource);
        
    //     // Get index and creat handle

    //     Ok(&resource)
    // }


    fn get_resource_storage<T: Resource<Storage = ResourceStorage::<T>>>(&self) -> Result<&ResourceStorage<T>> {
        self.resources.get::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }

    fn get_resource_storage_mut<T: Resource<Storage = ResourceStorage::<T>>>(&mut self) -> Result<&mut ResourceStorage<T>> {
        self.resources.get_mut::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }


    // -- Default resources
    pub fn create_default_resources(&mut self, renderer: &mut Renderer) {
        self.resources.insert::<Texture>(ResourceStorage::<Texture>::new());
        self.resources.insert::<Mesh>(ResourceStorage::<Mesh>::new());
        self.resources.insert::<Material>(ResourceStorage::<Material>::new());

        // Create default resources
        let texture_storage = self.get_resource_storage_mut::<Texture>().unwrap();

        let path = env::current_dir().unwrap().join("res").join("textures");

        let default_color_texture = Texture::new(renderer, "DefaultColor", path.join("default_color.png"), TextureType::Color).unwrap();
        texture_storage.data.insert("DefaultColor".to_string(), default_color_texture);
        
        let default_normal_texture = Texture::new(renderer, "DefaultNormal", path.join("default_normal.png"), TextureType::Normal).unwrap();
        texture_storage.data.insert("DefaultNormal".to_string(), default_normal_texture);
    }

    pub fn get_default_texture(&self, texture_type: TextureType) -> TextureHandle {
        match texture_type {
            TextureType::Color => TextureHandle { index: 0 },
            TextureType::Normal => TextureHandle { index: 1 },
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
