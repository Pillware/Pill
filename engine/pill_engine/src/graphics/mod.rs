#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

#[cfg(feature = "headless")]
mod dummy_renderer;
#[cfg(feature = "debug_ui")]
mod egui;
mod render_queue;
mod renderer;

// --- Use ---

pub use renderer::{
    HardwareRayQueryCapabilities, PillRenderer, RayOpacityMode, RayVisibility,
    RenderCamera, RenderFrame, RenderInstance, RenderLight, RendererBackend,
    RendererCameraHandle, RendererCapabilities, RendererMaterialHandle,
    RendererMeshHandle, RendererShaderHandle, RendererTextureHandle,
};
// Renderer type alias (Box<dyn PillRenderer>) is unused —
// callers use Box<dyn PillRenderer> directly.
#[allow(unused_imports)]
pub use renderer::Renderer;

#[cfg(feature = "headless")]
pub use self::dummy_renderer::DummyRenderer;
#[cfg(feature = "debug_ui")]
pub use egui::EguiUI;

pub use render_queue::{
    compose_render_queue_key, decompose_render_queue_key, RenderQueueItem, RenderQueueKey,
    RenderQueueKeyFields, RENDER_QUEUE_KEY_ORDER,
};
