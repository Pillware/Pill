use std::collections::{HashMap};
use std::env;
use std::num::NonZeroU32;


use boolinator::Boolinator;
use pill_core::{EngineError, get_type_name, PillSlotMapKey};

use crate::ecs::*;
use crate::engine::Engine;

use crate::graphics::Renderer;
use crate::resources::resource_map::{ Resource, ResourceMap };
use crate::resources::resource_storage::ResourceStorage;
use anyhow::{Result, Context, Error};

use super::{Material, Mesh, Texture, TextureType};

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

pill_core::define_new_pill_slotmap_key! { 
    pub struct MaterialHandle;
}

pill_core::define_new_pill_slotmap_key! { 
    pub struct MeshHandle;
}

pill_core::define_new_pill_slotmap_key! { 
    pub struct TextureHandle;
}

// pub trait ResourceHandle {
//     fn get_index(&self) -> u32; 
// }


// pub trait Resource {
//     fn get_collection<T>(&self, resource_manager: &mut ResourceManager) -> HashMap<String, Box<T>>;
// }


pub struct ResourceManager {
    resources: ResourceMap,
}

impl ResourceManager {
    pub fn new() -> Self {
	    let resource_manager = Self { 
            resources: ResourceMap::new(),
        };

        resource_manager
    }

    pub fn get_resource<'a, H, T: Resource<Storage = ResourceStorage::<H, T>>>(&self, resource_handle: &H) -> Result<&T> 
        where H: PillSlotMapKey + 'static
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage::<H, T>()?;
        
        // Get resource
        let resource = resource_storage.data.get(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(&resource)
    }


//     pub fn get_resource<H, T: Resource<Storage = ResourceStorage::<H, T>>>(&self, resource_handle: H) -> Result<&T> 
//     where H: PillSlotMapKey
// {
//     // Get resource storage from scene
//     let resource_storage = self.get_resource_storage::<H, T>()?;
    
//     // Get resource
//     let resource = resource_storage.data.get(resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

//     Ok(&resource)
// }

    // pub fn add_resource<H: PillSlotMapKey, T: Resource<Storage = ResourceStorage::<H, T>>>(&self, name: &str, resource: T) -> Result<&H> {
    //     // Get resource storage from scene
    //     let resource_storage = self.get_resource_storage::<T>()?;

    //     // [TODO] Add double hashmap and check if name is already there, if yes the return error, if not then insert new resource and create entry in double hashmap (actually to entries A->B and B->A)
    //     // Check if resource already exists
    //     // resource_storage.data.contains_key(name).eq(&false).ok_or(Error::new(EngineError::ResourceAlreadyExists(get_type_name::<T>(), name.to_string())))?;

    //     // Get index and create handle
    //     resource_storage.data.insert(name, resource);

    //     // Create resource handle


    //     Ok(&resource)
    // }


    fn get_resource_storage<H: PillSlotMapKey, T: Resource<Storage = ResourceStorage::<H, T>>>(&self) -> Result<&ResourceStorage<H, T>> {
        self.resources.get::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }

    fn get_resource_storage_mut<H: PillSlotMapKey, T: Resource<Storage = ResourceStorage::<H, T>>>(&mut self) -> Result<&mut ResourceStorage<H, T>> {
        self.resources.get_mut::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }


    // -- Default resources
    pub fn create_default_resources(&mut self, renderer: &mut Renderer) {
        // self.resources.insert::<Texture>(ResourceStorage::<Texture>::new());
        // self.resources.insert::<Mesh>(ResourceStorage::<Mesh>::new());
        // self.resources.insert::<Material>(ResourceStorage::<Material>::new());

    //     // Create default resources
    //     let texture_storage = self.get_resource_storage_mut::<Texture>().unwrap();


    //     // Load default resource data to executable
    //     let default_color_texture_bytes = include_bytes!("../../res/textures/default_color.png");



    //    // let path = env::current_dir().unwrap().join("res").join("textures");
    //    let path = env::current_dir().unwrap().join("res").join("textures");
    //     println!("xxxxxxxxx {}", path.display());

    //     let default_color_texture = Texture::new(renderer, "DefaultColor", path.join("default_color.png"), TextureType::Color).unwrap();
    //     texture_storage.data.insert("DefaultColor".to_string(), default_color_texture);
        
    //     let default_normal_texture = Texture::new(renderer, "DefaultNormal", path.join("default_normal.png"), TextureType::Normal).unwrap();
    //     texture_storage.data.insert("DefaultNormal".to_string(), default_normal_texture);
    }

    pub fn get_default_texture(&self, texture_type: TextureType) -> TextureHandle {
        match texture_type {
            TextureType::Color => TextureHandle::new(0,NonZeroU32::new(1).unwrap()),
            TextureType::Normal => TextureHandle::new(1,NonZeroU32::new(1).unwrap()),
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
}
