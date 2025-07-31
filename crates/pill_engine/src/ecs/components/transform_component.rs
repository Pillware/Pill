use crate::{
    ecs::{ Component, ComponentStorage },
};

use pill_core::{ PillTypeMap, PillTypeMapKey, Vector3f };

use pill_net::TrPacket;

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

pub struct TransformComponent {
    pub position: Vector3f,
    pub rotation: Vector3f,
    pub scale: Vector3f,
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
        }
    }
}

impl PillTypeMapKey for TransformComponent {
    type Storage = ComponentStorage<TransformComponent>;
}

impl Component for TransformComponent {

}

// --- Transform Packet ---
impl From<&TransformComponent> for TrPacket {
    fn from(t: &TransformComponent) -> Self {
        Self {
            pos: [t.position.x, t.position.y, t.position.z],
            rot: [t.rotation.x, t.rotation.y, t.rotation.z],
            scale: [t.scale.x, t.scale.y, t.scale.z],
        }
    }
}

impl From<&TrPacket> for TransformComponent {
    fn from(p: &TrPacket) -> Self {
        Self {
            position: Vector3f::new(p.pos[0], p.pos[1], p.pos[2]),
            rotation: Vector3f::new(p.rot[0], p.rot[1], p.rot[2]),
            scale: Vector3f::new(p.scale[0], p.scale[1], p.scale[2]),
        }
    }
}
