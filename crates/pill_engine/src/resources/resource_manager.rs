use std::collections::{HashMap};
use std::convert::TryInto;
use std::env;
use std::num::NonZeroU32;
use std::path::PathBuf;


use boolinator::Boolinator;
use pill_core::{EngineError, get_type_name, PillSlotMapKey};

use crate::config::*;
use crate::{ecs::*, engine};
use crate::engine::Engine;

use crate::graphics::Renderer;
//use crate::resources::resource_mapxxx::{ Resource, ResourceMap };
use typemap_rev::*;

use crate::resources::resource_storage::ResourceStorage;
use anyhow::{Result, Context, Error};

use super::{Material, Mesh, Texture, TextureType, MaterialHandle, TextureHandle, Resource};
use crate::graphics::{ RendererMaterialHandle, RendererTextureHandle };

pub struct ResourceManager {
    resources: TypeMap,
}

impl ResourceManager {
    pub fn new() -> Self {
	    Self { 
            resources: TypeMap::new(),
        }
    }

    pub fn register_resource_type<T>(&mut self) -> Result<()> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        self.resources.insert::<T>(Some(ResourceStorage::<T>::new()));
        Ok(())
    }

    pub fn add_resource<'a, T>(&'a mut self, resource: T) -> Result<T::Handle> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<T>()?.as_mut().unwrap();

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
    
    pub fn get_resource_handle<T>(&self, name: &str) -> Result<T::Handle> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage::<T>()?.as_ref().unwrap();
        
        // Get resource handle
        let resource_handle = resource_storage.mapping.get_value(&name.to_string()).ok_or(EngineError::InvalidSceneName(name.to_string()))?.clone();
        
        Ok(resource_handle)
    }

    pub fn get_resource<'a, T>(&'a self, resource_handle: &'a T::Handle) -> Result<&'a T> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage::<T>()?.as_ref().unwrap();
        
        // Get resource
        let resource = resource_storage.data.get(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource)
    }

    pub fn get_resource_by_name<'a, T>(&'a self, name: &str) -> Result<&'a T> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage::<T>()?.as_ref().unwrap();
        
        // Get handle by name
        let resource_handle = resource_storage.mapping.get_value(&name.to_string()).ok_or(EngineError::InvalidResourceName(name.to_string(), get_type_name::<T>()))?;
   
        // Get resource
        let resource = resource_storage.data.get(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource)
    }

    pub fn get_resource_mut<'a, T>(&'a mut self, resource_handle: &'a T::Handle) -> Result<&'a mut T> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<T>()?.as_mut().unwrap();
        
        // Get resource
        let resource = resource_storage.data.get_mut(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource)
    }

    pub fn get_resource_by_name_mut<'a, T>(&'a mut self, name: &str) -> Result<&'a mut T> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<T>()?.as_mut().unwrap();
        
        // Get handle by name
        let resource_handle = resource_storage.mapping.get_value(&name.to_string()).ok_or(EngineError::InvalidResourceName(name.to_string(), get_type_name::<T>()))?;

        // Get resource
        let resource = resource_storage.data.get_mut(*resource_handle).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource)
    }

    pub fn remove_resource<T>(&mut self, resource_handle: &T::Handle) -> Result<(T::Handle, T)> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<T>()?.as_mut().unwrap();

        // Check if exists
        resource_storage.mapping.contains_value(resource_handle).eq(&false).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        // Remove resource
        let resource = resource_storage.data.remove(*resource_handle).unwrap();
       
        // Remove mapping
        resource_storage.mapping.remove_by_value(resource_handle);

        Ok((resource_handle.clone(), resource))
    } 

    pub fn remove_resource_by_name<T>(&mut self, name: &str) -> Result<(T::Handle, T)> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        // Get resource storage from scene
        let resource_storage = self.get_resource_storage_mut::<T>()?.as_mut().unwrap();

        // Get handle by name
        let resource_handle = resource_storage.mapping.get_value(&name.to_string()).ok_or(EngineError::InvalidResourceName(name.to_string(), get_type_name::<T>()))?.clone();

        // Remove resource
        let resource = resource_storage.data.remove(resource_handle).unwrap();
       
        // Remove mapping
        resource_storage.mapping.remove_by_key(&name.to_string());

        Ok((resource_handle, resource))
    } 
        
    pub(crate) fn get_resource_storage<T>(&self) -> Result<&Option<ResourceStorage<T>>> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        self.resources.get::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }

    pub(crate) fn get_resource_storage_mut<T>(&mut self) -> Result<&mut Option<ResourceStorage<T>>> 
        where T: Resource<Value = Option<ResourceStorage::<T>>>
    {
        self.resources.get_mut::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }

    
    pub(crate) fn get_default_material(&self) -> Result<(MaterialHandle, RendererMaterialHandle)> {
        let resource_storage = self.get_resource_storage::<Material>()?.as_ref().unwrap();

        let material_handle = resource_storage.mapping.get_value(&DEFAULT_MATERIAL_NAME.to_string()).unwrap().clone();
        let material = resource_storage.data.get(material_handle).unwrap();
        let renderer_resource_handle = material.renderer_resource_handle.unwrap();

        Ok((material_handle, renderer_resource_handle))
    }

    pub(crate) fn get_default_texture(&self, texture_type: TextureType) -> Result<(TextureHandle, RendererTextureHandle)> {
        let resource_storage = self.get_resource_storage::<Texture>()?.as_ref().unwrap();

        let texture_name = match texture_type {
            TextureType::Color => DEFAULT_COLOR_TEXTURE_NAME,
            TextureType::Normal => DEFAULT_NORMAL_TEXTURE_NAME,
        };

        let texture_handle = resource_storage.mapping.get_value(&texture_name.to_string()).unwrap().clone();
        let texture = resource_storage.data.get(texture_handle).unwrap();
        let renderer_resource_handle = texture.renderer_resource_handle.unwrap();

        Ok((texture_handle, renderer_resource_handle))
    }
}