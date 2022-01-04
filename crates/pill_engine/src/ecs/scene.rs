
use crate::{
    ecs::{ Entity, ComponentStorage, Component, EntityHandle }
};

use pill_core::{ EngineError, PillTypeMap, PillTypeMapKey, PillSlotMap, get_type_name};

use anyhow::{Result, Context, Error};
use std::{ cell::RefCell, any::TypeId };
use log::{debug, info};

use super::BitmaskController;

// --- Scene ---

pub struct Scene {
    pub name: String,
    pub(crate) entity_counter: usize,
    pub(crate) entities: PillSlotMap<EntityHandle, Entity>,
    pub(crate) components: PillTypeMap,
    // pub(crate) allocator: Allocator,
    pub(crate) bitmask_controller: BitmaskController,  
}

impl Scene {
    pub fn new(name: String) -> Self {  
        return Self { 
            name,
            entity_counter: 0,
            entities: PillSlotMap::<EntityHandle, Entity>::with_key(),
            components: PillTypeMap::new(),
            // allocator: Allocator::new(),
            bitmask_controller: BitmaskController::new(),
        };
    }

    #[cfg(feature = "game")]
    pub fn get_counter(&mut self) -> &usize {
        &self.entity_counter
    }

    pub fn get_component_storage<T>(&self) -> Result<&ComponentStorage<T>> 
        where T: Component<Storage = ComponentStorage::<T>>
    {
        self.components.get::<T>().ok_or(Error::new(EngineError::ComponentNotRegistered(get_type_name::<T>(), self.name.clone())))
    }

    pub fn get_component_storage_mut<T>(&mut self) -> Result<&mut ComponentStorage<T>> 
        where T: Component<Storage = ComponentStorage::<T>>
    {
        self.components.get_mut::<T>().ok_or(Error::new(EngineError::ComponentNotRegistered(get_type_name::<T>(), self.name.clone())))
    }

    // pub fn get_allocator(&self) -> &Allocator {
    //     &self.allocator
    // }

    // pub fn get_allocator_mut(&mut self) -> &mut Allocator {
    //     &mut self.allocator
    // }

    pub fn get_bitmask_controller(&self) -> &BitmaskController {
        &self.bitmask_controller
    }

    pub fn get_bitmask_controller_mut(&mut self) -> &mut BitmaskController {
        &mut self.bitmask_controller
    }








    // pub fn get_component_storage_mut_with_count<T>(&mut self) -> (&mut ComponentStorage<T>, &usize) 
    //     where T: Component<Storage = ComponentStorage::<T>>
    // {
    //     (self.components.get_mut::<T>().unwrap(), self.allocator.get_max_index())
    // }

    pub fn get_one_component_storage<T>(&self) -> std::slice::Iter<'_, RefCell<Option<T>>>
        where T: Component<Storage = ComponentStorage::<T>>
    {
        self.get_component_storage::<T>().unwrap().data.iter()
    }

    pub fn get_two_component_storages<T, U>(&self) -> std::iter::Zip<
                                                      std::slice::Iter<'_, RefCell<Option<T>>>, 
                                                      std::slice::Iter<'_, RefCell<Option<U>>>> 
        where 
        T: Component<Storage = ComponentStorage::<T>>,
        U: Component<Storage = ComponentStorage::<U>>   
    {
        self.get_component_storage::<T>().unwrap().data.iter()
            .zip(self.get_component_storage::<U>().unwrap().data.iter())
    }

    pub fn get_three_component_storages<T, U, W>(&self) ->  std::iter::Zip<std::iter::Zip<
                                                            std::slice::Iter<'_, RefCell<Option<T>>>, 
                                                            std::slice::Iter<'_, RefCell<Option<U>>>>, 
                                                            std::slice::Iter<'_, RefCell<Option<W>>>> 
        where 
        T: Component<Storage = ComponentStorage::<T>>,
        U: Component<Storage = ComponentStorage::<U>>,
        W: Component<Storage = ComponentStorage::<W>>                                        
    {

        self.get_component_storage::<T>().unwrap().data.iter()
            .zip(self.get_component_storage::<U>().unwrap().data.iter())
            .zip(self.get_component_storage::<W>().unwrap().data.iter())
    }

    // pub fn get_four_component_storages<T, U, W, Z>(&self) -> std::iter::Zip<std::iter::Zip<std::iter::Zip<
    //                                                          std::slice::Iter<'_, RefCell<Option<T>>>, 
    //                                                          std::slice::Iter<'_, RefCell<Option<U>>>>, 
    //                                                          std::slice::Iter<'_, RefCell<Option<W>>>>,
    //                                                          std::slice::Iter<'_, RefCell<Option<Y>>>> 
    //     where 
    //     T: Component<Storage = ComponentStorage::<T>>,
    //     U: Component<Storage = ComponentStorage::<U>>,
    //     W: Component<Storage = ComponentStorage::<W>>,
    //     Y: Component<Storage = ComponentStorage::<Y>>                                          
    // {
    //     self.get_component_storage::<T>().unwrap().data.iter()
    //         .zip(self.get_component_storage::<U>().unwrap().data.iter())
    //         .zip(self.get_component_storage::<W>().unwrap().data.iter())
    //         .zip(self.get_component_storage::<Y>().unwrap().data.iter())
    // }
}
