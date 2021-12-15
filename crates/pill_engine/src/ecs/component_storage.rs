use std::{default, io::Error, ops::Index, cell::{RefCell, RefMut}, borrow::Borrow, os::windows::prelude::OpenOptionsExt};

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

#[derive(Debug)]
pub struct ComponentStorage<T> {
    pub data: Vec<RefCell<Option<T>>>,
}

unsafe impl<T> Sync for ComponentStorage<T> {}

impl<T> ComponentStorage<T> {
    pub fn new() -> Self {  
        Self { 
            data: Vec::<RefCell<Option<T>>>::new()
        }
    }

    pub fn set(&mut self, handle: EntityHandle, comp: T) {
        while self.data.len() <= handle.index {
            self.data.push(RefCell::new(None))
        }
        self.data[handle.index] = RefCell::new(Some(comp));
    }

    pub fn get(&self, index: usize) -> Option<T> {
        return None;
        // if self.data.borrow().len() <= index {
        //     return None
        // }
        // else {
        //      match &self.data.borrow()[index].is_none() {
        //          true => return None,
        //          false => return self.data.borrow()[index]
        //      }
        // }
        // if self.data[handle.index].generation == handle.generation {
        //     match &self.data[handle.index].component.is_none() {
        //         true => return None,
        //         false => return self.data[handle.index].component.as_ref()
        //     }
        // }
        // None
    }
    
    // fn borrow_component_vec<ComponentType: 'static>(
    //     &self,
    // ) -> Option<RefMut<Vec<Option<ComponentType>>>> {
    //     for component_vec in self.data.iter() {
    //         if let Some(component_vec) = component_vec
    //             .as_any()
    //             .downcast_ref::<RefCell<Vec<Option<ComponentType>>>>()
    //         {
    //             // Here we use `borrow_mut`. 
    //             // If this `RefCell` is already borrowed from this will panic.
    //             return Some(component_vec.borrow_mut());
    //         }
    //     }
    //     None
    // }

    pub fn get_mut(&mut self, handle: EntityHandle) -> Option<&mut T> {
        return None;
        // if self.data.borrow_mut().len() <= handle.index {
        //     return None
        // }
        // else {
        //      match &self.data.borrow_mut()[handle.index].is_none() {
        //          true => return None,
        //          false => return self.data.borrow_mut()[handle.index].as_mut()
        //      }
        // }
    }

