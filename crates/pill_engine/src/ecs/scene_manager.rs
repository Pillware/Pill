use std::{any::type_name, collections::HashMap};

use super::{Component, ComponentStorage, Entity, EntityHandle, Scene, SceneHandle};
use anyhow::{Result, Context, Error};
use boolinator::Boolinator;
use pill_core::EngineError;
use indexmap::IndexMap;

pub struct SceneManager {
    scenes:  IndexMap<String, Scene>,
    current_scene: Option<SceneHandle>,
}

impl SceneManager {
    pub fn new() -> Self {
	    Self { 
            scenes: IndexMap::<String, Scene>::new(),
            current_scene: None,
        }
    }

    pub fn register_component<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene: SceneHandle) -> Result<()> {
        // Get scene
        let target_scene = self.get_scene_mut(scene)?;

        // Check if component storage is already registered
        let component_type = type_name::<T>()[11..].to_string();
        target_scene.components.contains_key::<T>().eq(&true).ok_or(Error::new(EngineError::ComponentAlreadyRegistered(component_type, target_scene.name.clone())))?;

        // Create new component storage
        let component_storage = ComponentStorage::<T>::new();

        // Add component storage to scene
        target_scene.components.insert::<T>(component_storage);
        Ok(())
    }

    pub fn get_current_scene(&self) -> Result<SceneHandle> {
        // Check if current scene is set
        let current_scene_handle = self.current_scene.ok_or(Error::new(EngineError::CurrentSceneNotSet))?;

        // Return current scene handle
        Ok(current_scene_handle.clone())
    }

    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        // Check if scene with that name already exists
        self.scenes.contains_key(name).eq(&true).ok_or(Error::new(EngineError::SceneWithThisNameAlreadyExists))?;

        // Create and add new scene
        let new_scene = Scene::new(name.to_string());
        self.scenes.insert(name.to_string(), new_scene);
       
        // Return handle
        let new_scene_index = self.scenes.get_index_of(name).unwrap();
        Ok(SceneHandle::new(new_scene_index))
    }

    pub fn create_entity(&mut self, scene: SceneHandle) -> Result<EntityHandle> {
        // Get scene
        let target_scene = self.get_scene_mut(scene)?; // [TODO] Check if this will automatically return error and not Err(..) is needed. What if it returns Ok, function progresses? 
        
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
        let component_storage = target_scene.get_component_storage_mut::<T>();
        
        // Add component to storage
        component_storage.data.insert(entity.index, component);
        Ok(())
    }

    pub fn set_current_scene(&mut self, scene: SceneHandle) -> Result<()> {
        // Check if scene for that handle exists
        self.scenes.get_index_mut(scene.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?;

        // Set new current scene
        self.current_scene = Some(scene);
        Ok(())
    }

    pub fn get_scene_mut(&mut self, scene: SceneHandle) -> Result<&mut Scene> {
        // Get scene
        let scene = self.scenes.get_index_mut(scene.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?.1;
        Ok(scene)
    }

}