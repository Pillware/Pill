use std::{path::Iter};

use anyhow::{Result, Context, Error};
use itertools::{Zip, izip};
use log::{debug, info};
use std::cell::RefCell;
use pill_core::EngineError;
use crate::ecs::*;

// --------- SceneHandle

#[derive(Clone, Copy)]
pub struct SceneHandle {
    pub index: usize,
}

impl SceneHandle {
    pub fn new(index: usize) -> Self {
	    Self { 
            index,
        }
    }
}

// --------- Scene

pub struct Scene {

    // General
    pub name: String,

    // ECS
    pub(crate) entity_counter: usize,
    pub(crate) entities: Vec<EntityHandle>,
    pub(crate) components: ComponentMap,
    pub(crate) allocator: Allocator,
    pub(crate) bitmask_controller: BitmaskController
}

impl Scene {
    pub fn new(name: String) -> Self {  
        return Self { 
            name,
            entity_counter: 0,
            entities: Vec::<EntityHandle>::new(),
            components: ComponentMap::new(),
            allocator: Allocator::new(),
            bitmask_controller: BitmaskController::new()
        };
    }
    
    #[cfg(feature = "game")]
    pub fn get_counter(&self) -> &usize {
        &self.entity_counter
    }

    pub fn get_component_storage<T: Component<Storage = ComponentStorage::<T>>>(&self) -> &ComponentStorage<T> {
        self.components.get::<T>().take().unwrap()
    }

    pub fn get_component_storage_mut<T: Component<Storage = ComponentStorage::<T>>>(&mut self) -> &mut ComponentStorage<T> {
        self.components.get_mut::<T>().take().unwrap()
    }

    pub fn get_allocator(&self) -> &Allocator {
        &self.allocator
    }

    pub fn get_allocator_mut(&mut self) -> &mut Allocator {
        &mut self.allocator
    }

    pub fn get_bitmask_controller(&self) -> &BitmaskController {
        &self.bitmask_controller
    }

    pub fn get_bitmask_controller_mut(&mut self) -> &mut BitmaskController {
        &mut self.bitmask_controller
    }

    pub fn get_component_storage_mut_with_count<T: Component<Storage = ComponentStorage::<T>>>(&mut self) -> (&mut ComponentStorage<T>, &usize) {
        (self.components.get_mut::<T>().unwrap(), self.allocator.get_max_index())
    }

    pub fn get_one_component_storage<T: Component<Storage = ComponentStorage::<T>>>(&self) -> std::slice::Iter<'_, RefCell<Option<T>>> {
        self.get_component_storage::<T>().data.iter()
    }

    pub fn get_two_component_storages<T: Component<Storage = ComponentStorage::<T>>, U: Component<Storage = ComponentStorage::<U>>>(&self) -> 
                                                                                                std::iter::Zip<
                                                                                                std::slice::Iter<'_, RefCell<Option<T>>>, 
                                                                                                std::slice::Iter<'_, RefCell<Option<U>>>> {
        self.get_component_storage::<T>().data.iter()
            .zip(self.get_component_storage::<U>().data.iter())
    }

    pub fn get_three_component_storages<T: Component<Storage = ComponentStorage::<T>>, U: Component<Storage = ComponentStorage::<U>>, W: Component<Storage = ComponentStorage::<W>>>(&self) -> 
                                                                                                std::iter::Zip<std::iter::Zip<
                                                                                                std::slice::Iter<'_, RefCell<Option<T>>>, 
                                                                                                std::slice::Iter<'_, RefCell<Option<U>>>>, 
                                                                                                std::slice::Iter<'_, RefCell<Option<W>>>> {

        self.get_component_storage::<T>().data.iter()
            .zip(self.get_component_storage::<U>().data.iter())
            .zip(self.get_component_storage::<W>().data.iter())
    }

    pub fn get_four_component_storages<T: Component<Storage = ComponentStorage::<T>>, U: Component<Storage = ComponentStorage::<U>>, W: Component<Storage = ComponentStorage::<W>>, Y: Component<Storage = ComponentStorage::<Y>>>(&self) -> 
                                                                                                std::iter::Zip<std::iter::Zip<std::iter::Zip<
                                                                                                std::slice::Iter<'_, RefCell<Option<T>>>, 
                                                                                                std::slice::Iter<'_, RefCell<Option<U>>>>, 
                                                                                                std::slice::Iter<'_, RefCell<Option<W>>>>,
                                                                                                std::slice::Iter<'_, RefCell<Option<Y>>>> {
        self.get_component_storage::<T>().data.iter()
            .zip(self.get_component_storage::<U>().data.iter())
            .zip(self.get_component_storage::<W>().data.iter())
            .zip(self.get_component_storage::<Y>().data.iter())
    }
}
