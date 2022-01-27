use crate::{
    ecs::{ Scene, Entity, ComponentStorage, Component, EntityHandle, EntityFetcher, EntityBuilder, ComponentDestroyer }
};

use pill_core::{ EngineError, get_type_name, PillSlotMapKey };

use std::{ any::{ type_name, Any, TypeId }, collections::HashMap,  cell::RefCell };
use anyhow::{ Result, Context, Error };
use boolinator::Boolinator;

pill_core::define_new_pill_slotmap_key! { 
    pub struct SceneHandle;
}

pub struct SceneManager {
    pub(crate) scenes: pill_core::PillSlotMap<SceneHandle, Scene>, 
    pub(crate) mapping: pill_core::PillTwinMap<String, SceneHandle>, // Mapping from scene name to scene handle and vice versa
    pub(crate) max_entity_count: usize,
    active_scene_handle: Option<SceneHandle>,
}

impl SceneManager {
    pub fn new(max_entity_count: usize) -> Self {
	    Self { 
            scenes: pill_core::PillSlotMap::<SceneHandle, Scene>::with_key(),
            mapping: pill_core::PillTwinMap::<String, SceneHandle>::new(),
            max_entity_count,
            active_scene_handle: None,
        }
    }

    // --- Entity ---

    pub fn create_entity(&mut self, scene_handle: SceneHandle) -> Result<EntityHandle> {
        // Get maximum count of entities
        let max_entity_count = self.max_entity_count;

        // Get scene
        let target_scene = self.get_scene_mut(scene_handle)?;

        // Check if there is space for entity
        if target_scene.entities.len() >= max_entity_count {
            return Err(Error::new(EngineError::EntityLimitReached))
        }

        // Create new entity with empty bitmask
        let new_entity = Entity::new(scene_handle.clone());

        // Insert new entity into storage, with key as returned type
        let new_entity_handle = target_scene.entities.insert(new_entity);

        // Return handle to new entity
        Ok(new_entity_handle)
    }

    pub fn remove_entity(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle) -> Result<Vec::<Box<dyn ComponentDestroyer>>> {
        // Initialize collection for component destroyers to return to engine
        let mut component_destroyers = Vec::<Box<dyn ComponentDestroyer>>::new();
        
        // Get scene
        let target_scene = self.get_scene_mut(scene_handle)?;

        // Get entity bitmask
        let entity_bitmask = target_scene.entities.get_mut(entity_handle).unwrap().bitmask;

        // Get typeids of all components this entity has
        let components_typeids = target_scene.get_components_typeids_from_bitmask(entity_bitmask);

        // Get component destroyers to return to engine so it can call them
        for typeid in components_typeids {
            let component_destroyer = target_scene.get_component_destoyer(&typeid).unwrap();
            component_destroyers.push(component_destroyer);
        }
       
        // Remove entity from storage
        target_scene.entities.remove(entity_handle);

        Ok(component_destroyers)
    }

    // --- Component ---

    pub fn register_component<T>(&mut self, scene: SceneHandle) -> Result<()> 
        where T: Component<Storage = ComponentStorage::<T>>
    {
        // Prepare the capacity for component storage
        let component_storage_capacity = self.max_entity_count.clone();

        // Get scene
        let target_scene = self.get_scene_mut(scene)?;

        // Check if component is already registered
        if target_scene.is_component_registered::<T>() {
            return Err(Error::new(EngineError::ComponentAlreadyRegistered(get_type_name::<T>(), target_scene.name.clone())));
        }

        // Create new component storage
        let component_storage = ComponentStorage::<T>::new(component_storage_capacity);

        // Add component storage to scene
        target_scene.components.insert::<T>(component_storage);

        // Add bitmask for new component
        target_scene.add_component_bitmask::<T>();

        // Add component destroyer
        target_scene.add_component_destroyer::<T>();

        Ok(())
    }
    
