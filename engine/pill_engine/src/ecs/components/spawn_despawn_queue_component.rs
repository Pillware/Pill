#![cfg(feature = "net")]
use crate::{ecs::{ EntityHandle, GlobalComponent, GlobalComponentStorage }, TransformComponent};

use pill_core::{ PillTypeMap, PillTypeMapKey };


/// Stores requests coming from NetSystem so that *game* system
/// can consume them and spawn entities
#[derive(Default)]
pub struct SpawnDespawnQueueComponent {
    pub spawn: Vec<(u64, EntityHandle, TransformComponent)>, // TODO: do we need to store the client_id?
        pub despawn: Vec<(u64, EntityHandle)>,
        }

        impl PillTypeMapKey for SpawnDespawnQueueComponent {
            type Storage = GlobalComponentStorage<SpawnDespawnQueueComponent>;
            }
            impl GlobalComponent for SpawnDespawnQueueComponent {}


