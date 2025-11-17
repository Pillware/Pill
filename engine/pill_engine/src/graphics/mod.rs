#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod render_queue;
mod renderer;

// --- Use ---

pub use pill_core::{
    RendererBufferTag, RendererCameraTag, RendererMaterialTag, RendererMeshTag,
    RendererPipelineTag, RendererPipelineV2Tag, RendererTextureTag,
};
pub use renderer::{render_with_factory, WorldView, WorldViewFactory};
pub use renderer::{
    BufferDesc, MaterialDesc, PillRenderer, PipelineV2, PipelineV2Desc, Renderer,
    RendererBufferHandle, RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle,
    RendererPipelineHandle, RendererPipelineV2Handle, RendererTargetDesc, RendererTextureHandle,
    ShaderDesc,
};
pub use renderer::{Pass, WorldQuery};

pub use render_queue::{
    compose_render_queue_key, decompose_render_queue_key, RenderQueueItem, RenderQueueKey,
    RenderQueueKeyFields, RENDER_QUEUE_KEY_ORDER,
};
pub use render_queue::{RenderQuery, RenderQueueFactory};

// Passes owned by engine
pub mod pass_logo;
pub use pass_logo::PassLogo;
pub mod pass_overlay_uv;
pub use pass_overlay_uv::PassOverlayUV;
pub mod pass_overlay_depth;
pub use pass_overlay_depth::PassOverlayDepth;
pub mod pass_linearize_depth;
pub use pass_linearize_depth::PassLinearizeDepth;
pub mod pass_compose;
pub use pass_compose::PassCompose;
pub mod pass_scene;
pub use pass_scene::PassScene;
pub mod pass_egui;
pub use pass_egui::PassEgui;
pub mod pass_skybox_equirect;
pub use pass_skybox_equirect::PassSkyboxEquirect;
pub mod pass_ibl_diffuse_equirect;
pub use pass_ibl_diffuse_equirect::PassIblDiffuseEquirect;
pub mod pass_ibl_specular_equirect;
pub use pass_ibl_specular_equirect::PassIblSpecularEquirect;
