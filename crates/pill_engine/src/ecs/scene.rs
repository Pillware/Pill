use std::{cell::RefCell, any::TypeId};

use anyhow::{Result, Context, Error};
use log::{debug, info};

use pill_core::{EngineError, get_type_name, PillSlotMap};
use typemap_rev::TypeMap;
use crate::ecs::*;

// --- Scene ---

pub struct Scene {

    // General
    pub name: String,

    // ECS
    pub(crate) entity_counter: usize,
    pub(crate) entities: pill_core::PillSlotMap<EntityHandle, Entity>,
    pub(crate) components: ComponentMap,
    pub(crate) allocator: Allocator,
    pub(crate) bitmask_controller: BitmaskController,

    //pub(crate) active_camera_entity_handle: Option<EntityHandle>,
}

impl Scene {
    pub fn new(name: String) -> Self {  
        return Self { 
            name,
            entity_counter: 0,
            entities: pill_core::PillSlotMap::<EntityHandle, Entity>::with_key(),
            components: ComponentMap::new(),
            allocator: Allocator::new(),
            bitmask_controller: BitmaskController::new(),
            //active_camera_entity_handle: None,
        };
    }

    #[cfg(feature = "game")]
    pub fn get_counter(&mut self) -> &usize {
        &self.entity_counter
    }

    pub fn get_component_storage<T: Component<Storage = ComponentStorage::<T>>>(&self) -> Result<&ComponentStorage<T>> {
        self.components.get::<T>().ok_or(Error::new(EngineError::ComponentNotRegistered(get_type_name::<T>(), self.name.clone())))
    }

    pub fn get_component_storage_mut<T: Component<Storage = ComponentStorage::<T>>>(&mut self) -> Result<&mut ComponentStorage<T>> {
        self.components.get_mut::<T>().ok_or(Error::new(EngineError::ComponentNotRegistered(get_type_name::<T>(), self.name.clone())))
    }

    // pub fn get_active_camera_entity_handle(&self) -> Result<EntityHandle> {
    //     match self.active_camera_entity_handle {
    //         Some(value) => Ok(value),
    //         None => Err(Error::new(EngineError::NoActiveCamera)),
    //     }
    // }

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
        self.get_component_storage::<T>().unwrap().data.iter()
    }

    pub fn get_two_component_storages<T: Component<Storage = ComponentStorage::<T>>, U: Component<Storage = ComponentStorage::<U>>>(&self) -> 
                                                                                                std::iter::Zip<
                                                                                                std::slice::Iter<'_, RefCell<Option<T>>>, 
                                                                                                std::slice::Iter<'_, RefCell<Option<U>>>> {
        self.get_component_storage::<T>().unwrap().data.iter()
            .zip(self.get_component_storage::<U>().unwrap().data.iter())
    }

    pub fn get_three_component_storages<T: Component<Storage = ComponentStorage::<T>>, U: Component<Storage = ComponentStorage::<U>>, W: Component<Storage = ComponentStorage::<W>>>(&self) -> 
                                                                                                std::iter::Zip<std::iter::Zip<
                                                                                                std::slice::Iter<'_, RefCell<Option<T>>>, 
                                                                                                std::slice::Iter<'_, RefCell<Option<U>>>>, 
                                                                                                std::slice::Iter<'_, RefCell<Option<W>>>> {

        self.get_component_storage::<T>().unwrap().data.iter()
            .zip(self.get_component_storage::<U>().unwrap().data.iter())
            .zip(self.get_component_storage::<W>().unwrap().data.iter())
    }

    pub fn get_four_component_storages<T: Component<Storage = ComponentStorage::<T>>, U: Component<Storage = ComponentStorage::<U>>, W: Component<Storage = ComponentStorage::<W>>, Y: Component<Storage = ComponentStorage::<Y>>>(&self) -> 
                                                                                                std::iter::Zip<std::iter::Zip<std::iter::Zip<
                                                                                                std::slice::Iter<'_, RefCell<Option<T>>>, 
                                                                                                std::slice::Iter<'_, RefCell<Option<U>>>>, 
                                                                                                std::slice::Iter<'_, RefCell<Option<W>>>>,
                                                                                                std::slice::Iter<'_, RefCell<Option<Y>>>> {
        self.get_component_storage::<T>().unwrap().data.iter()
            .zip(self.get_component_storage::<U>().unwrap().data.iter())
            .zip(self.get_component_storage::<W>().unwrap().data.iter())
            .zip(self.get_component_storage::<Y>().unwrap().data.iter())
    }
    
}
