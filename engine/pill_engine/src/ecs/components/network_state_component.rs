use crate::{
    ecs::{ Component, ComponentStorage, TransformComponent },
};

use pill_core::{ PillTypeMap, PillTypeMapKey };

use serde::{Serialize, Deserialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum NetworkEntityState {
    Spawn,
    Despawn,
    Alive,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NetworkStateComponent{
    pub owner_id: u64, // client id
    pub state: NetworkEntityState,
    pub network_entity_id: u64, // unique entity id in the network
    pub transform: Option<TransformComponent>,
    pub entity_type: String, // type of entity
    // TODO: add more components (Health etc.)
}

impl Component for NetworkStateComponent {}
impl PillTypeMapKey for NetworkStateComponent {
    type Storage = ComponentStorage<NetworkStateComponent>;
}