    pub fn add_component_to_entity<T>(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle, component: T) -> Result<()> 
        where T: Component<Storage = ComponentStorage::<T>>
    {     
        // Get scene
        let target_scene = self.get_scene_mut(scene_handle)?;

        // Get component storage from scene
        let component_storage = target_scene.get_component_storage_mut::<T>()?;

        // Add component to storage
        let component_slot = component_storage.data.get_mut(entity_handle.data().index as usize).expect("Critical: Vector not initialized");
        component_slot.borrow_mut().insert(component);
        
        // Get the component bitmask
        let component_bitmask = target_scene.get_component_bitmask::<T>()?;
        
        // Update entity bitmask
        target_scene.entities.get_mut(entity_handle).unwrap().bitmask |= component_bitmask;

        Ok(())
    }

    pub fn remove_component_from_entity<T>(&mut self, scene_handle: SceneHandle, entity_handle: EntityHandle) -> Result<T> 
        where T: Component<Storage = ComponentStorage::<T>>
    {
        // Get scene
        let target_scene = self.get_scene_mut(scene_handle)?;

        // Get component bitmask
        let component_bitmask = target_scene.get_component_bitmask::<T>()?;

        // Update entity bitmask
        target_scene.entities.get_mut(entity_handle).unwrap().bitmask -= component_bitmask;

        // Get component storage from scene
        let component_storage = target_scene.get_component_storage_mut::<T>()?;

        // Delete the component from storage
        let mut component_slot = component_storage.data.get_mut(entity_handle.data().index as usize).expect("Critical: Vector not initialized").borrow_mut();
        let component: T = component_slot.take().unwrap();

        Ok(component)
    }

    pub fn get_entity_component<T>(&self, entity_handle: EntityHandle, scene_handle: SceneHandle) -> Result<&RefCell<Option<T>>>
        where T: Component<Storage = ComponentStorage::<T>>
    {
        // Get scene
        let target_scene = self.get_scene(scene_handle)?;

        // Get storage
        let storage = target_scene.components.get::<T>().unwrap();

        // Check if entity has requested component
        let entity = target_scene.entities.get(entity_handle).unwrap();

        // Get the bitmask mapped onto the given component to update entity's bitmask
        let component_bitmask = target_scene.get_component_bitmask::<T>()?;

        match entity.bitmask & component_bitmask != 0 {
            true => Ok(storage.data.get(entity_handle.0.index as usize).unwrap()),
            false => Err(Error::msg("Not found")),
        }
    }  

    // --- Scene ---

    pub fn create_scene(&mut self, name: &str) -> Result<SceneHandle> {
        // Check if scene with that name already exists
        if self.mapping.contains_key(&name.to_string()) {
            return Err(Error::new(EngineError::SceneAlreadyExists(name.to_string())))
        }

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

    pub fn remove_scene(&mut self, scene_handle: SceneHandle) -> Result<Scene> {
        let scene = self.scenes.get_mut(scene_handle).ok_or(Error::new(EngineError::InvalidSceneHandle))?;

        // Remove scene
        let scene = self.scenes.remove(scene_handle).ok_or(Error::new(EngineError::InvalidSceneHandle))?;

        // Return deleted scene
        Ok(scene)
    }

    // --- Active scene ---
    
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

        Ok(active_scene)
    }

    pub fn get_active_scene_mut(&mut self) -> Result<&mut Scene> {
        // Check if active scene is set
        let active_scene_handle = self.active_scene_handle.ok_or(Error::new(EngineError::NoActiveScene))?;
        let active_scene = self.get_scene_mut(active_scene_handle)?;

        Ok(active_scene)
    }

    // - Iterators

