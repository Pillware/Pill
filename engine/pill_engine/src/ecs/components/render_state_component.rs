use crate::ecs::{GlobalComponent, GlobalComponentStorage};
use crate::PillTypeMapKey;

// Keeps renderer/bootstrap state; minimal for now
pub struct RenderStateComponent {
    pub boot_done: bool,
    pub egui_client: Option<std::sync::Arc<crate::ecs::EguiClient>>,
}

impl RenderStateComponent {
    pub fn new() -> Self {
        Self { boot_done: false, egui_client: None }
    }
}

impl PillTypeMapKey for RenderStateComponent {
    type Storage = GlobalComponentStorage<RenderStateComponent>;
}

impl GlobalComponent for RenderStateComponent {}
