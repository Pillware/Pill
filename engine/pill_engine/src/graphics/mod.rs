#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod renderer;
mod render_queue;
mod egui;
mod postprocessing_effects;

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

pub use egui::EguiUI;

pub use render_queue::{
    RenderQueueItem,
    RenderQueueKeyFields,
    RenderQueueKey,
    compose_render_queue_key,
    decompose_render_queue_key,
    RENDER_QUEUE_KEY_ORDER,
};

pub use postprocessing_effects::{
    PostprocessingEffect,
    ColorAdjustmentsPostprocessingEffect,
    register_color_adjustments_postprocessing_effect,
    VignettePostProcessingEffect,
    register_vignette_postprocessing_effect,
};