    pub fn fetch_one_component_storage<A>(&self, scene: SceneHandle) -> Result<impl Iterator<Item = &RefCell<Option<A>>>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>
    {
        // Get filtered indexes for entities
        let filtered_indexes = EntityFetcher::new(self, scene.clone())
                                                        .filter_by_component::<A>()
                                                        .fetch_indexes();

        // Get scene
        let target_scene = self.get_scene(scene).unwrap();

        // Get storages
        let storages = target_scene.get_component_storage::<A>().unwrap().data.iter();

        // Return iterator from scene
        let iterator = storages
            .enumerate()
            .filter(move |(i, _t)| filtered_indexes.contains(i))
            .map(|(_i, t)| t);

        Ok(iterator)
    }

    pub fn fetch_one_component_storage_with_entity_handles<A>(&self, scene: SceneHandle) -> Result<impl Iterator<Item = (EntityHandle, &RefCell<Option<A>>)>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>
    {
        // Get filtered indexes for entities
        let (mut filtered_entities, filtered_indexes) = EntityFetcher::new(self, scene.clone())
                                                        .filter_by_component::<A>()
                                                        .fetch_entities_and_indexes();

        // Get scene
        let target_scene = self.get_scene(scene).unwrap();

        // Get storages
        let storages = target_scene.get_component_storage::<A>().unwrap().data.iter();

        // Return iterator from scene
        let iterator = storages
            .enumerate()
            .filter(move |(i, _t)| filtered_indexes.contains(i))
            .map(move |(_i, t)| (filtered_entities.pop_front().unwrap(), t));

        Ok(iterator)
    }


