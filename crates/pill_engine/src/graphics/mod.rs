#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]
#![cfg(feature = "rendering")]

mod renderer;
mod render_queue;

// --- Use ---

pub use renderer::{
    Renderer,
    RendererError,
    PillRenderer,
    RendererCameraHandle,
    RendererMaterialHandle,
    RendererMeshHandle,
    RendererTextureHandle,
    RendererPipelineHandle,
};

pub use render_queue::{
    RenderQueueItem,
    RenderQueueKeyFields,
    RenderQueueKey,
    compose_render_queue_key,
    decompose_render_queue_key,
    RENDER_QUEUE_KEY_ORDER,
};
