//! The renderer contract: the trait a frontend drives, what the renderer
//! reports for each frame, and a stub for builds without a GPU.
//!
//! # Responsibilities
//!
//! - Define [`PillRenderer`], the trait a frontend drives: resize, viewport,
//!   frame submission, surfaces, and asset invalidation.
//! - Report a frame's result ([`FrameOutcome`]) and the renderer's limits and
//!   frame statistics ([`RenderCapabilities`], [`RenderMetrics`]), so a
//!   frontend can display statistics without knowing which backend produced
//!   them.
//! - Name the future a renderer is built through ([`AttachFuture`]), so a
//!   frontend can await it where it cannot block - in a browser.
//! - Provide [`HeadlessRenderer`], the stub that keeps builds and tests
//!   without a GPU surface on the same call path as the wgpu backend.
//!
//! # Design
//!
//! The host drives the renderer through a `Box<dyn PillRenderer>`, so this
//! module names no graphics type: a windowed build implements the trait on its
//! wgpu renderer, a headless build swaps in [`HeadlessRenderer`], and the
//! frame loop cannot tell the two apart.

// Standard library
use std::future::Future;
use std::pin::Pin;

// External crates
use pill_engine::AssetManager;

// Current crate
use crate::{RawWindowData, RenderFrame, RenderViewport, RendererError, SurfaceId, SurfaceOptions};

// =============================================================================
// FrameOutcome
// =============================================================================

/// What one [`PillRenderer::render`] call did with the frame it was given.
///
/// A skipped frame is not a failure. The caller keeps running and shows
/// nothing new this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameOutcome {
    /// The frame was drawn and submitted to the surface.
    Presented,
    /// Nothing was drawn: every surface was minimized, could not present yet,
    /// or had no camera this frame - or the renderer is a stub with no GPU
    /// behind it.
    Skipped,
}

// =============================================================================
// RenderCapabilities
// =============================================================================

/// What the renderer can do: the limits of its device and the scene data it
/// draws.
///
/// The limits are what the device accepts. A caller that sizes transient
/// resources - render targets, uploads - reads them here instead of assuming
/// what a desktop GPU allows. A renderer with no GPU behind it reports the
/// zero defaults.
///
/// Not `Copy`: `consumed_components` is an owned list, not a `&'static` slice.
/// A renderer loaded as a module builds the list inside its own image, so a
/// static slice would point into that image after a reload unmaps it.
#[derive(Debug, Clone, Default)]
pub struct RenderCapabilities {
    /// Largest supported 2D texture dimension, in texels.
    pub max_texture_size: u32,
    /// Largest single GPU buffer the device accepts, in bytes.
    pub max_buffer_bytes: u64,
    /// Whether the surface presents high-dynamic-range output (wider range of
    /// brightness than a normal display shows). No renderer in this
    /// repository reports `true`.
    pub hdr: bool,
    /// Names of the components this renderer reads to build a frame, in the
    /// shared form every DLL knows a component type by.
    ///
    /// The host compares this list with the render components a project uses
    /// and warns about any the renderer ignores - a component that only
    /// another renderer's data crate draws, for example. The list is empty for
    /// a renderer that reads nothing, like the headless stub.
    pub consumed_components: Vec<String>,
}

// =============================================================================
// RenderMetrics
// =============================================================================

/// Frame statistics from the most recent [`PillRenderer::render`] call that
/// drew a frame.
///
/// The windowed renderer fills these in as it works. The host reads them to
/// log the first presented frame and to notice when a pass stops drawing. A
/// renderer that has not drawn leaves every field at zero.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderMetrics {
    /// CPU time spent preparing the frame: syncing the assets, building the
    /// draw queue, and building and uploading the instance buffer.
    pub prepare_micros: u64,
    /// CPU time spent planning each extra surface's chain and submitting every
    /// drawn surface to the GPU.
    pub submit_micros: u64,
    /// Passes of the frame's chain that had at least one draw.
    ///
    /// This counts passes, not GPU draw commands. One pass draws all the
    /// instance runs it was planned with, using as many commands as those runs
    /// need. Compare it with `passes` to see how much of the chain drew; it
    /// says nothing about batching.
    pub passes_with_draws: u32,
    /// Passes of the frame's chain, whether or not they drew anything.
    pub passes: u32,
    /// Bytes of instance data the frame's draw queue carried.
    pub instance_bytes: u64,
}

