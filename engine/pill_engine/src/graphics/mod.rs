#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

#[cfg(feature = "headless")]
mod dummy_renderer;
mod fullscreen;
#[cfg(feature = "post_fx")]
mod pass_bloom;
#[cfg(feature = "post_fx")]
mod pass_depth_of_field;
#[cfg(feature = "ui")]
mod pass_egui;
#[cfg(feature = "post_fx")]
mod pass_fxaa;
#[cfg(feature = "post_fx")]
mod pass_lens;
#[cfg(feature = "post_fx")]
mod pass_motion_blur;
mod pass_pbr_opaque;
#[cfg(feature = "post_fx")]
mod pass_ssao;
mod pass_tonemap;
#[cfg(feature = "post_fx")]
mod projection;
mod render_queue;
mod renderer;
mod scene_uniform;

// --- Use ---

pub use renderer::{
    camera_forward, camera_view_projection, BufferDesc, FrameTarget, Pass, PassTextureHandle,
    PillRenderer, Pipeline, PipelineDesc, Renderer, RendererMaterialHandle, RendererMeshHandle,
    RendererTargetDesc, RendererTextureHandle, ShaderDesc, TargetSize, TextureData, WorldQuery,
};

#[cfg(feature = "headless")]
pub use self::dummy_renderer::DummyRenderer;
#[cfg(feature = "post_fx")]
pub use pass_bloom::PassBloom;
#[cfg(feature = "post_fx")]
pub use pass_depth_of_field::PassDepthOfField;
#[cfg(feature = "ui")]
pub use pass_egui::PassEgui;
#[cfg(feature = "post_fx")]
pub use pass_fxaa::PassFxaa;
#[cfg(feature = "post_fx")]
pub use pass_lens::PassLens;
#[cfg(feature = "post_fx")]
pub use pass_motion_blur::PassMotionBlur;
pub use pass_pbr_opaque::PassPBROpaque;
#[cfg(feature = "post_fx")]
pub use pass_pbr_opaque::PassPBRTranslucent;
#[cfg(feature = "post_fx")]
pub use pass_ssao::PassSsao;
pub use pass_tonemap::PassTonemap;
pub(crate) use scene_uniform::SceneUniform;

pub use render_queue::{
    compose_render_queue_key, decompose_render_queue_key, RenderQueueKey, RenderQueueKeyFields,
    RENDER_QUEUE_KEY_MESH_INDEX,
};
