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

pill_core::define_new_pill_slotmap_key! { 
    pub struct MaterialHandle;
}

pill_core::define_new_pill_slotmap_key! { 
    pub struct MeshHandle;
}

pill_core::define_new_pill_slotmap_key! { 
    pub struct TextureHandle;
}

pub trait Resource : TypeMapKey {
    fn initialize(&mut self, engine: &mut Engine);
    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H);
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
	    Self { 
            resources: TypeMap::new(),
        }
    }

    pub fn register_resource_type<H, T>(&mut self) -> Result<()> 
        where H: PillSlotMapKey, T: Resource<Value = Option<ResourceStorage::<H, T>>>
    {
        self.resources.insert::<T>(Some(ResourceStorage::<H, T>::new()));
        Ok(())
    }

    pub fn add_resource<'a, H, T>(&'a mut self, resource: T) -> Result<H> 
        where H: PillSlotMapKey + 'a, T: Resource<Value = Option<ResourceStorage::<H, T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<H, T>()?.as_mut().unwrap();

        let resource_name = resource.get_name().to_owned();
        // [TODO] Check if name is valid (not empty string)
        // Check if resource already exists
        resource_storage.mapping.contains_key(&resource_name).eq(&false).ok_or(Error::new(EngineError::ResourceAlreadyExists(get_type_name::<T>(), resource_name.clone())))?;
        
        // Insert new resource
        let resource_handle = resource_storage.data.insert(resource);

        // Insert new mapping
        resource_storage.mapping.insert(&resource_name, &resource_handle);

        Ok(resource_handle)
    }
    
    pub fn get_resource<'a, H, T>(&'a self, resource_handle: &'a H) -> Result<&'a T> 
        where H: PillSlotMapKey, T: Resource<Value = Option<ResourceStorage::<H, T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage::<H, T>()?.as_ref().unwrap();
        
        // Get resource
        let resource = resource_storage.data.get(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource)
    }

    pub fn get_resource_by_name<'a, H, T>(&'a self, name: &str) -> Result<&'a T> 
        where H: PillSlotMapKey + 'a, T: Resource<Value = Option<ResourceStorage::<H, T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage::<H, T>()?.as_ref().unwrap();
        
        // Get handle by name
        let resource_handle = resource_storage.mapping.get_value(&name.to_string()).ok_or(EngineError::InvalidResourceName(name.to_string(), get_type_name::<T>()))?;
   
        // Get resource
        let resource = resource_storage.data.get(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource)
    }

    pub fn get_resource_mut<'a, H, T>(&'a mut self, resource_handle: &'a H) -> Result<&'a mut T> 
        where H: PillSlotMapKey, T: Resource<Value = Option<ResourceStorage::<H, T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<H, T>()?.as_mut().unwrap();
        
        // Get resource
        let resource = resource_storage.data.get_mut(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource)
    }

    pub fn get_resource_by_name_mut<'a, H, T>(&'a mut self, name: &str) -> Result<&'a mut T> 
        where H: PillSlotMapKey + 'a, T: Resource<Value = Option<ResourceStorage::<H, T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<H, T>()?.as_mut().unwrap();
        
        // Get handle by name
        let resource_handle = resource_storage.mapping.get_value(&name.to_string()).ok_or(EngineError::InvalidResourceName(name.to_string(), get_type_name::<T>()))?;

        // Get resource
        let resource = resource_storage.data.get_mut(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource)
    }

    pub fn remove_resource<H, T>(&mut self, resource_handle: &H) -> Result<(H, T)> 
        where H: PillSlotMapKey, T: Resource<Value = Option<ResourceStorage::<H, T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<H, T>()?.as_mut().unwrap();

        // Check if exists
        resource_storage.mapping.contains_value(resource_handle).eq(&false).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        // Remove resource
        let resource = resource_storage.data.remove(*resource_handle).unwrap();
       
        // Remove mapping
        resource_storage.mapping.remove_by_value(resource_handle);

        Ok((resource_handle.clone(), resource))
    } 

    pub fn remove_resource_by_name<H, T>(&mut self, name: &str) -> Result<(H, T)> 
        where H: PillSlotMapKey, T: Resource<Value = Option<ResourceStorage::<H, T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<H, T>()?.as_mut().unwrap();

        // Get handle by name
        let resource_handle = resource_storage.mapping.get_value(&name.to_string()).ok_or(EngineError::InvalidResourceName(name.to_string(), get_type_name::<T>()))?.clone();

        // Remove resource
        let resource = resource_storage.data.remove(resource_handle).unwrap();
       
        // Remove mapping
        resource_storage.mapping.remove_by_key(&name.to_string());

        Ok((resource_handle, resource))
    } 
        
    pub(crate) fn get_resource_storage<H, T>(&self) -> Result<&Option<ResourceStorage<H, T>>> 
        where H: PillSlotMapKey, T: Resource<Value = Option<ResourceStorage::<H, T>>>
    {
        self.resources.get::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }

    pub(crate) fn get_resource_storage_mut<H, T>(&mut self) -> Result<&mut Option<ResourceStorage<H, T>>> 
        where H: PillSlotMapKey, T: Resource<Value = Option<ResourceStorage::<H, T>>>
    {
        self.resources.get_mut::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }

    
    pub(crate) fn get_default_material(&self) -> Result<(MaterialHandle, RendererMaterialHandle)> {
        let resource_storage = self.get_resource_storage::<MaterialHandle, Material>()?.as_ref().unwrap();

        let material_handle = resource_storage.mapping.get_value(&"DefaultMaterial".to_string()).unwrap().clone();
        let material = resource_storage.data.get(material_handle).unwrap();
        let renderer_resource_handle = material.renderer_resource_handle.unwrap();

        return Ok((material_handle, renderer_resource_handle));
    }

    //pub(crate) fn get_default_texture_handle(&self, texture_name: &str) -> Result<TextureHandle> {
    pub(crate) fn get_default_texture(&self, texture_name: &str) -> Result<(TextureHandle, RendererTextureHandle)> {
        let resource_storage = self.get_resource_storage::<TextureHandle, Texture>()?.as_ref().unwrap();

        let texture_name = match texture_name {
            "Color" => "DefaultColor",
            "Normal" => "DefaultNormal",
            _ => panic!()
        };

        let texture_handle = resource_storage.mapping.get_value(&texture_name.to_string()).unwrap().clone();
        let texture = resource_storage.data.get(texture_handle).unwrap();
        let renderer_resource_handle = texture.renderer_resource_handle.unwrap();

        return Ok((texture_handle, renderer_resource_handle));
        
        // match texture_name {
        //     "Color" => {
        //         let texture_storage = self.get_resource_storage::<TextureHandle, Texture>().unwrap();
        //         let texture_handle = texture_storage.mapping.get_value(&"DefaultColor".to_string()).unwrap().clone();
        //         let texture = texture_storage.data.get(texture_handle).unwrap();
        //         let renderer_resource_handle = texture.renderer_resource_handle.unwrap();

        //         return Ok((texture_handle, renderer_resource_handle));
        //     },
        //         Ok(TextureHandle::new(1,NonZeroU32::new(1).unwrap())),
        //     "Normal" => Ok(TextureHandle::new(2,NonZeroU32::new(1).unwrap())),
        //     _ => panic!()
        // }

        // match texture_name {
        //     "Color" => Ok(TextureHandle::new(1,NonZeroU32::new(1).unwrap())),
        //     "Normal" => Ok(TextureHandle::new(2,NonZeroU32::new(1).unwrap())),
        //     _ => panic!()
        // }
    }
}