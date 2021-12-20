use std::{any::type_name, collections::HashMap, cell::RefCell};
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

    pub fn get_active_scene_handle(&self) -> Result<SceneHandle> {
        // Check if active scene is set
        let active_scene_handle = self.active_scene.ok_or(Error::new(EngineError::NoActiveScene))?;

        // Return active scene handle
        Ok(active_scene_handle.clone())
    }

    pub fn get_active_scene(&self) -> Result<&Scene> {
        // Check if active scene is set
        let active_scene_handle = self.active_scene.ok_or(Error::new(EngineError::NoActiveScene))?;
        let active_scene = self.get_scene(active_scene_handle)?;

        // Return active scene handle
        Ok(active_scene)
    }

    pub fn get_allocator_mut(&mut self, scene: SceneHandle) -> Result<&mut Allocator> {
        // Get scene
        let target_scene = self.get_scene_mut(scene)?;

        // Get allocator from scene
        let index_allocator = target_scene.get_allocator_mut();

        Ok(index_allocator)
    }

    pub fn get_bitmask_controller_mut(&mut self, scene: SceneHandle) -> Result<&mut BitmaskController> {
        // Get scene
        let target_scene = self.get_scene_mut(scene)?;

        // Get allocator from scene
        let controller = target_scene.get_bitmask_controller_mut();

        Ok(controller)
    }

    pub fn get_bitmask_controller(&mut self, scene: SceneHandle) -> Result<&BitmaskController> {
        // Get scene
        let target_scene = self.get_scene_mut(scene)?;

        // Get allocator from scene
        let controller = target_scene.get_bitmask_controller_mut();

        Ok(controller)
    }

    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        // Check if scene with that name already exists
        self.scenes.contains_key(name).eq(&false).ok_or(Error::new(EngineError::SceneAlreadyExists(name.to_string())))?;

        // Create and add new scene
        let new_scene = Scene::new(name.to_string());
        self.scenes.insert(name.to_string(), new_scene);
       
        // Return handle
        let new_scene_index = self.scenes.get_index_of(name).unwrap();
        Ok(SceneHandle::new(new_scene_index))
    }

    pub fn create_entity(&mut self, scene_handle: SceneHandle) -> Result<EntityHandle> {
        // Get scene
        let target_scene = self.get_scene_mut(scene_handle)?; // [TODO] Check if this will automatically return error and not Err(..) is needed. What if it returns Ok, function progresses? 
        
        // Get index allocator for entity
        let index_allocator = target_scene.get_allocator_mut();

        // Create new entity
        let new_entity = index_allocator.allocate_new_entity();

        // Insert new entity into scene
        // target_scene.entities.insert(target_scene.entity_counter, new_entity);
        if target_scene.entities.len() <= new_entity.get_index() {
            target_scene.entities.insert(target_scene.entity_counter, new_entity);
        }
        else {
            target_scene.entities[new_entity.get_index()] = new_entity;
        }
        target_scene.entity_counter += 1;


        // Get bitmask controller for new entity's bitmask allocation
        let target_bitmask_coontroller = target_scene.get_bitmask_controller_mut();

        // Allocate new bitmask entry for the entity
        target_bitmask_coontroller.add_new_entity_bitmask(0, new_entity.get_index().clone());

        // Return handle
        Ok(EntityHandle::new(new_entity.get_index(), new_entity.get_generation()))
    }

    pub fn add_component_to_entity<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene_handle: SceneHandle, entity: EntityHandle, component: T) -> Result<()> {     
        // Get scene
        let target_scene = self.get_scene_mut(scene_handle)?;

        // Get component storage from scene
        let component_storage = target_scene.get_component_storage_mut::<T>()?;

        // Add component to storage
        component_storage.set(entity.clone(), component);

        // Update the bitmask for the given entity
        target_scene.get_bitmask_controller_mut().update_after_component_insertion::<T>(entity.get_index().clone());

        Ok(())
    }

    pub fn set_active_scene(&mut self, scene_handle: SceneHandle) -> Result<()> {
        // Check if scene for that handle exists
        self.scenes.get_index_mut(scene_handle.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?;

        // Set new active scene
        self.active_scene = Some(scene_handle);
        Ok(())
    }

    pub fn get_scene(&self, scene_handle: SceneHandle) -> Result<&Scene> {
        // Get scene
        let scene = self.scenes.get_index(scene_handle.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?.1;
        Ok(scene)
    }

    pub fn get_scene_mut(&mut self, scene_handle: SceneHandle) -> Result<&mut Scene> {
        // Get scene
        let scene = self.scenes.get_index_mut(scene_handle.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?.1;
        Ok(scene)
    }

    pub fn get_scene_handle(&self, name: &str) -> Result<SceneHandle> {
        // Get scene index
        let scene_index = self.scenes.get_index_of(name).ok_or(Error::new(EngineError::InvalidSceneName(name.to_string())))?;

         // Return handle
         Ok(SceneHandle::new(scene_index))
    }

    pub fn fetch_one_component_storage<A: Component<Storage = ComponentStorage<A>>>(&mut self, scene: SceneHandle) -> Result<impl Iterator<Item = &RefCell<Option<A>>>> {

        let filtered_indexes = self.get_bitmask_controller_mut(scene)
                                                    .unwrap()
                                                    .filter_by_component::<A>()
                                                    .fetch_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();

        // Return iterator from scene

        Ok(target_scene.get_one_component_storage::<A>()
                    .enumerate()
                    .filter(move |(i, t)| filtered_indexes.contains(i))
                    .map(|(i, t)| t))
    }

    pub fn fetch_two_component_storages<A: Component<Storage = ComponentStorage::<A>>, 
                            B: Component<Storage = ComponentStorage<B>>>(&mut self, scene: SceneHandle) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>)>> {

        //Get filtered indexes for entities
        let filtered_indexes = self.get_bitmask_controller_mut(scene)
                                                    .unwrap()
                                                    .filter_by_component::<A>()
                                                    .filter_by_component::<B>()
                                                    .fetch_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();
        
        // Return iterator from scene
        Ok(target_scene.get_two_component_storages::<A, B>()
                    .enumerate()
                    .filter(move |(i, (t, u ))| filtered_indexes.contains(i))
                    .map(|(i, (t, u ))| (t, u)))
                    
    }

    pub fn fetch_three_component_storages<A: Component<Storage = ComponentStorage::<A>>, 
                            B: Component<Storage = ComponentStorage<B>>,
                            C: Component<Storage = ComponentStorage<C>>>(&mut self, scene: SceneHandle) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>)>> {

        //Get filtered indexes for entities
        let filtered_indexes = self.get_bitmask_controller_mut(scene)
                                                    .unwrap()
                                                    .filter_by_component::<A>()
                                                    .filter_by_component::<B>()
                                                    .filter_by_component::<C>()
                                                    .fetch_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();
        
        // Return iterator from scene
        Ok(target_scene.get_three_component_storages::<A, B, C>()
                    .enumerate()
                    .filter(move |(i, ((t, u ), w))| filtered_indexes.contains(i))
                    .map(|(i, ((t, u), w))| (t, u, w)))
                    
    }

    pub fn fetch_four_component_storages<A: Component<Storage = ComponentStorage::<A>>, 
                            B: Component<Storage = ComponentStorage<B>>,
                            C: Component<Storage = ComponentStorage<C>>,
                            D: Component<Storage = ComponentStorage<D>>>(&mut self, scene: SceneHandle) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>, &RefCell<Option<D>>)>> {

        //Get filtered indexes for entities
        let filtered_indexes = self.get_bitmask_controller_mut(scene)
                                                    .unwrap()
                                                    .filter_by_component::<A>()
                                                    .filter_by_component::<B>()
                                                    .filter_by_component::<C>()
                                                    .filter_by_component::<D>()
                                                    .fetch_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();
        
        // Return iterator from scene
        Ok(target_scene.get_four_component_storages::<A, B, C, D>()
                    .enumerate()
                    .filter(move |(i, (((a, b), c), d))| filtered_indexes.contains(i))
                    .map(|(i, (((a, b), c), d))| (a, b, c, d)))
                    
    }
}