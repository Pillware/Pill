#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

#[cfg(feature = "headless")]
mod dummy_renderer;
#[cfg(feature = "ui")]
mod pass_egui;
mod pass_pbr_opaque;
mod pass_tonemap;
mod render_queue;
mod renderer;

// --- Use ---

pub use renderer::{
    camera_forward, camera_view_projection, BufferDesc, FrameTarget, Pass, PassTextureHandle,
    PillRenderer, Pipeline, PipelineDesc, Renderer, RendererMaterialHandle, RendererMeshHandle,
    RendererTargetDesc, RendererTextureHandle, ShaderDesc, TargetSize, TextureData, WorldQuery,
};

#[cfg(feature = "headless")]
pub use self::dummy_renderer::DummyRenderer;
#[cfg(feature = "ui")]
pub use pass_egui::PassEgui;
pub use pass_pbr_opaque::PassPBROpaque;
pub use pass_tonemap::PassTonemap;

pub use render_queue::{
    compose_render_queue_key, decompose_render_queue_key, RenderQueueKey, RenderQueueKeyFields,
    RENDER_QUEUE_KEY_MESH_INDEX,
};
