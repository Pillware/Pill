use std::{default, io::Error, ops::Index};

use pill_core::na::Storage;
use core::default::Default;

use super::EntityHandle;

pub struct StorageEntry<T> {
    pub component: Option<T>,
    pub generation: u32
}

impl <T> Default for StorageEntry<T> {
    fn default() -> Self {
        Self {
            component: None,
            generation: 0
        }
    }
}

impl<T> StorageEntry<T> {
    pub fn new(comp: T, gen: u32) -> Self {
        Self {
            component: Some(comp),
            generation: gen
        }
    }

    // Getters and setters

    pub fn set_generation(&mut self, gen: u32) {
        self.generation = gen;
    }

    pub fn set_component(&mut self, comp: T) {
        self.component = Some(comp);
    }

    pub fn get_generation(&self) -> &u32 {
        &self.generation
    }

    pub fn get_component(&self) -> Option<&T> {
        match self.component.is_none() {
            true => return None,
            false => return self.component.as_ref()
        }
    }

    pub fn get_generation_mut(&mut self) -> &mut u32 {
        &mut self.generation
    }

    pub fn get_component_mut(&mut self) -> Option<&mut T> {
        match self.component.is_none() {
            false => return None,
            true => return self.component.as_mut()
        }
    }

}

pub struct ComponentStorage<T> {
    pub data: Vec<StorageEntry<T>>,
}

impl<T> ComponentStorage<T> {
    pub fn new() -> Self {  
        Self { 
            data: Vec::<StorageEntry<T>>::new(),
        }
    }

    pub fn set(&mut self, handle: EntityHandle, comp: T) {
        while self.data.len() <= handle.index {
            self.data.push(StorageEntry::default())
        }
        self.data[handle.index].set_generation(handle.generation);
        self.data[handle.index].set_component(comp);
    }

    pub fn get(&self, handle: EntityHandle) -> Option<&T> {
        if self.data[handle.index].generation == handle.generation {
            match &self.data[handle.index].component.is_none() {
                true => return None,
                false => return self.data[handle.index].component.as_ref()
            }
        }
        None
    }

    pub fn get_mut(&mut self, handle: EntityHandle) -> Option<&mut T> {
        if self.data[handle.index].generation == handle.generation {
            match &self.data[handle.index].component.is_none() {
                true => return None,
                false => return self.data[handle.index].component.as_mut()
            }
        }
        None
    }

    pub fn fill_up(&mut self, length : usize) {
        for _ in 0..length {
            let entry = StorageEntry::<T>::default();
            self.data.push(entry);
        }
    }

}

impl<T> Iterator for ComponentStorage<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}

#[cfg(test)]
mod test {
    use std::slice::SliceIndex;

    use itertools::izip;
    use scene::Scene;
    use scene_manager::SceneManager;

    use crate::ecs::{EntityHandle, scene_manager, Component, scene};

    use super::{ComponentStorage, StorageEntry};

    #[test]
    fn basic_component_insertion() {
        let mut components = ComponentStorage::<u64>::new();

        let number: u64 = 10;
        let handle = EntityHandle::new(0, 0);

        components.set(handle, number);

        assert_eq!(Some(&10), components.get(handle));

        components.set(handle, 20);
        assert_eq!(Some(&20), components.get(handle));

        let second_handle = EntityHandle::new(0, 1);
        components.set(second_handle, 30);
        assert_eq!(None, components.get(handle));
        assert_eq!(Some(&30), components.get(second_handle));
    }

    #[test]
    fn mutable_component_test() {
        let mut components = ComponentStorage::<String>::new();

        let first = EntityHandle::new(0, 0);
        let second = EntityHandle::new(1, 1);

        components.set(first, String::from("TEST STRING"));
        assert_eq!(components.get(first), Some(&String::from("TEST STRING")));

        let new_string = components.get(first).unwrap().to_owned() + &String::from(" WORKS");
        components.set(first, new_string.to_string());
        assert_eq!(components.get(first), Some(&String::from("TEST STRING WORKS")))
    }

    struct Health(u32);
    struct Shield(i32);
    struct Name(String);
    
    impl Component for Health { type Storage = ComponentStorage<Self> ;}
    impl Component for Shield { type Storage = ComponentStorage<Self> ;}
    impl Component for Name { type Storage = ComponentStorage<Self> ;}


    #[test]
    fn basic_multiple_components_iteration_test() {
        // Basic scenario for iterating over entities
        // Two entities containing health, shield, and name components
        // We want to iterate over them to create system for dealing damage to them
        // Let's assume, that we want to have all three components shown
        // Name will be printed, and either shield and/or health will be reduced by some constant

        // Create scene manager
        let mut scene_manager = SceneManager::new();

        // Create scene
        let mut scene = scene_manager.create_scene("Default").unwrap();

        // Register components
        scene_manager.register_component::<Health>(scene);
        scene_manager.register_component::<Shield>(scene);
        scene_manager.register_component::<Name>(scene);

        // Create some damage variable
        let damage = 15;

        // Create entities
        let first_entity = scene_manager.create_entity(scene).unwrap();
        let second_entity = scene_manager.create_entity(scene).unwrap();

        // Add components to entities
        scene_manager.add_component_to_entity(scene, first_entity, Health(15));
        scene_manager.add_component_to_entity(scene, first_entity, Shield(10));
        scene_manager.add_component_to_entity(scene, first_entity, Name(String::from("Frodo")));

        scene_manager.add_component_to_entity(scene, second_entity, Health(5));
        scene_manager.add_component_to_entity(scene, second_entity, Shield(5));
        scene_manager.add_component_to_entity(scene, second_entity, Name(String::from("Sam")));
        
        // Get components storages

        let target_scene = scene_manager.get_scene(scene).unwrap();
        let first_storage = target_scene.get_component_storage::<Health>();
        let second_storage = target_scene.get_component_storage::<Shield>();
        let third_storage = target_scene.get_component_storage::<Name>();

    }
}