use std::{default, io::Error, ops::Index, cell::{RefCell, RefMut}, borrow::Borrow, os::windows::prelude::OpenOptionsExt};


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
      
        let capacity = 100;
        let mut data = Vec::<RefCell<Option<T>>>::with_capacity(capacity);
        for _ in 0..capacity {
            data.push(RefCell::new(None));
        }

        Self { 
            data
        }
    }

    pub fn set(&mut self, handle: EntityHandle, comp: T) {
        unsafe
        {
            while self.data.len() <= (handle.get_data().index as usize) {
            self.data.push(RefCell::new(None))
        }
            self.data[handle.get_data().index as usize] = RefCell::new(Some(comp));
        }
    }

    pub fn delete(&mut self, entity_handle: EntityHandle) {
        unsafe {
            if self.data.len() > entity_handle.get_data().index as usize {
                self.data[entity_handle.get_data().index as usize].replace(None);
            }
        }
    }

    // pub fn get(&self, index: usize) -> Option<T> {
    //     return None;
    //     if self.data.borrow().len() <= index {
    //         return None
    //     }
    //     else {
    //          match &self.data.borrow()[index].is_none() {
    //              true => return None,
    //              false => return self.data.borrow()[index]
    //          }
    //     }
    //     if self.data[handle.index].generation == handle.generation {
    //         match &self.data[handle.index].component.is_none() {
    //             true => return None,
    //             false => return self.data[handle.index].component.as_ref()
    //         }
    //     }
    //     None
    // }
    
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

    // pub fn get_mut(&mut self, handle: EntityHandle) -> Option<&mut T> {
    //     return None;
    //     if self.data.borrow_mut().len() <= handle.index {
    //         return None
    //     }
    //     else {
    //          match &self.data.borrow_mut()[handle.index].is_none() {
    //              true => return None,
    //              false => return self.data.borrow_mut()[handle.index].as_mut()
    //          }
    //     }
    // }

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
    use std::{slice::SliceIndex, borrow::{Borrow, BorrowMut}, cell::{RefMut, RefCell}};

    use scene::Scene;
    use scene_manager::SceneManager;

    use crate::ecs::{EntityHandle, scene_manager, Component, scene};

    use super::{ComponentStorage, StorageEntry};

    #[derive(Debug)]
    struct Health(u32);
    #[derive(Debug)]
    struct Shield(i32);
    #[derive(Debug)]
    struct Name(String);

    impl Component for Health { type Storage = ComponentStorage<Self> ;}
    impl Component for Shield { type Storage = ComponentStorage<Self> ;}
    impl Component for Name { type Storage = ComponentStorage<Self> ;}

    // #[test]
    // unsafe fn basic_multiple_components_iteration_test() {
    //     // Basic scenario for iterating over entities
    //     // Two entities containing health, shield, and name components
    //     // We want to iterate over them to create system for dealing damage to them
    //     // Let's assume, that we want to have all three components shown
    //     // Name will be printed, and either shield and/or health will be reduced by some constant

    //     // Create scene manager
    //     let mut scene_manager = SceneManager::new();

    //     // Create scene
    //     let mut scene = scene_manager.create_scene("Default").unwrap();

    //     // Register components
    //     scene_manager.register_component::<Health>(scene)?;
    //     scene_manager.register_component::<Shield>(scene)?;
    //     scene_manager.register_component::<Name>(scene)?;

    //     // Create entities
    //     let first_entity = scene_manager.create_entity(scene).unwrap();
    //     let second_entity = scene_manager.create_entity(scene).unwrap();
    //     let third_entity = scene_manager.create_entity(scene).unwrap();

    //     // Add components to first entity
    //     scene_manager.add_component_to_entity(scene, first_entity, Health(15))?;
    //     scene_manager.add_component_to_entity(scene, first_entity, Shield(10))?;
    //     scene_manager.add_component_to_entity(scene, first_entity, Name(String::from("Frodo")))?;

    //     // Add components to second entity
    //     scene_manager.add_component_to_entity(scene, second_entity, Health(5))?;
    //     scene_manager.add_component_to_entity(scene, second_entity, Shield(5))?;
    //     scene_manager.add_component_to_entity(scene, second_entity, Name(String::from("Sam")))?;
        
    //     // Add components to third entity
    //     scene_manager.add_component_to_entity(scene, third_entity, Health(50))?;
    //     scene_manager.add_component_to_entity(scene, third_entity, Name(String::from("Gimli")))?;

    //     // Get components storages

    //     let target_scene = scene_manager.get_scene(scene).unwrap();
    //     let first_storage = target_scene.get_component_storage::<Health>()?;
    //     let second_storage = target_scene.get_component_storage::<Shield>()?;
    //     let third_storage = target_scene.get_component_storage::<Name>()?;
        
    //     let first_iter = target_scene.get_two_component_storages::<Name, Health>().next();

    // }
}