use crate::{
    ecs::{ Component, ComponentStorage },
};

use pill_core::{ PillTypeMap, PillTypeMapKey, Vector3f };

use serde::{ Serialize, Deserialize };

use cgmath::Zero;


// --- Builder ---

pub struct TransformComponentBuilder {
    component: TransformComponent,
}

impl TransformComponentBuilder {
    pub fn default() -> Self {
        Self {
            component: TransformComponent::new(),
        }
    }

    pub fn position(mut self, position: Vector3f) -> Self {
        self.component.position = position;
        self
    }

    pub fn rotation(mut self, rotation: Vector3f) -> Self {
        self.component.rotation = rotation;
        self
    }

    pub fn scale(mut self, scale: Vector3f) -> Self {
        self.component.scale = scale;
        self
    }

    pub fn build(self) -> TransformComponent {
        self.component
    }
}

// --- Transform Component ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformComponent {
    pub position: Vector3f,
    pub rotation: Vector3f,
    pub scale: Vector3f,
    pub net_dirty: bool,
}

impl TransformComponent {
    pub fn builder() -> TransformComponentBuilder {
        TransformComponentBuilder::default()
    }

    pub fn new() -> Self {
        Self {
            position: Vector3f::zero(),
            rotation: Vector3f::zero(),
            scale: Vector3f::new(1.0, 1.0, 1.0),
            net_dirty: false,
        }
    }
}

impl Default for TransformComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl PillTypeMapKey for TransformComponent {
    type Storage = ComponentStorage<TransformComponent>;
}

impl Component for TransformComponent {

}
