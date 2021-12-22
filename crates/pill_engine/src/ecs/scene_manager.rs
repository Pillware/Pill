use std::{any::type_name, collections::HashMap};
use anyhow::{Result, Context, Error};
use boolinator::Boolinator;
use pill_core::{EngineError, get_type_name};
use indexmap::IndexMap;

use crate::ecs::*;

pill_core::define_new_pill_slotmap_key! { 
    pub struct SceneHandle;
}

pub struct SceneManager {
    pub(crate) scenes: pill_core::PillSlotMap<SceneHandle, Scene>, 
    pub(crate) mapping: pill_core::PillTwinMap<String, SceneHandle>, // Mapping from scene name to scene handle and vice versa
    active_scene_handle: Option<SceneHandle>,
}

impl SceneManager {
    pub fn new() -> Self {
	    Self { 
            scenes: pill_core::PillSlotMap::<SceneHandle, Scene>::with_key(),
            mapping: pill_core::PillTwinMap::<String, SceneHandle>::new(),
            active_scene_handle: None,
        }
    }

    pub fn register_component<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene: SceneHandle) -> Result<()> {
        // Get scene
        let target_scene = self.get_scene_mut(scene)?;

        // Check if component storage is already registered
        target_scene.components.contains_key::<T>().eq(&false).ok_or(Error::new(EngineError::ComponentAlreadyRegistered(get_type_name::<T>(), target_scene.name.clone())))?;

        // Create new component storage
        let component_storage = ComponentStorage::<T>::new();

        // Add component storage to scene
        target_scene.components.insert::<T>(component_storage);
        Ok(())
    }

    pub fn create_entity(&mut self, scene_handle: SceneHandle) -> Result<EntityHandle> {
        // Get scene
        let target_scene = self.get_scene_mut(scene_handle)?;
        
        // Create new entity
        let new_entity_id = target_scene.entity_counter;
        let new_entity = Entity { 
            name: String::from("Hello"), // [TODO] Is this even needed?
            index: new_entity_id,   
        };

        // Insert new entity into scene
        target_scene.entities.insert(target_scene.entity_counter, new_entity);
        target_scene.entity_counter += 1;

        // Return handle
        Ok(EntityHandle::new(new_entity_id))
    }

    pub fn add_component_to_entity<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene_handle: SceneHandle, entity: EntityHandle, component: T) -> Result<()> {     
        // Get scene
        let target_scene = self.get_scene_mut(scene_handle)?;

        // Get component storage from scene
        let component_storage = target_scene.get_component_storage_mut::<T>()?;
        
        // [TODO] Check if that component already exists, probably component mask needs to be checked

        // Add component to storage
        // [TODO!!!] This is (WAS) very wrong!, We are inserting to vector what does not mean "access element and save data in it"
        // It means "insert completely new element and shift all other on the right to the right by 1!"
        // What is more if there are no elements before this index it will panic! So we need to allocate empty elements to make it work
        //component_storage.data.insert(entity.index, component); 

        *(component_storage.data.get_mut(entity.index).unwrap()) = Some(component); 
        Ok(())
    }

    // - Scene -

    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        // Check if scene with that name already exists
        self.mapping.contains_key(&name.to_string()).eq(&false).ok_or(Error::new(EngineError::SceneAlreadyExists(name.to_string())))?;

        // Create new scene
        let new_scene = Scene::new(name.to_string());

        // Insert new scene
        let scene_handle = self.scenes.insert(new_scene);
       
        // Insert new mapping
        self.mapping.insert(&name.to_string(), &scene_handle);

        Ok(scene_handle)
    }

    pub fn get_scene_handle(&self, name: &str) -> Result<SceneHandle> {
        let scene_handle = self.mapping.get_value(&name.to_string()).ok_or(EngineError::InvalidSceneName(name.to_string()))?.clone();
        Ok(scene_handle)
    }

    pub fn get_scene(&self, scene_handle: SceneHandle) -> Result<&Scene> {
        let scene = self.scenes.get(scene_handle).ok_or(Error::new(EngineError::InvalidSceneHandle))?;
        Ok(scene)
    }

    pub fn get_scene_mut(&mut self, scene_handle: SceneHandle) -> Result<&mut Scene> {
        let scene = self.scenes.get_mut(scene_handle).ok_or(Error::new(EngineError::InvalidSceneHandle))?;
        Ok(scene)
    }

    // - Active scene -
    
    pub fn set_active_scene(&mut self, scene_handle: SceneHandle) -> Result<()> {
        // Check if scene for that handle exists
        self.scenes.get_mut(scene_handle).ok_or(Error::new(EngineError::InvalidSceneHandle))?;

        // Set new active scene
        self.active_scene_handle = Some(scene_handle);
        Ok(())
    }

    pub fn get_active_scene_handle(&self) -> Result<SceneHandle> {
        match self.active_scene_handle {
            Some(v) =>  Ok(v.clone()),
            None => Err(Error::new(EngineError::NoActiveScene)),
        }
    }

    pub fn get_active_scene(&self) -> Result<&Scene> {
        // Check if active scene is set
        let active_scene_handle = self.active_scene_handle.ok_or(Error::new(EngineError::NoActiveScene))?;
        let active_scene = self.get_scene(active_scene_handle)?;

        // Return active scene handle
        Ok(active_scene)
    }

    pub fn get_active_scene_mut(&mut self) -> Result<&mut Scene> {
        // Check if active scene is set
        let active_scene_handle = self.active_scene_handle.ok_or(Error::new(EngineError::NoActiveScene))?;
        let active_scene = self.get_scene_mut(active_scene_handle)?;

        // Return active scene handle
        Ok(active_scene)
    }
}