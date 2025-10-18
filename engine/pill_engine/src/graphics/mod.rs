#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod render_queue;
mod renderer;

// --- Use ---

pub use renderer::{
    BufferDesc, PillRenderer, PipelineV2, PipelineV2Desc, Renderer, RendererBufferHandle,
    RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle, RendererPipelineHandle,
    RendererPipelineV2Handle, RendererTextureHandle, ShaderDesc,
};

pub use render_queue::{
    compose_render_queue_key, decompose_render_queue_key, RenderQueueItem, RenderQueueKey,
    RenderQueueKeyFields, RENDER_QUEUE_KEY_ORDER,
};
