use pill_core::PillTypeMapKey;
use uuid::Uuid;

use crate::{Component, ComponentStorage};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct UuidComponent {
    pub uuid: u128,
}

impl Default for UuidComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl UuidComponent {
    pub fn new() -> Self {
        Self {
            uuid: Uuid::new_v4().as_u128(),
        }
    }
}

impl PillTypeMapKey for UuidComponent {
    type Storage = ComponentStorage<UuidComponent>;
}

impl Component for UuidComponent {}
