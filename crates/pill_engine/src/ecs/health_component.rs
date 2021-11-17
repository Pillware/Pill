use std::default;

pub use crate::ecs::{Component, ComponentStorage};

pub struct HealthComponent {
    pub value: u32
}

impl Component for HealthComponent {
    type Storage = ComponentStorage<HealthComponent>;
}

impl Default for HealthComponent {
    fn default() -> Self {
        Self {
            value: 0
        }
    }
}