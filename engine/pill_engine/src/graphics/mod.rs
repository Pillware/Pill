#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod renderer;
#[cfg(feature = "headless")]
mod dummy_renderer;
mod render_queue;
mod egui;

// --- Use ---

pub use renderer::{
    Renderer,
    PillRenderer,
    RendererCameraHandle,
    RendererMaterialHandle,
    RendererMeshHandle,
    RendererTextureHandle,
    RendererShaderHandle,
};

#[cfg(feature = "headless")]
pub use self::dummy_renderer::DummyRenderer;
pub use egui::EguiUI;

pub use render_queue::{
    RenderQueueItem,
    RenderQueueKeyFields,
    RenderQueueKey,
    compose_render_queue_key,
    decompose_render_queue_key,
    RENDER_QUEUE_KEY_ORDER,
};
