use crate::config::*;
use crate::graphics::{RendererMaterialHandle, RendererTextureHandle};
use super::{ResourceStorage, Resource, MaterialHandle, Material, TextureType, TextureHandle, Texture};

use pill_core::{EngineError, get_type_name, PillSlotMapKey};

use std::collections::{HashMap};
use std::convert::TryInto;
use std::env;
use std::num::NonZeroU32;
use std::path::PathBuf;
use boolinator::Boolinator;
use typemap_rev::*;
use anyhow::{Result, Context, Error};

pub struct ResourceManager {
    resources: TypeMap,
}

impl ResourceManager {
    pub fn new() -> Self {
	    Self { 
            resources: TypeMap::new(),
        }
    }

     // --- Slots

    pub(crate) fn get_resource_slot<'a, T>(&'a self, resource_handle: &T::Handle) -> Result<&'a Option<T>> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        // Get resource storage
        let resource_storage = self.get_resource_storage::<T>()?;
        // Get resource slot
        let resource_slot = resource_storage.data.get(resource_handle.clone())
            .ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource_slot)
    }

    pub(crate) fn get_resource_slot_mut<'a, T>(&'a mut self, resource_handle: &T::Handle) -> Result<&'a mut Option<T>> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        // Get resource storage
        let resource_storage = self.get_resource_storage_mut::<T>()?;
        // Get resource slot
        let resource_slot = resource_storage.data.get_mut(resource_handle.clone())
            .ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;

        Ok(resource_slot)
    }

    // --- Storages

    pub(crate) fn get_resource_storage<T>(&self) -> Result<&ResourceStorage<T>> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        self.resources.get::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }

    pub(crate) fn get_resource_storage_mut<T>(&mut self) -> Result<&mut ResourceStorage<T>> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        self.resources.get_mut::<T>().ok_or(Error::new(EngineError::ResourceNotRegistered(get_type_name::<T>())))
    }

    // --- Register - Add - Remove

    pub fn register_resource_type<T>(&mut self) -> Result<()> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        self.resources.insert::<T>(ResourceStorage::<T>::new());

        Ok(())
    }

    pub fn add_resource<'a, T>(&'a mut self, resource: T) -> Result<T::Handle> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        // Get resource storage
        let resource_storage = self.get_resource_storage_mut::<T>()?;
        let resource_name = resource.get_name().to_owned();
        // Check if resource already exists
        resource_storage.mapping.contains_key(&resource_name).eq(&false)
            .ok_or(Error::new(EngineError::ResourceAlreadyExists(get_type_name::<T>(), resource_name.clone())))?;
        // Insert new resource
        let resource_handle = resource_storage.data.insert(Some(resource));
        // Insert new mapping
        resource_storage.mapping.insert(&resource_name, &resource_handle);

        Ok(resource_handle)
    }

    pub fn remove_resource<T>(&mut self, resource_handle: &T::Handle) -> Result<(T::Handle, T)> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        // Get resource storage
        let resource_storage = self.get_resource_storage_mut::<T>()?;
        // Check if exists
        resource_storage.mapping.contains_value(resource_handle).eq(&true).ok_or(Error::new(EngineError::InvalidResourceHandle(get_type_name::<T>())))?;
        // Remove resource
        let resource = resource_storage.data.remove(*resource_handle).unwrap().expect("Critical: Resource is None");
        // Remove mapping
        resource_storage.mapping.remove_by_value(resource_handle);

        Ok((resource_handle.clone(), resource))
    } 

    pub fn remove_resource_by_name<T>(&mut self, name: &str) -> Result<(T::Handle, T)> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        // Get resource storage
        let resource_storage = self.get_resource_storage_mut::<T>()?;
        // Get handle by name
        let resource_handle = resource_storage.mapping.get_value(&name.to_string()).ok_or(EngineError::InvalidResourceName(name.to_string(), get_type_name::<T>()))?.clone();
        // Remove resource
        let resource = resource_storage.data.remove(resource_handle).unwrap().expect("Critical: Resource is None");
        // Remove mapping
        resource_storage.mapping.remove_by_key(&name.to_string());

        Ok((resource_handle, resource))
    } 

    // --- Get

    pub fn get_resource_handle<T>(&self, name: &str) -> Result<T::Handle> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        // Get resource storage
        let resource_storage = self.get_resource_storage::<T>()?;
        // Get resource handle
        let resource_handle = resource_storage.mapping.get_value(&name.to_string()).ok_or(EngineError::InvalidSceneName(name.to_string()))?.clone();
        
        Ok(resource_handle)
    }

    pub fn get_resource<'a, T>(&'a self, resource_handle: &'a T::Handle) -> Result<&'a T> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        // Get resource
        let resource = self.get_resource_slot::<T>(resource_handle)?.as_ref().expect("Critical: Resource is None");

        Ok(resource)
    }

    pub fn get_resource_by_name<'a, T>(&'a self, name: &str) -> Result<&'a T> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        // Get resource storage
        let resource_storage = self.get_resource_storage::<T>()?;
        // Get handle by name
        let resource_handle = resource_storage.mapping.get_value(&name.to_string())
            .ok_or(EngineError::InvalidResourceName(name.to_string(), get_type_name::<T>()))?;
        // Get resource
        let resource = self.get_resource_slot::<T>(resource_handle)?.as_ref().expect("Critical: Resource is None");

        Ok(resource)
    }

    pub fn get_resource_mut<'a, T>(&'a mut self, resource_handle: &'a T::Handle) -> Result<&'a mut T> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        // Get resource
        let resource = self.get_resource_slot_mut::<T>(resource_handle)?.as_mut().expect("Critical: Resource is None");

        Ok(resource)
    }

    pub fn get_resource_by_name_mut<'a, T>(&'a mut self, name: &str) -> Result<&'a mut T> 
        where T: Resource<Value = ResourceStorage::<T>>
    {
        // Get resource storage
        let resource_storage = self.get_resource_storage_mut::<T>()?;
        // Get handle by name
        let resource_handle = resource_storage.mapping.get_value(&name.to_string())
            .ok_or(EngineError::InvalidResourceName(name.to_string(), get_type_name::<T>()))?.clone();
        // Get resource
        let resource = self.get_resource_slot_mut::<T>(&resource_handle)?.as_mut().expect("Critical: Resource is None");

        Ok(resource)
    }

    // --- Get default
        
    pub(crate) fn get_default_material(&self) -> Result<(MaterialHandle, RendererMaterialHandle)> {
        // Get resource storage
        let resource_storage = self.get_resource_storage::<Material>()?;
        // Get handle by name
        let resource_handle = resource_storage.mapping.get_value(&DEFAULT_MATERIAL_NAME.to_string()).expect("Critical: Default Resource not existing").clone();
        // Get slot
        let resource_slot = self.get_resource_slot::<Material>(&resource_handle)?.as_ref();
        // Get renderer resource handle
        let renderer_resource_handle = resource_slot.expect("Critical: Resource is None").renderer_resource_handle.expect("Critical: RendererResource is None");

        Ok((resource_handle, renderer_resource_handle))
    }

    pub(crate) fn get_default_texture(&self, texture_type: TextureType) -> Result<(TextureHandle, RendererTextureHandle)> {
        // Get resource storage
        let resource_storage = self.get_resource_storage::<Texture>()?;

        // Get handle by name
        let texture_name = match texture_type {
            TextureType::Color => DEFAULT_COLOR_TEXTURE_NAME,
            TextureType::Normal => DEFAULT_NORMAL_TEXTURE_NAME,
        };
        let resource_handle = resource_storage.mapping.get_value(&texture_name.to_string()).unwrap().clone();
        // Get slot
        let resource_slot = self.get_resource_slot::<Texture>(&resource_handle)?.as_ref();
        // Get renderer resource handle
        let renderer_resource_handle = resource_slot.expect("Critical: Resource is None").renderer_resource_handle.expect("Critical: RendererResource is None");

        Ok((resource_handle, renderer_resource_handle))
    }
}