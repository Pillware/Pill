use super::{EntityHandle, SceneManager, SceneHandle, Component, ComponentStorage};
use anyhow::{Result, Error};

pub struct EntityBuilder<'a> {
    pub entity: EntityHandle,
    pub scene_manager: &'a mut SceneManager,
    pub scene_handle: SceneHandle
}

impl<'a> EntityBuilder<'a> {

    pub fn with_component<T: Component<Storage = ComponentStorage::<T>>>(self, component: T) -> Self {
        {
        self.scene_manager.add_component_to_entity(self.scene_handle.clone(), self.entity.clone(), component).unwrap();
        }
        self
    }

    pub fn build(self) -> EntityHandle {
        self.entity
    }
}   