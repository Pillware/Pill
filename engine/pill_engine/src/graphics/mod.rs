#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

#[cfg(feature = "headless")]
mod dummy_renderer;
mod pass_background;
#[cfg(feature = "ui")]
mod pass_egui;
mod pass_mesh;
mod pass_pbr_opaque;
mod pass_tonemap;
mod render_queue;
mod renderer;

// --- Use ---

pub use renderer::{
    BufferDesc, Pass, PillRenderer, PipelineV2, PipelineV2Desc, Renderer, RendererCameraHandle,
    RendererMaterialHandle, RendererMeshHandle, RendererShaderHandle, RendererTargetDesc,
    RendererTextureHandle, ShaderDesc, WorldQuery,
};

#[cfg(feature = "headless")]
pub use self::dummy_renderer::DummyRenderer;
pub use pass_background::PassBackground;
#[cfg(feature = "ui")]
pub use pass_egui::PassEgui;
pub use pass_mesh::PassMesh;
pub use pass_pbr_opaque::PassPBROpaque;
pub use pass_tonemap::PassTonemap;

pub use render_queue::{
    compose_pbr_render_queue_key, compose_render_queue_key, decompose_render_queue_key,
    RenderQueueItem, RenderQueueKey, RenderQueueKeyFields, RENDER_QUEUE_KEY_ORDER,
};
