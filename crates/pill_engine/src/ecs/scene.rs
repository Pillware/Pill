
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

    pub(crate) fn get_counter(&mut self) -> &usize {
        &self.entity_counter
    }

    pub(crate) fn get_component_storage<T>(&self) -> Result<&ComponentStorage<T>> 
        where T: Component<Storage = ComponentStorage::<T>>
    {
        self.components.get::<T>().ok_or(Error::new(EngineError::ComponentNotRegistered(get_type_name::<T>(), self.name.clone())))
    }

    pub(crate) fn get_component_storage_mut<T>(&mut self) -> Result<&mut ComponentStorage<T>> 
        where T: Component<Storage = ComponentStorage::<T>>
    {
        self.components.get_mut::<T>().ok_or(Error::new(EngineError::ComponentNotRegistered(get_type_name::<T>(), self.name.clone())))
    }

    pub(crate) fn get_bitmask_controller(&self) -> &BitmaskController {
        &self.bitmask_controller
    }

    pub(crate) fn get_bitmask_controller_mut(&mut self) -> &mut BitmaskController {
        &mut self.bitmask_controller
    }
}
