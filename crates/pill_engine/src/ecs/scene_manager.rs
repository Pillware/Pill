use std::{any::type_name, collections::HashMap, cell::RefCell, path::Iter};
use anyhow::{Result, Context, Error};
use boolinator::Boolinator;
use pill_core::{EngineError, get_type_name};
use indexmap::IndexMap;
use typemap_rev::TypeMapKey;
use itertools::*;

use crate::ecs::*;

use super::component_storage::StorageEntry;

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

    // Getters and setters

    pub fn get_active_scene(&self) -> Result<SceneHandle> {
        // Check if active scene is set
        let active_scene_handle = self.active_scene.ok_or(Error::new(EngineError::NoActiveScene))?;

        // Return active scene handle
        Ok(active_scene_handle.clone())
    }

    pub fn set_active_scene(&mut self, scene: SceneHandle) -> Result<()> {
        // Check if scene for that handle exists
        self.scenes.get_index_mut(scene.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?;

        // Set new active scene
        self.active_scene = Some(scene);
        Ok(())
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

    pub fn get_scene_mut(&mut self, scene: SceneHandle) -> Result<&mut Scene> {
        // Get scene
        let scene = self.scenes.get_index_mut(scene.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?.1;
        Ok(scene)
    }

    pub fn get_scene(&self, scene: SceneHandle) -> Result<&Scene> {
        // Get scene
        let scene = self.scenes.get_index(scene.index).ok_or(Error::new(EngineError::InvalidSceneHandle))?.1;
        Ok(scene)
    }

    pub fn get_component_storage_mut<T: Component<Storage = ComponentStorage<T>>>(&mut self, scene: SceneHandle) -> Result<&mut ComponentStorage<T>> {
        let target_scene = self.get_scene_mut(scene)?;
        
        let storage = target_scene.get_component_storage_mut::<T>();
        Ok(storage)
    }

    pub fn get_component_storage<T: Component<Storage = ComponentStorage<T>>>(&mut self, scene: SceneHandle) -> Result<&ComponentStorage<T>> {
        let target_scene = self.get_scene(scene)?;
        
        let storage = target_scene.get_component_storage::<T>();
        Ok(storage)
    }


    // Utility functions

    pub fn register_component<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene: SceneHandle) -> Result<()> {
        // Get scene
        let target_scene = self.get_scene_mut(scene)?;

        // Check if component storage is already registered
        target_scene.components.contains_key::<T>().eq(&false).ok_or(Error::new(EngineError::ComponentAlreadyRegistered(get_type_name::<T>(), target_scene.name.clone())))?;

        // Create new component storage
        let component_storage = ComponentStorage::<T>::new();

        // Add component storage to scene
        target_scene.components.insert::<T>(component_storage);

        // Get storage and max index as count
        let (storage, count) = target_scene.get_component_storage_mut_with_count::<T>();

        // Add default components up till the size of max index to storage to match size from other storages
        storage.fill_up(*count);

        // Add bitmask mapping for component

        target_scene.bitmask_controller.add_bitmap::<T>();

        Ok(())
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
        let target_scene = self.get_scene_mut(scene)?; // [TODO] Check if this will automatically return error and not Err(..) is needed. What if it returns Ok, function progresses? 
        
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


    pub fn add_component_to_entity<T: Component<Storage = ComponentStorage::<T>>>(&mut self, scene: SceneHandle, entity: EntityHandle, component: T) -> Result<()> {     
        // Get scene
        let target_scene = self.get_scene_mut(scene)?;

        // Get component storage from scene
        let component_storage = target_scene.get_component_storage_mut::<T>();

        // Add component to storage
        component_storage.set(entity.clone(), component);

        // Update the bitmask for the given entity
        target_scene.get_bitmask_controller_mut().update_after_component_insertion::<T>(entity.get_index().clone());

        Ok(())
    }

    pub fn fetch_one_component_storage<A: Component<Storage = ComponentStorage<A>>>(&mut self, scene: SceneHandle) -> impl Iterator<Item = &RefCell<Option<A>>> {

        let filtered_indexes = self.get_bitmask_controller_mut(scene)
                                                    .unwrap()
                                                    .filter_by_component::<A>()
                                                    .fetch_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();

        // Return iterator from scene
        target_scene.get_one_component_storage::<A>()
                    .enumerate()
                    .filter(move |(i, t)| filtered_indexes.contains(i))
                    .map(|(i, t)| t)
    }

    pub fn fetch_two_component_storages<A: Component<Storage = ComponentStorage::<A>>, 
                            B: Component<Storage = ComponentStorage<B>>>(&mut self, scene: SceneHandle) -> impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>)> {

        //Get filtered indexes for entities
        let filtered_indexes = self.get_bitmask_controller_mut(scene)
                                                    .unwrap()
                                                    .filter_by_component::<A>()
                                                    .filter_by_component::<B>()
                                                    .fetch_indexes();
        
        // Get scene
        let target_scene = self.get_scene(scene).unwrap();
        
        // Return iterator from scene
        target_scene.get_two_component_storages::<A, B>()
                    .enumerate()
                    .filter(move |(i, (t, u ))| filtered_indexes.contains(i))
                    .map(|(i, (t, u ))| (t, u))
                    
    }

    pub fn fetch_three_component_storages<A: Component<Storage = ComponentStorage::<A>>, 
                            B: Component<Storage = ComponentStorage<B>>,
                            C: Component<Storage = ComponentStorage<C>>>(&mut self, scene: SceneHandle) -> impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>)> {

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
        target_scene.get_three_component_storages::<A, B, C>()
                    .enumerate()
                    .filter(move |(i, ((t, u ), w))| filtered_indexes.contains(i))
                    .map(|(i, ((t, u), w))| (t, u, w))
                    
    }

    pub fn fetch_four_component_storages<A: Component<Storage = ComponentStorage::<A>>, 
                            B: Component<Storage = ComponentStorage<B>>,
                            C: Component<Storage = ComponentStorage<C>>,
                            D: Component<Storage = ComponentStorage<D>>>(&mut self, scene: SceneHandle) -> impl Iterator<Item = (&RefCell<Option<A>>, &RefCell<Option<B>>, &RefCell<Option<C>>, &RefCell<Option<D>>)> {

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
        target_scene.get_four_component_storages::<A, B, C, D>()
                    .enumerate()
                    .filter(move |(i, (((a, b), c), d))| filtered_indexes.contains(i))
                    .map(|(i, (((a, b), c), d))| (a, b, c, d))
                    
    }
    
}

