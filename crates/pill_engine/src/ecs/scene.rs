use anyhow::{Result, Context, Error};
use log::{debug, info};

use pill_core::{EngineError, get_type_name};
use typemap_rev::TypeMap;
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
    pub(crate) entities: Vec<Entity>,
    pub(crate) components: ComponentMap,
}

impl Scene {
    pub fn new(name: String) -> Self {  
        return Self { 
            name,
            entity_counter: 0,
            entities: Vec::<Entity>::new(),
            components: ComponentMap::new(),
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
}