// =============================================================================
// AttachFuture
// =============================================================================

/// A renderer that is still being built for a window: what a statically
/// linked renderer's attach returns.
///
/// It is a future because creating a GPU device takes time, and a browser only
/// finishes the work once control returns to its event loop. A native frontend
/// blocks on it once; a web frontend awaits it. Not `Send`: a web GPU handle
/// has to stay on the thread that created it.
pub type AttachFuture = Pin<Box<dyn Future<Output = Result<Box<dyn PillRenderer>, RendererError>>>>;

// =============================================================================
// PillRenderer
// =============================================================================

/// The contract a renderer implements and a frontend drives once per frame.
///
/// Boxing it as `Box<dyn PillRenderer>` lets a frontend swap the wgpu backend
/// for [`HeadlessRenderer`] without touching its frame loop.
///
/// A wrapper that delegates to another renderer - the host's module renderer
/// does - must forward every method, including the ones with defaults. A
/// method it leaves out runs the trait's default instead of the wrapped
/// renderer's, and nothing reports the omission.
pub trait PillRenderer {
    /// Returns the limits of the device the renderer draws on.
    ///
    /// The default returns zeroed limits, which is all a renderer with no GPU
    /// behind it can say.
    fn capabilities(&self) -> RenderCapabilities {
        RenderCapabilities::default()
    }

    /// Returns the statistics of the most recent frame that drew.
    ///
    /// The counts describe the last [`Self::render`] call that drew a frame;
    /// before the first such frame they are all zero.
    fn metrics(&self) -> RenderMetrics {
        RenderMetrics::default()
    }

    /// Tells the renderer that surface `id` now measures `width` by `height`
    /// physical pixels. An id the renderer does not draw does nothing.
    ///
    /// A zero width or height means the surface is minimized, and frames skip
    /// it until a usable size arrives. The primary surface is resized through
    /// its own id ([`SurfaceId::PRIMARY`]) like any other.
    ///
    /// This method has no default on purpose: a renderer that forgot to
    /// forward a resize would keep drawing at the old size, with nothing in
    /// the log to say why.
    fn resize_surface(&mut self, id: SurfaceId, width: u32, height: u32);

    /// Restricts drawing to a sub-region of surface `id`.
    ///
    /// `None` restores full-surface rendering. Required for the same reason
    /// [`Self::resize_surface`] is.
    fn set_surface_viewport(&mut self, id: SurfaceId, viewport: Option<RenderViewport>);

    /// Draws one frame from `assets` and presents it.
    ///
    /// Returns [`FrameOutcome::Skipped`] when there is nothing to present: a
    /// minimized surface, a surface that may not present yet, or a frame
    /// without a camera. That is a normal outcome, not an error.
    ///
    /// The asset store is read, not copied. The renderer compares it with what
    /// it last uploaded, and the borrow lasts for the call.
    ///
    /// # Errors
    ///
    /// Fails with [`RendererError`] when the GPU work itself fails: a lost or
    /// out-of-memory surface, a renderer resource the call referenced that is
    /// not in storage, or a command buffer that wgpu refuses at submission.
    ///
    /// An asset that will not build is not an error. The pass that needed it
    /// is reported and skipped.
    fn render(
        &mut self,
        frame: &RenderFrame,
        assets: &AssetManager,
    ) -> Result<FrameOutcome, RendererError>;

    /// Forces the next frame to rebuild every GPU object from the asset store.
    ///
    /// Normally the renderer rebuilds only what changed, tracked by content
    /// versions. This clears that bookkeeping, for a caller that replaced the
    /// whole asset set and cannot trust the recorded versions any more.
    fn invalidate_assets(&mut self);

