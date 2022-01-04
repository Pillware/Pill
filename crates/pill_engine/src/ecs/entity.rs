use crate::ecs::{ SceneManager, SceneHandle, Component, ComponentStorage };

use anyhow::{Result, Error};


pill_core::define_new_pill_slotmap_key! { 
    pub struct EntityHandle;
}

// --- Builder ---

pub struct EntityBuilder<'a> {
    pub entity: EntityHandle,
    pub scene_manager: &'a mut SceneManager,
    pub scene_handle: SceneHandle
}

impl<'a> EntityBuilder<'a> {
    pub fn with_component<T: Component<Storage = ComponentStorage::<T>>>(self, component: T) -> Self {
        self.scene_manager.add_component_to_entity(self.scene_handle.clone(), self.entity.clone(), component).unwrap();
        self
    }

    pub fn build(self) -> EntityHandle {
        self.entity
    }
}   

// --- Entity ---

#[derive(Debug)]
pub struct Entity {
    pub(crate) bitmask: u32
}

impl Entity {
    pub fn new(bitmask: u32) -> Self {
        Self {
            bitmask,
        }
    }
}

impl Default for Entity {
    fn default() -> Self {
        Self {
            bitmask: 0
        }
    }
}
