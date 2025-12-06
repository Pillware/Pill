use crate::ecs::{GlobalComponent, GlobalComponentStorage};
use crate::PillTypeMapKey;

// Keeps renderer/bootstrap state; minimal for now
pub struct RenderStateComponent {
    pub boot_done: bool,
    pub egui_client: Option<std::sync::Arc<crate::ecs::EguiClient>>,

    // Vignette post-processing parameters
    pub vignette_intensity: f32,
    pub vignette_smoothness: f32,
    pub vignette_radius: f32,
}

impl RenderStateComponent {
    pub fn new() -> Self {
        Self {
            boot_done: false,
            egui_client: None,
            vignette_intensity: 0.85,
            vignette_smoothness: 0.23,
            vignette_radius: 1.15,
        }
    }
}

impl PillTypeMapKey for RenderStateComponent {
    type Storage = GlobalComponentStorage<RenderStateComponent>;
}

impl GlobalComponent for RenderStateComponent {}