#[cfg(test)]
mod test {
    
    use std::borrow::Borrow;

    use super::*;

    #[test]
    fn test_simple_component_addition() {
        let mut scene_manager = SceneManager::new();
        let scene = scene_manager.create_scene("Default").unwrap();

        scene_manager.set_active_scene(scene);

        scene_manager.register_component::<NameComponent>(scene);

        let entity_1 = scene_manager.create_entity(scene).unwrap();
        scene_manager.add_component_to_entity(scene, entity_1, NameComponent {name: String::from("Text 1")});

        let entity_2 = scene_manager.create_entity(scene).unwrap();
        scene_manager.add_component_to_entity(scene, entity_2, NameComponent::default());

        let entity_3 = scene_manager.create_entity(scene).unwrap();
        scene_manager.add_component_to_entity(scene, entity_3, NameComponent {name: String::from("Text 3")});
        
        let storage = scene_manager.get_scene_mut(scene).unwrap().get_component_storage_mut::<NameComponent>();
        scene_manager.register_component::<HealthComponent>(scene);

        scene_manager.add_component_to_entity(scene, entity_1, HealthComponent {value: 10});
        scene_manager.add_component_to_entity(scene, entity_2, HealthComponent {value: 20});
    }

    #[derive(Debug)]
    struct FirstStruct(u32);
    #[derive(Debug)]
    struct SecondStruct(bool, String);
    #[derive(Debug)]
    struct ThirdStruct(FirstStruct, SecondStruct, usize, bool);
    #[derive(Debug)]
    struct FourthStruct(Option<bool>);

    impl Component for FirstStruct { type Storage = ComponentStorage<Self> ;}
    impl Component for SecondStruct { type Storage = ComponentStorage<Self> ;}
    impl Component for ThirdStruct { type Storage = ComponentStorage<Self>; }
    impl Component for FourthStruct {type Storage = ComponentStorage<Self>; }

    #[test] 
    fn test_bitmask_addition() {
        let mut scene_manager = SceneManager::new();
        let scene = scene_manager.create_scene("Default").unwrap();
        let entity = scene_manager.create_entity(scene).unwrap();

        scene_manager.register_component::<FirstStruct>(scene);
        scene_manager.register_component::<SecondStruct>(scene);
        scene_manager.register_component::<ThirdStruct>(scene);
        scene_manager.register_component::<FourthStruct>(scene);

        let bitmask_controller = scene_manager.get_bitmask_controller_mut(scene).unwrap();

        assert_eq!(bitmask_controller.get_bitmap::<FirstStruct>(), &0b0001);
        assert_eq!(bitmask_controller.get_bitmap::<ThirdStruct>(), &0b0100);
        assert_eq!(bitmask_controller.get_bitmap::<FourthStruct>(), &0b1000);
        assert_eq!(bitmask_controller.get_bitmap::<SecondStruct>(), &0b0010);
    }

