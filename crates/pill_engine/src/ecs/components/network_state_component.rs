#![cfg(feature = "net")]
use crate::{
    ecs::{ Component, ComponentStorage, TransformComponent },
};

use pill_core::{ PillTypeMap, PillTypeMapKey };

use serde::{Serialize, Deserialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct NetworkStateComponent{
    pub dirty: bool,
    pub transform: Option<TransformComponent>,
    // TODO: add more components (Health etc.)
}

impl Component for NetworkStateComponent {}
impl PillTypeMapKey for NetworkStateComponent {
    type Storage = ComponentStorage<NetworkStateComponent>;
}

