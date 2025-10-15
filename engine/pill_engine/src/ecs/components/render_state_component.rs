use crate::ecs::{GlobalComponent, GlobalComponentStorage};
use crate::PillTypeMapKey;

// Keeps renderer/bootstrap state; minimal for now
pub struct RenderStateComponent {
    pub boot_done: bool,
}

impl RenderStateComponent {
    pub fn new() -> Self {
        Self { boot_done: false }
    }
}

impl PillTypeMapKey for RenderStateComponent {
    type Storage = GlobalComponentStorage<RenderStateComponent>;
}

impl GlobalComponent for RenderStateComponent {}
