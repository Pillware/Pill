use typemap_rev::TypeMapKey;

pub use crate::ecs::{Component, ComponentStorage};

pub struct NameComponent {
    pub name: String,
}

impl Component for NameComponent {
    type Storage = ComponentStorage<NameComponent>;
}

impl Default for NameComponent {
    fn default() -> Self {
        Self {
            name: String::from("Default")
        }
    }
}