use std::{any::type_name, collections::HashMap};
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

    pub fn fetch_two_storages<T: Component<Storage = ComponentStorage::<T>>, 
                            U: Component<Storage = ComponentStorage<U>>>(&mut self, scene: SceneHandle) {
        
        // Get filtering bitmask, which we can apply to get the correct entities
        // let filtering_bitmask = self.get_bitmask_controller_mut(scene)
        //                                             .unwrap()
        //                                             .filter_by_component::<T>()
        //                                             .filter_by_component::<U>()
        //                                             .get_filtering_bitmask();

        //Get filtered indexes for entities
        let filtered_indexes = self.get_bitmask_controller_mut(scene)
                                                    .unwrap()
                                                    .filter_by_component::<T>()
                                                    .filter_by_component::<U>()
                                                    .fetch_indexes();
        
        // Clean filtering bitmask for sake of future filterining bitmasks creation
        // self.get_bitmask_controller_mut(scene).unwrap().clear_filter();
        
        // let target_scene = self.get_scene_mut(scene).unwrap();

        // let first = target_scene.get_component_storage_element_mut_at::<T>(0);
        // let second = target_scene.get_component_storage_element_mut_at::<U>(0);
        // // let mut T_storage = Vec::<&StorageEntry<T>>::new();
        // // let mut U_storage = Vec::<&StorageEntry<U>>::new();
        
        // println!("{} {}", first.generation)
        // {
        //     let storage = &self.get_scene_mut(scene).unwrap().get_component_storage_mut::<T>().data;
        //     for item in storage {
        //         T_storage.push(item.clone());
        //     }
        // }

        // {
        //     let storage = &self.get_scene_mut(scene).unwrap().get_component_storage_mut::<U>().data;
        //     for item in storage {
        //         U_storage.push(item.clone());
        //     }
        // }
        
        // for (&first, &second) in izip!(T_storage, U_storage) {
        //     println!("hmm");
        // }
        
        // for (one, two) in izip!(first, second) {
        //     println!("Hello");
        // }
    }

}

#[cfg(test)]
mod test {
    
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
        
        let health_storage = scene_manager.get_scene_mut(scene).unwrap().get_component_storage_mut::<HealthComponent>();

        assert_eq!(3, health_storage.data.len());
    }

    struct FirstStruct(u32);
    struct SecondStruct(bool, String);
    struct ThirdStruct(FirstStruct, SecondStruct, usize, bool);
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

    #[test]
    fn test_basic_iteration() {
        let mut scene_manager = SceneManager::new();

        let scene = scene_manager.create_scene("Default").unwrap();
        let first = scene_manager.create_entity(scene).unwrap();
        let second = scene_manager.create_entity(scene).unwrap();
        let third = scene_manager.create_entity(scene).unwrap();

        scene_manager.register_component::<FirstStruct>(scene);
        scene_manager.register_component::<FourthStruct>(scene);

        scene_manager.add_component_to_entity(scene, first, FirstStruct(10));
        scene_manager.add_component_to_entity(scene, first, FourthStruct(None));
        scene_manager.add_component_to_entity(scene, second, FourthStruct(Some(false)));
        scene_manager.add_component_to_entity(scene, third, FirstStruct(1));
        scene_manager.add_component_to_entity(scene, third, FourthStruct(Some(true)));

        //scene_manager.fetch_two_storages::<FirstStruct, FourthStruct>(scene);
    }
}