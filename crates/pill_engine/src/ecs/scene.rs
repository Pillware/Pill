
use crate::{
    ecs::{ Entity, ComponentStorage, Component, EntityHandle }
};

use pill_core::{ EngineError, PillTypeMap, PillTypeMapKey, PillSlotMap, get_type_name};

use anyhow::{Result, Context, Error};
use std::{ cell::RefCell, any::TypeId, slice::Iter, iter::Zip };
use log::{debug, info};

use super::BitmaskController;

// --- Scene ---

pub struct Scene {
    pub(crate) name: String,
    pub(crate) entity_counter: usize,
    pub(crate) entities: PillSlotMap<EntityHandle, Entity>,
    pub(crate) components: PillTypeMap,
    pub(crate) bitmask_controller: BitmaskController,  
}

impl Scene {
    pub fn new(name: String) -> Self {  
        return Self { 
            name,
            entity_counter: 0,
            entities: PillSlotMap::<EntityHandle, Entity>::with_key(),
            components: PillTypeMap::new(),
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

    pub fn get_bitmask_controller(&self) -> &BitmaskController {
        &self.bitmask_controller
    }

    pub fn get_bitmask_controller_mut(&mut self) -> &mut BitmaskController {
        &mut self.bitmask_controller
    }

    pub fn get_one_component_storage<A>(&self) -> Iter<'_, RefCell<Option<A>>>
        where A: Component<Storage = ComponentStorage::<A>>
    {
        self.get_component_storage::<A>().unwrap().data.iter()
    }

    pub fn get_two_component_storages<A, B>(&self) -> Zip<
                                                      Iter<'_, RefCell<Option<A>>>, 
                                                      Iter<'_, RefCell<Option<B>>>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>   
    {
        self.get_component_storage::<A>().unwrap().data.iter()
            .zip(self.get_component_storage::<B>().unwrap().data.iter())
    }

    pub fn get_three_component_storages<A, B, C>(&self) -> Zip<std::iter::Zip<
                                                           Iter<'_, RefCell<Option<A>>>, 
                                                           Iter<'_, RefCell<Option<B>>>>, 
                                                           Iter<'_, RefCell<Option<C>>>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>,
        C: Component<Storage = ComponentStorage::<C>>                                        
    {

        self.get_component_storage::<A>().unwrap().data.iter()
            .zip(self.get_component_storage::<B>().unwrap().data.iter())
            .zip(self.get_component_storage::<C>().unwrap().data.iter())
    }

    pub fn get_four_component_storages<A, B, C, D>(&self) -> Zip<std::iter::Zip<std::iter::Zip<
                                                             Iter<'_, RefCell<Option<A>>>, 
                                                             Iter<'_, RefCell<Option<B>>>>, 
                                                             Iter<'_, RefCell<Option<C>>>>,
                                                             Iter<'_, RefCell<Option<D>>>> 
        where 
        A: Component<Storage = ComponentStorage::<A>>,
        B: Component<Storage = ComponentStorage::<B>>,
        C: Component<Storage = ComponentStorage::<C>>,
        D: Component<Storage = ComponentStorage::<D>>                                          
    {
        self.get_component_storage::<A>().unwrap().data.iter()
            .zip(self.get_component_storage::<B>().unwrap().data.iter())
            .zip(self.get_component_storage::<C>().unwrap().data.iter())
            .zip(self.get_component_storage::<D>().unwrap().data.iter())
    }
}