    pub fn fill_up(&mut self, length : usize) {
        for _ in 0..length {
            self.data.push(RefCell::new(None));
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
    use std::{slice::SliceIndex, borrow::{Borrow, BorrowMut}, cell::RefMut};

    use itertools::izip;
    use scene::Scene;
    use scene_manager::SceneManager;

    use crate::ecs::{EntityHandle, scene_manager, Component, scene, HealthComponent};

    use super::{ComponentStorage, StorageEntry};

    // #[test]
    // fn basic_component_insertion() {
    //     let mut components = ComponentStorage::<u64>::new();

    //     let number: u64 = 10;
    //     let handle = EntityHandle::new(0, 0);

    //     components.set(handle, number);

    //     assert_eq!(Some(10), components.get(handle.index));

    //     components.set(handle, 20);
    //     assert_eq!(Some(20), components.get(handle.index));

    //     let second_handle = EntityHandle::new(0, 1);
    //     components.set(second_handle, 30);
    //     assert_eq!(None, components.get(handle.index));
    //     assert_eq!(Some(30), components.get(second_handle.index));
    // }

    // #[test]
    // fn mutable_component_test() {
    //     let mut components = ComponentStorage::<String>::new();

    //     let first = EntityHandle::new(0, 0);
    //     let second = EntityHandle::new(1, 1);

    //     components.set(first, String::from("TEST STRING"));
    //     assert_eq!(components.get(first.index), Some(String::from("TEST STRING")));

    //     let new_string = components.get(first.index).unwrap().to_owned() + &String::from(" WORKS");
    //     components.set(first, new_string.to_string());
    //     assert_eq!(components.get(first.index), Some(String::from("TEST STRING WORKS")))
    // }

    #[derive(Debug)]
    struct Health(u32);
    #[derive(Debug)]
    struct Shield(i32);
    #[derive(Debug)]
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

        // let target_scene = scene_manager.get_scene(scene).unwrap();
        // let first_storage = target_scene.get_component_storage::<Health>().data.borrow();
        // let mut second_storage = target_scene.get_component_storage::<Shield>().data.borrow_mut();
        // let mut third_storage = target_scene.get_component_storage::<Name>().data.borrow_mut();

        // // Works
        // // for (first, second) in first_storage.iter().zip(second_storage.iter()) {
        // //     println!("{} {}", first.as_ref().unwrap().0.to_string(), second.as_ref().unwrap().0.to_string());
        // // }

        // // Works
        // // for (first, second, third) in izip!(first_storage.iter(), second_storage.iter(), third_storage.iter()) {
        // //     println!("Component: {} {} {}", first.as_ref().unwrap().0.to_string(), second.as_ref().unwrap().0.to_string(), third.as_ref().unwrap().0);
        // // }

        // for (first, second, third) in izip!(first_storage.iter(), second_storage.iter_mut(), third_storage.iter_mut()) {
        //     println!("Component: {} {} {}", first.as_ref().unwrap().0.to_string(), second.as_mut().unwrap().0.to_string(), third.as_mut().unwrap().0);
        //     second.as_mut().unwrap().0 = second.as_mut().unwrap().0 * 3 + first.as_ref().unwrap().0 as i32;
        //     for (item) in first_storage.iter() {
        //         println!("Once again: Health = {}", item.as_ref().unwrap().0.to_string());
        //     }
        // }

        // for (first, second, third) in izip!(first_storage.iter(), second_storage.iter_mut(), third_storage.iter_mut()) {
        //     println!("Component: {} {} {}", first.as_ref().unwrap().0.to_string(), second.as_mut().unwrap().0.to_string(), third.as_mut().unwrap().0);
        // }

        let target_scene = scene_manager.get_scene(scene).unwrap();
        let first_storage = target_scene.get_component_storage::<Health>();
        let second_storage = target_scene.get_component_storage::<Shield>();
        let third_storage = target_scene.get_component_storage::<Name>();
        
        println!("\nBefore taking damage: \n");

        for (first, second, third) in izip!(first_storage.data.iter(), second_storage.data.iter(), third_storage.data.iter()) {
            println!("My name is {}, and my stats are: Health {} Shield {}", 
                    third.borrow().as_ref().unwrap().0,
                    first.borrow().as_ref().unwrap().0.to_string(), second.borrow_mut().as_mut().unwrap().0.to_string());
        }

        let damage = 8;
        println!("\nLet's deal some damage!");


        for(hp, shield, name) in izip!(first_storage.data.iter(), second_storage.data.iter(), third_storage.data.iter()) {
            println!("{}", hp.borrow_mut().as_ref().unwrap().0);
            let mut x = name.borrow_mut();
            let y = x.as_mut().unwrap();
            // name.borrow_mut().as_mut().unwrap().0 += &String::from("dsdsd");
            println!("{}", y.0);
            // shield.borrow_mut().as_mut().unwrap().0 -= 8;
            // println!("{}", x.0);
            // if shield.borrow_mut().as_mut().unwrap().0 < 0 {
            //     println!("As a result, {} lost shield, and it's value is now {}!", name.borrow().as_ref().unwrap().0,
            //     shield.borrow_mut().as_mut().unwrap().0);
            //     shield.borrow_mut().as_mut().unwrap().0 -= damage;
            //     hp.borrow_mut().as_mut().unwrap().0 -= 2;
            // }
        }

        println!("\nAfter taking damage: \n");

        for (first, second, third) in izip!(first_storage.data.iter(), second_storage.data.iter(), third_storage.data.iter()) {
            println!("My name is {}, and my stats are: Health {} Shield {}", 
                    third.borrow().as_ref().unwrap().0,
                    first.borrow().as_ref().unwrap().0.to_string(),
                    second.borrow_mut().as_mut().unwrap().0.to_string());
        }

        // for(hp, shield, name) in izip!(first_storage.data.iter(), second_storage.data.iter(), third_storage.data.iter()) {
        //     let mut x = name.borrow_mut().as_mut().unwrap();
        //     shield.borrow_mut().as_mut().unwrap().0 -= 8;
        //     println!("{}", x.0);
        //     if shield.borrow_mut().as_mut().unwrap().0 < 0 {
        //         println!("As a result, {} lost shield, and it's value is now {}!", name.borrow().as_ref().unwrap().0,
        //         shield.borrow_mut().as_mut().unwrap().0);
        //         shield.borrow_mut().as_mut().unwrap().0 -= damage;
        //         hp.borrow_mut().as_mut().unwrap().0 -= 2;
        //     }
        // }

    }
}