use crate::ecs::{Component, ComponentStorage};
use pill_core::PillTypeMapKey;

pub struct ScriptComponent {
    pub script_type: String,
    pub started: bool,
}

impl PillTypeMapKey for ScriptComponent {
    type Storage = ComponentStorage<ScriptComponent>;
}

impl Component for ScriptComponent {}