    pub fn fetch_two_component_storages<A, B>(&self, scene: SceneHandle) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>)>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>
    {
        // Get filtered indexes for entities
        let filtered_indexes = EntityFetcher::new(self, scene.clone())
                                                        .filter_by_component::<A>()
                                                        .filter_by_component::<B>()
                                                        .fetch_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();
        
        // Get storages
        let storages = target_scene.get_component_storage::<A>()?.data.iter()
            .zip(target_scene.get_component_storage::<B>()?.data.iter());

        // Return iterator from scene
        let iterator = storages
            .enumerate()
            .filter(move |(i, (_t, _u ))| filtered_indexes.contains(i))
            .map(|(_i, (t, u ))| (t, u));

        Ok(iterator)
    }

    pub fn fetch_two_component_storages_with_entity_handles<A, B>(&self, scene: SceneHandle) -> Result<impl Iterator<Item = (EntityHandle, &RefCell<Option<A>>, &RefCell<Option<B>>)>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>
    {
        // Get filtered indexes for entities
        let (mut filtered_entities, filtered_indexes) = EntityFetcher::new(self, scene.clone())
                                                        .filter_by_component::<A>()
                                                        .filter_by_component::<B>()
                                                        .fetch_entities_and_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();
        
        // Get storages
        let storages = target_scene.get_component_storage::<A>()?.data.iter()
            .zip(target_scene.get_component_storage::<B>()?.data.iter());

        // Return iterator from scene
        let iterator = storages
                    .enumerate()
                    .filter(move |(i, (_t, _u ))| filtered_indexes.contains(i))
                    .map(move |(_i, (t, u ))| (filtered_entities.pop_front().unwrap(), t, u));

        Ok(iterator)
    }

    pub fn fetch_three_component_storages<A, B, C>(&self, scene: SceneHandle) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>)>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>,
        C: Component<Storage = ComponentStorage::<C>>
    {
        // Get filtered indexes for entities
        let filtered_indexes = EntityFetcher::new(self, scene.clone())
                                                        .filter_by_component::<A>()
                                                        .filter_by_component::<B>()
                                                        .filter_by_component::<C>()
                                                        .fetch_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();

        // Get storages
        let storages = target_scene.get_component_storage::<A>()?.data.iter()
            .zip(target_scene.get_component_storage::<B>()?.data.iter())
            .zip(target_scene.get_component_storage::<C>()?.data.iter());

        // Return iterator from scene
        let iterator = storages
            .enumerate()
            .filter(move |(i, ((_t, _u ), _w))| filtered_indexes.contains(i))
            .map(|(_i, ((t, u), w))| (t, u, w));

        Ok(iterator)
                    
    }

    pub fn fetch_three_component_storages_with_entity_handles<A, B, C>(&self, scene: SceneHandle) -> Result<impl Iterator<Item = (EntityHandle, &RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>)>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>,
        C: Component<Storage = ComponentStorage::<C>>
    {
        // Get filtered indexes for entities
        let (mut filtered_entities, filtered_indexes) = EntityFetcher::new(self, scene.clone())
                                                        .filter_by_component::<A>()
                                                        .filter_by_component::<B>()
                                                        .filter_by_component::<C>()
                                                        .fetch_entities_and_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();
        
        // Get storages
        let storages = target_scene.get_component_storage::<A>()?.data.iter()
            .zip(target_scene.get_component_storage::<B>()?.data.iter())
            .zip(target_scene.get_component_storage::<C>()?.data.iter());

        // Return iterator from scene
        let iterator = storages
            .enumerate()
            .filter(move |(i, ((_t, _u ), _w))| filtered_indexes.contains(i))
            .map(move |(_i, ((t, u), w))| (filtered_entities.pop_front().unwrap(), t, u, w));

        Ok(iterator)
    }

    pub fn fetch_four_component_storages<A, B, C, D>(&self, scene: SceneHandle) -> Result<impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>, &RefCell<Option<D>>)>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>,
        C: Component<Storage = ComponentStorage::<C>>,
        D: Component<Storage = ComponentStorage::<D>>
    {
        // Get filtered indexes for entities
        let filtered_indexes = EntityFetcher::new(self, scene.clone())
                                                        .filter_by_component::<A>()
                                                        .filter_by_component::<B>()
                                                        .filter_by_component::<C>()
                                                        .filter_by_component::<D>()
                                                        .fetch_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();
        
        // Get storages
        let storages = target_scene.get_component_storage::<A>().unwrap().data.iter()
            .zip(target_scene.get_component_storage::<B>().unwrap().data.iter())
            .zip(target_scene.get_component_storage::<C>().unwrap().data.iter())
            .zip(target_scene.get_component_storage::<D>().unwrap().data.iter());

        // Return iterator from scene
        let iterator = storages
            .enumerate()
            .filter(move |(i, (((_a, _b), _c), _d))| filtered_indexes.contains(i))
            .map(|(_i, (((a, b), c), d))| (a, b, c, d));

        Ok(iterator)
    }

    pub fn fetch_four_component_storages_with_entity_handles<A, B, C, D>(&self, scene: SceneHandle) -> Result<impl Iterator<Item = (EntityHandle, &RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>, &RefCell<Option<D>>)>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>,
        C: Component<Storage = ComponentStorage::<C>>,
        D: Component<Storage = ComponentStorage::<D>>
    {
        // Get filtered indexes for entities
        let (mut filtered_entities, filtered_indexes) = EntityFetcher::new(self, scene.clone())
                                                        .filter_by_component::<A>()
                                                        .filter_by_component::<B>()
                                                        .filter_by_component::<C>()
                                                        .filter_by_component::<D>()
                                                        .fetch_entities_and_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();
        
        // Get storages
        let storages = target_scene.get_component_storage::<A>().unwrap().data.iter()
            .zip(target_scene.get_component_storage::<B>().unwrap().data.iter())
            .zip(target_scene.get_component_storage::<C>().unwrap().data.iter())
            .zip(target_scene.get_component_storage::<D>().unwrap().data.iter());

        // Return iterator from scene
        let iterator = storages
                    .enumerate()
                    .filter(move |(i, (((_a, _b), _c), _d))| filtered_indexes.contains(i))
                    .map(move |(_i, (((a, b), c), d))| (filtered_entities.pop_front().unwrap(), a, b, c, d));

        Ok(iterator)
    }
}