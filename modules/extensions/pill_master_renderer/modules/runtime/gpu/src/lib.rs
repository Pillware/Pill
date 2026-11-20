//! The wgpu renderer: the GPU work that turns the world's renderer data into
//! frames.
//!
//! # Responsibilities
//!
//! - Own the GPU half: the device and surface lifecycle ([`Renderer`]), the
//!   [`resources`] the renderer keeps, the [`drawers`], the [`render_queue`]
//!   keys that order a frame's draws, and the [`ui`] painter that draws the
//!   frame's `pill_ui_frame` lists.
//! - Fill each frame from the world ([`frame`], the `rendering` system).
//! - Install the renderer on an engine through [`register`], which adds that
//!   system. The components and assets are not registered here:
//!   `pill_master_renderer_data` registers them itself.
//!
//! # Design
//!
//! The data a renderer draws with - assets, components, the default pipelines
//! and their WGSL - lives in `pill_master_renderer_data`; the contract between
//! host and renderer - the frame, the backend trait, the camera - lives in
//! `pill_renderer_api`. Neither links wgpu, and both are re-exported here.
//! This crate is the only renderer crate that links wgpu, which is what lets a
//! development host load and reload it as a module while everything around it
//! compiles against data only.

/// The mesh drawer: batches the instances a frame queues and records the
/// instanced draws they become.
///
/// Crate-private: `renderer.rs` is its only consumer, and other crates reach
/// the renderer through its extension ABI, not through these helpers.
pub(crate) mod drawers;

/// Renderer failures, re-exported from the contract crate, and the capture
/// that turns wgpu's errors into them.
pub mod error;

/// The system that turns the world into the frame the renderer draws.
pub mod frame;

/// The per-instance transform the drawer uploads for the vertex shader.
pub mod instance;

/// Holds the frame's chain as GPU objects and the plan each frame is recorded
/// from.
mod chain;

/// The asset preview path: one small off-screen render, read back as pixels.
mod preview;

/// Debug drawing, gizmos, selection outlines, picking and the scene grid,
/// drawn over each surface.
mod overlay;

/// The query sets and readbacks behind GPU profiling measurements.
pub mod profiler;

/// Mapping a GPU buffer back to the CPU without stalling a frame.
mod readback;

/// One queued draw per frame instance, and the sort key that puts draws
/// sharing a shader, material and mesh next to each other.
pub mod render_queue;

/// The wgpu renderer: the surfaces, the GPU caches and the frame submission.
pub mod renderer;

/// The GPU objects that mirror the asset store, refreshed one asset type at a
/// time.
mod rendering_resources_manager;

/// One module per resource the renderer keeps, re-exported flat so callers
/// name them without the path.
pub mod resources;

/// A GPU buffer rewritten every surface, shared by the overlay and the UI
/// painter.
mod stream_buffer;

/// The window surface, its swapchain, and the lifecycle that keeps them live.
mod surface;

/// The UI painter: draws the frame's `pill_ui_frame` lists over each surface.
mod ui;

/// The C ABI a host drives this renderer through when it loads it as a module.
///
/// Public so the wrapper crate's expansion of `__pill_renderer_entry_points!`
/// can call into it; the exported symbols come from that macro.
#[cfg(not(target_arch = "wasm32"))]
pub mod module_entry;

// External crates
use pill_engine::{pill_module, Engine};

// The contract and the data crates are re-exported here - their items and
// their module names - so this crate's own paths (`crate::api`,
// `crate::assets`, `crate::components`, `crate::config`) resolve.
pub use pill_master_renderer_data::components::*;
pub use pill_master_renderer_data::{assets, components, config};
pub use pill_master_renderer_data::{
    AssetLoader, Material, MaterialBuilder, MaterialParameter, Mesh, MeshVertex, PassKind,
    PassTarget, PipelinePass, RenderPass, RenderingPipeline, Shader, ShaderBuilder,
    ShaderParameterSlot, ShaderParameterType, ShaderTextureSlot, Texture, TextureType,
};
pub use pill_renderer_api::api;
pub use pill_renderer_api::{
    FrameOutcome, HeadlessRenderer, PillRenderer, RawWindowData, RenderCapabilities, RenderMetrics,
};

// Current crate
pub use error::RendererError;
pub use frame::{rendering_system, RenderFrame, RenderInstance, ResolvedPass};
pub use instance::Instance;
pub use renderer::Renderer;
pub use resources::RenderingManager;

// =============================================================================
// Free Functions
// =============================================================================

/// The registration half: declares nothing itself and returns zero.
///
/// This is the half a reload repeats. The post-update `rendering` system needs
/// no call of its own: [`frame::rendering_system`] declares itself, so the
/// engine registers it under the module's own owner before this function runs,
/// and the next generation registers its own after the reload removed the
/// previous one.
///
/// Registers no data: `pill_master_renderer_data`'s own `register` declares the
/// components, asset types, resources and default pipeline before this module
/// loads, and doing it here would point the asset types' tables at this image,
/// which reloads most often.
pub fn register(_engine: &mut Engine) -> u32 {
    0
}

/// The artifact's one-time entry point: nothing is one-time here.
///
/// Everything the module owns is registration that [`register`] repeats on
/// every generation. This function exists because a host that links the crate
/// statically loads it once and calls this entry point alone, and because the
/// generated `host_module_*` wrapper carries it as `pill_module_init` by
/// expanding `__pill_module_entry_points!`.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

/// Starts building a renderer backend on a window, for a host that links this
/// crate statically - the shipping posture, where there is no module to load.
///
/// Returns a future because creating the device is asynchronous: a native
/// frontend blocks on it once, and a web frontend awaits it. A host that loads
/// this crate as a module reaches the same renderer through the blocking
/// `pill_renderer_attach` export instead.
///
/// # Errors
///
/// The future fails with a [`RendererError`] when building the renderer fails:
/// acquiring the surface or the adapter, creating the device, or creating an
/// object built after them - the surface configuration, the depth buffer, the
/// resource storage, the default material or the camera.
///
/// # Safety
///
/// `window` must name a live window that outlives the returned renderer.
pub unsafe fn attach(
    window: pill_renderer_api::RawWindowData,
    width: u32,
    height: u32,
) -> pill_renderer_api::AttachFuture {
    Box::pin(async move {
        // SAFETY: forwarded from this function's own contract, which covers
        // the future as much as the renderer it resolves to.
        let renderer = unsafe { Renderer::new_async(window, width, height) }.await?;
        Ok(Box::new(renderer) as Box<dyn PillRenderer>)
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::Engine;

    /// The artifact entry point registers the declared `rendering` system,
    /// and the reload half declares nothing, so a second registration cannot
    /// leave a second copy.
    #[test]
    fn registering_the_renderer_adds_the_rendering_system_once() {
        let mut engine = Engine::new();

        assert_eq!(
            pill_engine::component_registry::run_artifact_registration(&mut engine, register),
            0
        );
        assert_eq!(register(&mut engine), 0);

        let registered = engine
            .system_snapshots()
            .iter()
            .filter(|snapshot| snapshot.name == "rendering")
            .count();
        assert_eq!(registered, 1, "the rendering system is registered once");
    }
}
