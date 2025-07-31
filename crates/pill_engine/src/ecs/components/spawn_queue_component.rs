use crate::{
    ecs::{ GlobalComponent, GlobalComponentStorage },
};

use pill_core::{ PillTypeMap, PillTypeMapKey };

use pill_net::{ TrPacket };

/// Stores requests coming from NetSystem so that *game* system
/// can consume them and spawn entities
#[derive(Default)]
pub struct SpawnQueueComponent {
    pub requests: Vec<(u64, TrPacket)>,
}

impl PillTypeMapKey for SpawnQueueComponent {
    type Storage = GlobalComponentStorage<SpawnQueueComponent>;
}
impl GlobalComponent for SpawnQueueComponent {}

