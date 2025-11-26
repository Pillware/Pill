#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod renderer;
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

pub use render_queue::{
    RenderQueueItem,
    RenderQueueKeyFields,
    RenderQueueKey,
    compose_render_queue_key,
    decompose_render_queue_key,
    RENDER_QUEUE_KEY_ORDER,
};
