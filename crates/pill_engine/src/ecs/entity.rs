use crate::{
    engine::Engine,
    ecs::{ SceneManager, SceneHandle, Component, ComponentStorage }, 
};

use anyhow::{Result, Error};


pill_core::define_new_pill_slotmap_key! { 
    pub struct EntityHandle;
}

// --- Builder ---

pub struct EntityBuilder<'a> {
    pub engine: &'a mut Engine,
    pub entity_handle: EntityHandle,
    pub scene_handle: SceneHandle
}

impl<'a> EntityBuilder<'a> {
    pub fn with_component<T: Component<Storage = ComponentStorage::<T>>>(self, component: T) -> Self {
        self.engine.add_component_to_entity(self.scene_handle.clone(), self.entity_handle.clone(), component).unwrap();
        self
    }

    pub fn build(self) -> EntityHandle {
        self.entity_handle
    }
}  

// --- Entity ---

pub struct Entity {
    pub(crate) bitmask: u16,
    pub(crate) scene_handle: SceneHandle
}

impl Entity {
    pub fn new(scene_handle: SceneHandle) -> Self {
        Self {
            bitmask: 0b0000_0000_0000_0000,
            scene_handle,
        }
    }
}
