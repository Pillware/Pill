#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

#[cfg(feature = "headless")]
mod dummy_renderer;
#[cfg(feature = "debug_ui")]
mod egui;
mod render_queue;
mod renderer;

// --- Use ---

pub use renderer::{
    PillRenderer, Renderer, RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle,
    RendererShaderHandle, RendererTextureHandle,
};

#[cfg(feature = "headless")]
pub use self::dummy_renderer::DummyRenderer;
#[cfg(feature = "debug_ui")]
pub use egui::EguiUI;

pub use render_queue::{
    compose_render_queue_key, decompose_render_queue_key, RenderQueueItem, RenderQueueKey,
    RenderQueueKeyFields, RENDER_QUEUE_KEY_ORDER,
};
