use std::collections::{HashMap};
use std::convert::TryInto;
use std::env;
use std::num::NonZeroU32;
use std::path::PathBuf;


use boolinator::Boolinator;
use pill_core::{EngineError, get_type_name, PillSlotMapKey};

use crate::{ecs::*, engine};
use crate::engine::Engine;

use crate::graphics::Renderer;
//use crate::resources::resource_mapxxx::{ Resource, ResourceMap };
use typemap_rev::*;

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


// pub trait Resource : TypeMapKey {

// }


//pub trait Resource : TypeMapKey<Value = ResourceStorage<H, T>> {
pub trait Resource : TypeMapKey {
    fn initialize(&mut self, engine: &mut Engine);
    fn destroy(&mut self, engine: &mut Engine);
    fn get_name(&self) -> String;
}

pub enum ResourceLoadType {
    Path(PathBuf),
    Bytes(Box::<[u8]>),
}


pub struct ResourceManager {
    resources: TypeMap,
}

impl ResourceManager {
    pub fn new() -> Self {
	    let resource_manager = Self { 
            resources: TypeMap::new(),
        };

        resource_manager
    }

    pub fn get_resource<'a, H, T: Resource<Value = ResourceStorage::<H, T>>>(&'a self, resource_handle: &'a H) -> Result<&'a T> 
        where H: PillSlotMapKey
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage::<H, T>()?;
        
        // Get resource
        let resource = resource_storage.data.get(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource)
    }

    pub fn get_resource_mut<'a, H, T: Resource<Value = ResourceStorage::<H, T>>>(&'a mut self, resource_handle: &'a H) -> Result<&'a mut T> 
        where H: PillSlotMapKey
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<H, T>()?;
        
        // Get resource
        let resource = resource_storage.data.get_mut(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource)
    }

    pub fn add_resource<'a, H: PillSlotMapKey + 'a, T: Resource<Value = ResourceStorage::<H, T>> >(&'a mut self, resource: T) -> Result<H> 
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<H, T>()?;

        // [TODO] Add double hashmap and check if name is already there, if yes the return error, if not then insert new resource and create entry in double hashmap (actually to entries A->B and B->A)
        // Check if resource already exists
        //resource_storage.data.contains_key(name).eq(&false).ok_or(Error::new(EngineError::ResourceAlreadyExists(get_type_name::<T>(), name.to_string())))?;
        
            
        // Insert new
        let resource_handle: H = resource_storage.data.insert(resource);

        Ok(resource_handle)
    }

    fn get_resource_storage<H: PillSlotMapKey, T: Resource<Value = ResourceStorage::<H, T>>>(&self) -> Result<&ResourceStorage<H, T>> {
        self.resources.get::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }

    fn get_resource_storage_mut<H: PillSlotMapKey, T: Resource<Value = ResourceStorage::<H, T>>>(&mut self) -> Result<&mut ResourceStorage<H, T>> {
        self.resources.get_mut::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }

    pub fn register_resource_type<H: PillSlotMapKey, T: Resource<Value = ResourceStorage::<H, T>>>(&mut self) -> Result<()> {
        self.resources.insert::<T>(ResourceStorage::<H, T>::new());
        Ok(())
    }

    pub fn get_default_texture_handle(&self, texture_type: TextureType) -> TextureHandle {
        match texture_type {
            TextureType::Color => TextureHandle::new(1,NonZeroU32::new(1).unwrap()),
            TextureType::Normal => TextureHandle::new(2,NonZeroU32::new(1).unwrap()),
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
