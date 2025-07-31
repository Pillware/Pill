use crate::{
    ecs::{ Component, ComponentStorage },
};

use pill_core::{ PillTypeMap, PillTypeMapKey };

use serde::{Serialize, Deserialize};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct OwnerComponent {
    pub client_id: u64, // 0 - server-side
}

impl Component for OwnerComponent {}
impl PillTypeMapKey for OwnerComponent {
    type Storage = ComponentStorage<OwnerComponent>;
}