    /// Starts drawing into another surface: a second window or canvas, sized
    /// `width` by `height` physical pixels. It is drawn after the primary
    /// surface, through the camera the frame resolved for it
    /// ([`RenderFrame::surface_views`]). Attaching an id that is already
    /// attached replaces that surface.
    ///
    /// The default refuses, for a renderer that draws one surface only.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::UnsupportedSurface`] if the renderer draws a
    /// single surface or `id` is [`SurfaceId::PRIMARY`]. Any other failure is
    /// one the surface creation itself reports.
    ///
    /// # Safety
    ///
    /// `window` must name a live window that outlives the surface. The caller
    /// keeps it alive until [`Self::detach_surface`] is called or the renderer
    /// is dropped, whichever comes first.
    unsafe fn attach_surface(
        &mut self,
        id: SurfaceId,
        window: RawWindowData,
        width: u32,
        height: u32,
        options: SurfaceOptions,
    ) -> Result<(), RendererError> {
        let _ = (window, width, height, options);
        Err(RendererError::UnsupportedSurface {
            detail: format!("surface {} (this renderer draws one surface)", id.0),
        })
    }

    /// Stops drawing into a surface attached with [`Self::attach_surface`] and
    /// releases it. An id the renderer does not draw does nothing.
    fn detach_surface(&mut self, _id: SurfaceId) {}

    /// Tells the renderer whether `id` may present on the next
    /// [`Self::render`].
    ///
    /// Some window systems pace a surface themselves - a Wayland subsurface,
    /// through its frame callbacks - so the frontend passes that state on. A
    /// surface that may not present is skipped instead of waited for. Every
    /// surface starts ready, and the primary surface is paced the same way.
    fn set_surface_ready(&mut self, _id: SurfaceId, _ready: bool) {}

    /// Takes the answers to the picks the GPU has finished since the last
    /// call.
    ///
    /// A pick is answered a few frames after it was asked, when the GPU has
    /// read the result back, and the frame that asked never waits for it. A
    /// pick on a surface the frame did not draw is answered in the same frame,
    /// with no hit. The frontend writes what this returns into the world's
    /// [`crate::PickResults`]. The default answers nothing, for a renderer
    /// without a pick buffer.
    fn take_pick_results(&mut self) -> Vec<crate::PickResult> {
        Vec::new()
    }

    /// Takes every preview the GPU has finished since the last call.
    ///
    /// A preview is read back a few frames after it was asked, like a pick. A
    /// request the renderer cannot draw is refused at once, so the caller does
    /// not wait for an answer that will never come. The frontend writes what
    /// this returns into the world's [`crate::PreviewResults`]. The default
    /// answers nothing, for a renderer without an off-screen preview target.
    fn take_preview_results(&mut self) -> Vec<crate::PreviewImage> {
        Vec::new()
    }
}

// =============================================================================
// HeadlessRenderer
// =============================================================================

/// Stub renderer for frontends that run without a GPU surface.
///
/// Nothing reaches a GPU. Resize, viewport, surface readiness and asset
/// invalidation do nothing, [`PillRenderer::render`] returns
/// [`FrameOutcome::Skipped`], and the default methods answer with no picks and
/// no previews. [`PillRenderer::attach_surface`] keeps its default, which
/// refuses a second surface. A headless frame loop therefore runs unchanged.
#[derive(Default)]
pub struct HeadlessRenderer;

impl PillRenderer for HeadlessRenderer {
    fn resize_surface(&mut self, _: SurfaceId, _: u32, _: u32) {}
    fn set_surface_viewport(&mut self, _: SurfaceId, _: Option<RenderViewport>) {}
    fn render(&mut self, _: &RenderFrame, _: &AssetManager) -> Result<FrameOutcome, RendererError> {
        Ok(FrameOutcome::Skipped)
    }
    fn invalidate_assets(&mut self) {}
}
