//! The CPU renderer: draws a frame's meshes into an RGB565 framebuffer at the
//! panel's resolution, with no graphics stack behind it.
//!
//! # Responsibilities
//!
//! - Implement the renderer contract ([`PillRenderer`]) on the processor
//!   ([`renderer::EmbeddedRenderer`]): resize, viewport, frame submission,
//!   asset invalidation, capabilities and statistics.
//! - Draw the frame's instances flat-shaded and textured, depth-tested, into
//!   packed RGB565 pixels ([`rasterizer::SceneRasterizer`]).
//! - Own the panel's framebuffer and its byte view ([`framebuffer`]).
//! - Convert the textures a frame samples once per content version
//!   ([`texture_cache`]).
//!
//! # Design
//!
//! This renderer fills the same role as `pill_renderer`, and both name the same
//! data crate (`pill_rendering`): the components, assets, materials and the
//! shared frame building in `pill_rendering_core` are theirs, and only the
//! drawing differs. It links no wgpu, no window and no build script, so it can
//! run on a device without a GPU.
//!
//! Everything the frame carries is plain data published by the scene's systems,
//! so this crate registers no component, resource or system of its own: it
//! reads a [`RenderFrame`] and writes pixels. A preview or a device frontend
//! drives it through the same [`PillRenderer`] trait the GPU backend
//! implements.
//!
//! The whole rasterizer is plain floats and integers over slice indices: no
//! allocation happens per frame after the buffers are sized, which is what the
//! smallest target - a device core without an FPU-heavy budget - needs.
//!
//! # Memory budget
//!
//! At the default 240x240 panel the renderer holds:
//!
//! - the framebuffer: `240 * 240 * 2` bytes = 112.5 KiB of packed RGB565,
//!   plus the same again while [`Framebuffer::as_rgb565_le`] fills the byte
//!   view a display driver reads;
//! - the depth buffer: `240 * 240 * 2` bytes = 112.5 KiB of 16-bit depths.
//!
//! That is 225 KiB for the two buffers, before any texture is cached. A cached
//! texture costs `width * height * 2` bytes, once per texture the frame
//! samples, and the byte view exists only while a driver is reading it.
//!
//! [`PillRenderer`]: pill_renderer_api::PillRenderer
//! [`RenderFrame`]: pill_renderer_api::RenderFrame
//! [`Framebuffer::as_rgb565_le`]: framebuffer::Framebuffer::as_rgb565_le

/// The RGB565 framebuffer and its colour packing.
pub mod framebuffer;

/// The debug lines and labels drawn over the scene.
pub mod debug_draw;

/// The system that turns the world into the frame the renderer draws.
pub mod frame;

/// The C ABI a host drives this renderer through when it loads it as a module.
pub mod module_entry;

/// The CPU triangle rasterizer.
pub mod rasterizer;

/// The embedded renderer itself: the contract implementation and the panel's
/// resolution.
pub mod renderer;

/// The RGB565 copies of the textures a frame samples.
pub mod texture_cache;

/// The frame's UI draw lists.
pub mod ui;

/// The scene components the frame system reads, re-exported from the renderer
/// contract and the data crate so a game imports them all from the renderer it
/// draws with.
pub mod components {
    pub use pill_engine::common_components::TransformComponent;
    pub use pill_renderer_api::components::CameraComponent;
    pub use pill_rendering::components::MeshRendererComponent;
}

// Re-exported so a caller - a preview, a device frontend, a test - names the
// renderer and its panel without the module path.
pub use framebuffer::{pack_rgb565, unpack_rgb565, Framebuffer};
pub use renderer::{EmbeddedRenderer, PanelSize, DEFAULT_PANEL_SIZE};
pub use texture_cache::{CachedTexture, TextureCache};
pub use ui::UiRasterizer;

// The contract and the data crate are re-exported here - their items and their
// module names - so this crate's own paths resolve and a host names the same
// types whichever renderer it loaded.
pub use pill_renderer_api::{
    FrameOutcome, PillRenderer, RawWindowData, RenderCapabilities, RenderFrame, RenderMetrics,
};

// External crates
use pill_engine::{pill_module, Engine};

// =============================================================================
// Free Functions
// =============================================================================

/// The registration half: declares the data crate's content roots and returns
/// zero.
///
/// This is the half a reload repeats. The post-update `rendering` system needs
/// no call of its own: [`frame::rendering_system`] declares itself, so the
/// engine registers it under the module's own owner before this function runs,
/// and the next generation registers its own after the reload removed the
/// previous one.
///
/// It registers no components or asset types, and installs no pipeline: the
/// data crate declares them before this module loads, and a processor renderer
/// draws whatever chain the project selected, through its own rasterizer. What
/// it declares is the materials the frame's materials come from.
pub fn register(_engine: &mut Engine) -> u32 {
    // The materials a frame draws with are the data crate's, so its root has
    // to be part of this renderer's content as well. A running process usually
    // has it already, from the data module's own registration; a test and a
    // statically linked build do not.
    pill_rendering::declare_crate_roots();
    0
}
/// The artifact's one-time entry point: nothing is one-time here.
///
/// Everything the module owns is registration that [`register`] repeats on
/// every generation, and the system [`frame::rendering_system`] declares.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

/// Starts building a renderer for a window, for a host that links this crate
/// statically.
///
/// A host that loads this crate as a module reaches the same renderer through
/// the blocking `pill_renderer_attach` export instead. The window is accepted
/// and not drawn into yet; the panel's resolution is read from the environment
/// through [`renderer::panel_from_environment`].
///
/// # Errors
///
/// The future never fails today: building the renderer has no fallible step
/// while it draws into its own framebuffer.
///
/// # Safety
///
/// `window`, when the renderer starts showing its frames, must name a live
/// window that outlives the returned renderer.
pub unsafe fn attach(
    window: RawWindowData,
    width: u32,
    height: u32,
) -> pill_renderer_api::AttachFuture {
    Box::pin(async move {
        let _ = (window, width, height);
        let renderer = EmbeddedRenderer::new(renderer::panel_from_environment());
        Ok(Box::new(renderer) as Box<dyn PillRenderer>)
    })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_engine::AssetManager;

    /// The registration declares the data crate's material root and can run
    /// twice, which is what a reload does.
    #[test]
    fn registering_twice_declares_the_data_crates_roots_once() {
        let mut engine = Engine::new();
        assert_eq!(pill_rendering::register(&mut engine), 0);
        assert_eq!(register(&mut engine), 0);
        assert_eq!(register(&mut engine), 0);

        assert!(
            engine.world().get_resource::<AssetManager>().is_some(),
            "the engine inserts the store the data crate registers into"
        );
    }

    /// The artifact entry point registers the declared `rendering` system once,
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