    #[test]
    fn test_bitmask_addition_and_update() {
        let mut scene_manager = SceneManager::new();
        let scene = scene_manager.create_scene("Default").unwrap();
        let entity = scene_manager.create_entity(scene).unwrap();

        scene_manager.register_component::<FirstStruct>(scene);
        scene_manager.register_component::<SecondStruct>(scene);
        scene_manager.register_component::<ThirdStruct>(scene);
        scene_manager.register_component::<FourthStruct>(scene);

        {
        assert_eq!(scene_manager.get_bitmask_controller(scene).unwrap().get_entity_bitmask(entity.get_index()), &0b000);
        }

        scene_manager.add_component_to_entity(scene, entity, SecondStruct(true, String::from("Default")));

        {
        assert_eq!(scene_manager.get_bitmask_controller(scene).unwrap().get_entity_bitmask(entity.get_index()), &0b0010);
        }

        scene_manager.add_component_to_entity(scene, entity, FourthStruct(Some(true)));

        {
        assert_eq!(scene_manager.get_bitmask_controller(scene).unwrap().get_entity_bitmask(entity.get_index()), &0b1010);
        }
    } 

    #[derive(Debug)]
    struct Health(u32);
    #[derive(Debug)]
    struct Shield(i32);
    #[derive(Debug)]
    struct Name(String);
    #[derive(Debug)]
    struct Charisma(i32);

    impl Component for Health { type Storage = ComponentStorage<Self> ;}
    impl Component for Shield { type Storage = ComponentStorage<Self> ;}
    impl Component for Name { type Storage = ComponentStorage<Self> ;}
    impl Component for Charisma { type Storage = ComponentStorage<Self> ;}

    #[test]
    fn test_basic_iteration() {
        let mut scene_manager = SceneManager::new();

        let scene = scene_manager.create_scene("Default").unwrap();
        let first_entity = scene_manager.create_entity(scene).unwrap();
        let second_entity = scene_manager.create_entity(scene).unwrap();
        let third_entity = scene_manager.create_entity(scene).unwrap();

        scene_manager.register_component::<Health>(scene);
        scene_manager.register_component::<Shield>(scene);
        scene_manager.register_component::<Name>(scene);

        scene_manager.add_component_to_entity(scene, first_entity, Health(20));
        scene_manager.add_component_to_entity(scene, second_entity, Health(50));
        scene_manager.add_component_to_entity(scene, third_entity, Health(30));

        scene_manager.add_component_to_entity(scene, first_entity, Shield(15));
        scene_manager.add_component_to_entity(scene, third_entity, Shield(10));

        scene_manager.add_component_to_entity(scene, first_entity, Name(String::from("Gimli")));
        scene_manager.add_component_to_entity(scene, second_entity, Name(String::from("Legolas")));
        scene_manager.add_component_to_entity(scene, third_entity, Name(String::from("Aragorn")));

        for (first, second, third) in scene_manager.fetch_three_component_storages::<Health, Name, Shield>(scene) {
            println!("{:?} {:?}", first.borrow_mut().as_mut(), second.borrow_mut().as_mut());
            first.borrow_mut().as_mut().unwrap().0 *= 10;
        }

        println!("");

        for health in scene_manager.fetch_one_component_storage::<Health>(scene) {
            println!("{:?}", health.borrow().as_ref());
        }

        println!("");

        for (first, second) in scene_manager.fetch_two_component_storages::<Health, Name>(scene) {
            println!("{:?} {:?}", first.borrow_mut().as_mut(), second.borrow_mut().as_mut());
            first.borrow_mut().as_mut().unwrap().0 *= 10;
        }

        println!("");

        scene_manager.register_component::<Charisma>(scene);
        scene_manager.add_component_to_entity(scene, second_entity, Charisma(8));
        scene_manager.add_component_to_entity(scene, third_entity, Charisma(12));

        for (name, health, shield, charisma) in scene_manager.fetch_four_component_storages::<Name, Health, Shield, Charisma>(scene) {
            println!("{:?} {:?} {:?} {:?}", name.borrow_mut().as_mut(), health.borrow_mut().as_mut(), shield.borrow().as_ref(), charisma.borrow().as_ref());
        }

        println!("");

        for(name, health, charisma) in scene_manager.fetch_three_component_storages::<Name, Health, Charisma>(scene) {
            println!("{:?} {:?} {:?}", name.borrow().as_ref(), charisma.borrow().as_ref(), health.borrow().as_ref())
        }

    }
}