use std::{any::type_name, collections::HashMap};
use anyhow::{Result, Context, Error};
use boolinator::Boolinator;
use pill_core::{EngineError, get_type_name};
use indexmap::IndexMap;

use crate::ecs::*;

pub struct SceneManager {
    scenes: IndexMap<String, Scene>,
    active_scene: Option<SceneHandle>,
}

impl SceneManager {
    pub fn new() -> Self {
	    Self { 
            scenes: IndexMap::<String, Scene>::new(),
            active_scene: None,
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

    pub fn get_active_scene(&self) -> Result<SceneHandle> {
        // Check if active scene is set
        let active_scene_handle = self.active_scene.ok_or(Error::new(EngineError::NoActiveScene))?;

        // Return active scene handle
        Ok(active_scene_handle.clone())
    }

    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        // Check if scene with that name already exists
        self.scenes.contains_key(name).eq(&false).ok_or(Error::new(EngineError::SceneAlreadyExists))?;

        // Create and add new scene
        let new_scene = Scene::new(name.to_string());
        self.scenes.insert(name.to_string(), new_scene);
       
        // Return handle
        let new_scene_index = self.scenes.get_index_of(name).unwrap();
        Ok(SceneHandle::new(new_scene_index))
    }

    pub fn create_entity(&mut self, scene: SceneHandle) -> Result<EntityHandle> {
        // Get scene
        let target_scene = self.get_scene_mut(scene)?;
        
        // Create new entity
        let new_entity_id = target_scene.entity_counter;
        let new_entity = Entity { 
            name: String::from("Hello"),
            index: new_entity_id,   
        };

        // Insert new entity into scene
        target_scene.entities.insert(target_scene.entity_counter, new_entity);
        target_scene.entity_counter += 1;

        // Return handle
        Ok(EntityHandle::new(new_entity_id))
    }

    pub fn add_component_to_entity<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene: SceneHandle, entity: EntityHandle, component: T) -> Result<()> {     
        // Get scene
        let target_scene = self.get_scene_mut(scene)?;

        // Get component storage from scene
        let component_storage = target_scene.get_component_storage_mut::<T>()?;
        
        // [TODO] Check if that component already exists, probably component mask needs to be checked

        // Add component to storage
        component_storage.data.insert(entity.index, component);
        Ok(())
    }

    pub fn set_active_scene(&mut self, scene: SceneHandle) -> Result<()> {
        // Check if scene for that handle exists
        self.scenes.get_index_mut(scene.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?;

        // Set new active scene
        self.active_scene = Some(scene);
        Ok(())
    }

    pub fn get_scene(&mut self, scene: SceneHandle) -> Result<&Scene> {
        // Get scene
        let scene = self.scenes.get_index(scene.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?.1;
        Ok(scene)
    }

    pub fn get_scene_mut(&mut self, scene: SceneHandle) -> Result<&mut Scene> {
        // Get scene
        let scene = self.scenes.get_index_mut(scene.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?.1;
        Ok(scene)
    }



}