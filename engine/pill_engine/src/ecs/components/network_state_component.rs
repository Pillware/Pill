use crate::{
    ecs::{ Component, ComponentStorage, TransformComponent },
};

use pill_core::{ PillTypeMap, PillTypeMapKey };

use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetEntityState {
    Spawn,
    Despawn,
    Alive,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkStateComponent{
    pub owner_id: u64, // client id
    pub state: NetEntityState,
    pub net_entity_id: u64, // unique entity id in the network
    pub transform: Option<TransformComponent>,
    // TODO: add more components (Health etc.)
}

impl Component for NetworkStateComponent {}
impl PillTypeMapKey for NetworkStateComponent {
    type Storage = ComponentStorage<NetworkStateComponent>;
}

