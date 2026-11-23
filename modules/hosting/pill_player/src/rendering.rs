//! A renderer attached to a window, and the runtime that draws a statically
//! linked project there.
//!
//! # Responsibilities
//!
//! - Keep a renderer backend and the window it draws on together, and drop the
//!   backend first ([`AttachedRenderer`]). Resize a surface, set its
//!   viewport, move rendering to another window, and draw the frame the
//!   `rendering` system filled in.
//! - Run a statically linked project in a window ([`RenderingRuntime`]):
//!   register the linked renderer, attach it, and draw every frame.
//!
//! # Design
//!
//! The development host has its own windowed host because its renderer is a
//! reloadable module, but it keeps the same [`AttachedRenderer`]. The two
//! differ only in where a backend comes from, never in how it is kept or
//! drawn with.

// Standard library
use std::collections::BTreeMap;
use std::future::Future;

// External crates
use pill_core::info;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_engine::{Engine, World};
use pill_renderer_api::{
    AttachFuture, PillRenderer, RawWindowData, RenderFrame, RenderViewport, RendererError,
    SurfaceId, SurfaceOptions,
};

// Current crate
use crate::render_window::{
    attach_renderer_to_window, attach_renderer_to_window_async, AttachedWindow, RendererWindow,
};
use crate::{FrameDriver, RenderingError, Runtime, StaticProject, WindowedFrameDriver};

// =============================================================================
// Constants
// =============================================================================

/// The message a windowed build reports when it has no renderer to attach.
///
/// Both ways to reach this are build decisions rather than runtime failures: a
/// project whose settings select no `renderer:`, and a shipping bundle
/// generated without the rendering feature. The message names those two, not
/// the feature, because the code that reports it is only compiled when that
/// feature is on.
const NO_LINKED_RENDERER: &str = "this build links no renderer: the project selects no `renderer:`, or its shipping bundle was generated without the rendering feature";

// =============================================================================
// SurfaceRecord
// =============================================================================

/// The window one surface draws on, plus what is needed to attach it to
/// another backend.
///
/// The primary window and every extra surface are kept in the same record, so
/// a resize, a viewport change or a pacing change needs one lookup instead of
/// a branch, and a backend swap repeats the same steps for all of them.
struct SurfaceRecord {
    /// The frontend window, held only to keep it alive while the backend draws
    /// on it.
    #[allow(dead_code)]
    window: AttachedWindow,
    /// The window's handles as data, for re-attaching to another backend.
    window_data: RawWindowData,
    /// The surface size last handed to the backend.
    size: (u32, u32),
    /// The region drawn to, or `None` for the whole surface.
    viewport: Option<RenderViewport>,
    /// How the surface presents; `None` for the primary, which takes the
    /// backend's default.
    surface_options: Option<SurfaceOptions>,
    /// Whether the surface may present (the frontend's pacing).
    ready: bool,
}

// =============================================================================
// AttachedRenderer
// =============================================================================

/// A renderer backend and the window it draws on.
///
/// Fields drop in declaration order. The backend's surface is built on the
/// window's raw handles, so the backend must not outlive the window.
pub struct AttachedRenderer {
    /// The backend drawing on the primary window; declared first so it drops
    /// first.
    renderer: Box<dyn PillRenderer>,
    /// The window a processor renderer's pixels are shown in, once the backend
    /// answers [`PillRenderer::panel_frame`].
    #[cfg(not(target_arch = "wasm32"))]
    panel_blit: Option<crate::panel_blit::PanelBlit>,
    /// The primary window, kept in the same record as every extra surface. A
    /// per-surface operation is then one lookup instead of a branch, and a
    /// backend swap repeats the same steps for both. Declared after
    /// `renderer`, so the backend drops before the window it draws on.
    primary_surface: SurfaceRecord,
    /// Whether a frame with draw calls has been presented, for the one-time
    /// first-frame line.
    has_presented_scene: bool,
    /// The pass count last written to the log, so a frame that changes how
    /// much it draws is reported once rather than every frame.
    last_reported_pass_count: u32,
    /// Whether [`Self::detach_backend`] dropped the backend and nothing has
    /// replaced it yet. [`Self::replace_backend`] asserts this, because
    /// assigning over a live backend drops it while its module may be gone.
    detached: bool,
    /// Every surface attached after the primary window, by surface id.
    /// Declared after `renderer`, so the backend drops first.
    extra_surfaces: BTreeMap<u32, SurfaceRecord>,
}

impl AttachedRenderer {
    /// Attach a backend built by `attach` to `window`.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when the window cannot give its handles or
    /// `attach` fails.
    pub fn attach<W: RendererWindow>(
        window: W,
        width: u32,
        height: u32,
        attach: impl FnOnce(RawWindowData) -> Result<Box<dyn PillRenderer>, RendererError>,
    ) -> Result<Self, RendererError> {
        let (renderer, window, window_data) = attach_renderer_to_window(window, attach)?;
        Ok(Self::from_parts(
            renderer,
            window,
            window_data,
            width,
            height,
        ))
    }

    /// Like [`Self::attach`], but the backend is built asynchronously: `attach`
    /// returns a future that produces it.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when the window cannot give its handles or
    /// the future fails.
    pub async fn attach_async<W, F>(
        window: W,
        width: u32,
        height: u32,
        attach: impl FnOnce(RawWindowData) -> F,
    ) -> Result<Self, RendererError>
    where
        W: RendererWindow,
        F: Future<Output = Result<Box<dyn PillRenderer>, RendererError>>,
    {
        let (renderer, window, window_data) =
            attach_renderer_to_window_async(window, attach).await?;
        Ok(Self::from_parts(
            renderer,
            window,
            window_data,
            width,
            height,
        ))
    }

    /// Build a renderer from a backend and the parts of its window.
    ///
    /// This is the one place the struct is built, so the synchronous and
    /// asynchronous attach paths cannot drift apart.
    fn from_parts(
        renderer: Box<dyn PillRenderer>,
        window: AttachedWindow,
        window_data: RawWindowData,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            renderer,
            #[cfg(not(target_arch = "wasm32"))]
            panel_blit: None,
            primary_surface: SurfaceRecord {
                window,
                window_data,
                size: (width, height),
                viewport: None,
                surface_options: None,
                ready: true,
            },
            has_presented_scene: false,
            last_reported_pass_count: 0,
            detached: false,
            extra_surfaces: BTreeMap::new(),
        }
    }

    /// The record for surface `id`: the primary window, or one attached later.
    fn surface_record_mut(&mut self, id: SurfaceId) -> Option<&mut SurfaceRecord> {
        if id == SurfaceId::PRIMARY {
            Some(&mut self.primary_surface)
        } else {
            self.extra_surfaces.get_mut(&id.0)
        }
    }

    /// Move rendering to a new window, with a backend built by `attach`.
    ///
    /// The replacement is built before the current backend is dropped, so a
    /// failure leaves the current window drawn.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when the new window cannot give its handles
    /// or `attach` fails.
    pub fn retarget<W: RendererWindow>(
        &mut self,
        window: W,
        width: u32,
        height: u32,
        attach: impl FnOnce(RawWindowData) -> Result<Box<dyn PillRenderer>, RendererError>,
    ) -> Result<(), RendererError> {
        let attachment = attach_renderer_to_window(window, attach)?;
        self.install_attachment(attachment, width, height);
        Ok(())
    }

    /// Like [`Self::retarget`], but the backend is built asynchronously:
    /// `attach` returns a future that produces it.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when the new window cannot give its handles
    /// or the future fails.
    pub async fn retarget_async<W, F>(
        &mut self,
        window: W,
        width: u32,
        height: u32,
        attach: impl FnOnce(RawWindowData) -> F,
    ) -> Result<(), RendererError>
    where
        W: RendererWindow,
        F: Future<Output = Result<Box<dyn PillRenderer>, RendererError>>,
    {
        let attachment = attach_renderer_to_window_async(window, attach).await?;
        self.install_attachment(attachment, width, height);
        Ok(())
    }

    /// Take over a backend and the window it was built on, replacing both.
    fn install_attachment(
        &mut self,
        (renderer, window, window_data): (Box<dyn PillRenderer>, AttachedWindow, RawWindowData),
        width: u32,
        height: u32,
    ) {
        // Step 1: Swap in the new backend and window. The renderer is assigned
        // first, so the old renderer drops while its window is still alive and
        // that window is released only afterwards. The viewport and pacing the
        // old window was given survive the swap, because they belong to the
        // surface, not to the backend.
        let viewport = self.primary_surface.viewport;
        let ready = self.primary_surface.ready;
        self.renderer = renderer;
        // A new window means the old blit belongs to a window that is going.
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.panel_blit = None;
        }
        self.detached = false;
        self.primary_surface = SurfaceRecord {
            window,
            window_data,
            size: (width, height),
            viewport,
            surface_options: None,
            ready,
        };
        // Step 2: Tell the new backend what the old one was told.
        self.replay_surfaces();
    }

    /// Replace the backend on the same window, keeping the viewport, the pacing
    /// and every extra surface.
    ///
    /// The current backend must be detached first ([`Self::detach_backend`]).
    /// Assigning over a live backend drops it, and a backend that lives in a
    /// module the host is about to unload must be gone before that unload.
    /// Every caller detaches first, and this asserts it in debug builds.
    ///
    /// # Panics
    ///
    /// Panics in debug builds if the current backend has not been detached.
    pub fn replace_backend(&mut self, renderer: Box<dyn PillRenderer>) {
        debug_assert!(
            self.detached,
            "replace_backend over a live backend drops it while its module may be gone; call detach_backend first"
        );
        self.renderer = renderer;
        // The window did not change, so a blit of the old backend's pixels
        // must not survive into the new one: it holds that backend's frame.
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.panel_blit = None;
        }
        self.detached = false;
        self.replay_surfaces();
    }

    /// Tell a new backend what the one it replaced was told: the primary
    /// surface's viewport and pacing first, then every extra surface.
    ///
    /// The pacing is easy to forget. Without it, a window the frontend asked to
    /// hold would start presenting again on the first frame after a renderer
    /// reload. The primary keeps it in its surface record for the same reason
    /// the extra surfaces do.
    fn replay_surfaces(&mut self) {
        let viewport = self.primary_surface.viewport;
        let ready = self.primary_surface.ready;
        self.renderer
            .set_surface_viewport(SurfaceId::PRIMARY, viewport);
        self.renderer.set_surface_ready(SurfaceId::PRIMARY, ready);
        self.reattach_extra_surfaces();
    }

    /// Attach every extra surface to the current backend again, with its size,
    /// viewport and pacing, after the backend was replaced (a renderer reload)
    /// or moved to another window.
    ///
    /// A surface the new backend refuses stays in the map and its refusal is
    /// logged: its window is still open, and the next replacement tries again.
    fn reattach_extra_surfaces(&mut self) {
        for (id, surface) in &self.extra_surfaces {
            let surface_options = surface.surface_options.unwrap_or_default();
            // SAFETY: `extra_surfaces` keeps the surface's window alive until
            // the surface is detached or this renderer is dropped. The backend
            // is declared before that map, so it drops first either way.
            let attach_result = unsafe {
                self.renderer.attach_surface(
                    SurfaceId(*id),
                    surface.window_data,
                    surface.size.0,
                    surface.size.1,
                    surface_options,
                )
            };
            match attach_result {
                Ok(()) => {
                    self.renderer
                        .set_surface_viewport(SurfaceId(*id), surface.viewport);
                    self.renderer
                        .set_surface_ready(SurfaceId(*id), surface.ready);
                    info!(
                        target: telemetry_target::RENDERING,
                        "surface {id} reattached to the new renderer"
                    );
                }
                Err(error) => pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    "surface {id} could not be reattached to the new renderer: {error}"
                ),
            }
        }
    }

    /// Draw into another window as surface `id`, sized `width` by `height`
    /// physical pixels. The window stays alive until [`Self::detach_surface`].
    ///
    /// The world's [`pill_renderer_api::SurfaceCameras`] decides which camera
    /// draws into the surface; a surface with no binding stays empty.
    ///
    /// A backend that draws only one surface - the placeholder a development
    /// host holds while its renderer module builds or reloads - leaves the
    /// surface pending instead of refusing it. The surface is kept and attached
    /// to the next backend that replaces this one.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when the window cannot give its handles or
    /// the backend refuses the surface for another reason.
    pub fn attach_surface<W: RendererWindow>(
        &mut self,
        id: SurfaceId,
        window: W,
        width: u32,
        height: u32,
        surface_options: SurfaceOptions,
    ) -> Result<(), RendererError> {
        // Step 1: Take the window's handles as data; the surface is built on
        // them.
        let window_data = RawWindowData::from_window(&window)?;
        // SAFETY: the window is stored in `extra_surfaces` below and kept until
        // the surface is detached or this renderer is dropped; the backend,
        // declared first, drops before it.
        let attach_result = unsafe {
            self.renderer
                .attach_surface(id, window_data, width, height, surface_options)
        };
        // Step 2: A backend that draws one surface leaves the surface pending;
        // anything else it refuses is an error.
        match attach_result {
            Ok(()) => {}
            Err(RendererError::UnsupportedSurface { detail }) => info!(
                target: telemetry_target::RENDERING,
                "surface {} waits for a renderer that draws more surfaces ({detail})", id.0
            ),
            Err(error) => return Err(error),
        }
        // Step 3: Keep the window, and what re-attaching the surface needs.
        self.extra_surfaces.insert(
            id.0,
            SurfaceRecord {
                window: Box::new(window),
                window_data,
                size: (width, height),
                viewport: None,
                surface_options: Some(surface_options),
                ready: true,
            },
        );
        Ok(())
    }

    /// Stop drawing into surface `id`, then release its window.
    pub fn detach_surface(&mut self, id: SurfaceId) {
        // The backend's surface first, then the window it was built on.
        self.renderer.detach_surface(id);
        self.extra_surfaces.remove(&id.0);
    }

    /// Forward a physical resize of surface `id`.
    ///
    /// One method serves every surface. The size is recorded in the surface's
    /// own record, so the caller does not need to know whether it resizes the
    /// primary window or an extra surface.
    pub fn resize_surface(&mut self, id: SurfaceId, width: u32, height: u32) {
        let Some(record) = self.surface_record_mut(id) else {
            return;
        };
        record.size = (width, height);
        let viewport = record.viewport;
        self.renderer.resize_surface(id, width, height);
        self.renderer.set_surface_viewport(id, viewport);
    }

    /// Restrict drawing to a region of surface `id`; `None` draws all of it.
    pub fn set_surface_viewport(&mut self, id: SurfaceId, viewport: Option<RenderViewport>) {
        let Some(record) = self.surface_record_mut(id) else {
            return;
        };
        record.viewport = viewport;
        self.renderer.set_surface_viewport(id, viewport);
    }

    /// Whether surface `id` may present on the next frame; see
    /// [`PillRenderer::set_surface_ready`]. Applies to the primary surface too,
    /// which keeps it in its record so a backend swap cannot lose it.
    pub fn set_surface_ready(&mut self, id: SurfaceId, ready: bool) {
        if let Some(record) = self.surface_record_mut(id) {
            record.ready = ready;
        }
        self.renderer.set_surface_ready(id, ready);
    }

    /// The ids of the surfaces attached after the primary window.
    pub fn extra_surface_ids(&self) -> Vec<SurfaceId> {
        self.extra_surfaces
            .keys()
            .map(|id| SurfaceId(*id))
            .collect()
    }

    /// Drop the backend and draw nothing until another one replaces it.
    ///
    /// For a host whose backend lives in a module about to be unloaded: the
    /// backend must go while that module is still mapped.
    pub fn detach_backend(&mut self) {
        self.renderer = Box::new(pill_renderer_api::HeadlessRenderer);
        self.detached = true;
    }

    /// The current backend; [`Self::detach_backend`] leaves the headless one.
    pub fn renderer(&self) -> &dyn PillRenderer {
        self.renderer.as_ref()
    }

    /// The primary window's handles as data.
    pub fn window_data(&self) -> RawWindowData {
        self.primary_surface.window_data
    }

    /// The surface size last handed to the backend.
    pub fn surface_size(&self) -> (u32, u32) {
        self.primary_surface.size
    }

    /// The region drawn to, or `None` for the whole window.
    pub fn viewport(&self) -> Option<RenderViewport> {
        self.primary_surface.viewport
    }

    /// Draw the frame the `rendering` system left in `world`, if it left one.
    ///
    /// The renderer reads the frame and the asset store straight out of the
    /// world instead of being handed a copy of the assets. Both borrows last
    /// for the whole call, which stops the store from changing while a frame
    /// that uses it is being drawn.
    ///
    /// A world with no frame, or with no asset store, draws nothing. That is
    /// not an error.
    ///
    /// # Errors
    ///
    /// Returns whatever the backend's `render` returns.
    pub fn render(&mut self, world: &World) -> Result<(), RendererError> {
        // Step 1: Take the frame and the store it was built from, if this
        // world has both.
        let (Some(frame), Some(assets)) = (
            world.get_resource::<RenderFrame>(),
            world.get_resource::<pill_engine::AssetManager>(),
        ) else {
            return Ok(());
        };
        let outcome = self.renderer.render(frame, assets)?;
        let metrics = self.renderer.metrics();
        // Step 2: Report a frame whose drawn-pass count changed. This line
        // shows, in a run's log, that a pass stopped drawing (a material no
        // pass names, a shader that failed to load). It also lets a test check
        // what a settled frame drew without anyone watching the window.
        if metrics.passes_with_draws != self.last_reported_pass_count {
            self.last_reported_pass_count = metrics.passes_with_draws;
            info!(
                target: telemetry_target::RENDERING,
                passes_with_draws = metrics.passes_with_draws,
                passes = metrics.passes,
                instances = frame.instances.len(),
                "[render] frame draws"
            );
        }
        // Step 3: Report the first frame that presented with draw calls. A
        // window opened before any camera is enabled presents empty frames;
        // none of those is the start of the scene.
        if !self.has_presented_scene
            && matches!(outcome, pill_renderer_api::FrameOutcome::Presented)
            && self.renderer.metrics().passes_with_draws > 0
        {
            self.has_presented_scene = true;
            // Through the log rather than stdout, because a browser has no
            // stdout. The terminal lane prints the same line on native.
            let metrics = self.renderer.metrics();
            info!(
                target: telemetry_target::RENDERING,
                passes = metrics.passes,
                passes_with_draws = metrics.passes_with_draws,
                instance_bytes = metrics.instance_bytes,
                prepare_micros = metrics.prepare_micros,
                submit_micros = metrics.submit_micros,
                "[render] First frame: {outcome:?}; camera={}; instances={}",
                frame
                    .surface_views
                    .contains_key(&pill_renderer_api::SurfaceId::PRIMARY),
                frame.instances.len()
            );
        }
        Ok(())
    }

    /// Draw the frame the `rendering` system left in `world`, then hand back
    /// the picks and previews the GPU finished since the last frame.
    ///
    /// The order matters: the renderer cannot answer a request made this frame
    /// before the draw that produces the answer. A frontend that delivered
    /// first would read the previous frame's results again. Both windowed
    /// drivers call this one method, so neither of them can get the order
    /// wrong.
    ///
    /// # Errors
    ///
    /// Returns what [`Self::render`] returns.
    pub fn present(&mut self, world: &mut World) -> Result<(), RendererError> {
        // Step 1: Draw the frame the `rendering` system left in the world.
        self.render(world)?;
        // Step 2: Show the pixels a processor renderer drew, when it drew them
        // itself instead of on a surface.
        #[cfg(not(target_arch = "wasm32"))]
        self.show_panel_frame();
        // Step 3: Hand back the picks and previews the GPU finished meanwhile.
        self.deliver_pick_results(world);
        self.deliver_preview_results(world);
        Ok(())
    }

    /// Shows a processor renderer's most recent frame in the primary window.
    ///
    /// A renderer that presents through a graphics device answers no panel
    /// frame, and nothing happens. The frame keeps the panel's resolution and
    /// is stretched over the window, so a preview shows what a device panel
    /// will.
    #[cfg(not(target_arch = "wasm32"))]
    fn show_panel_frame(&mut self) {
        let Some(frame) = self.renderer.panel_frame() else {
            return;
        };
        if self.panel_blit.is_none() {
            self.panel_blit =
                crate::panel_blit::PanelBlit::new(self.primary_surface.window_data.window);
        }
        let Some(panel_blit) = &mut self.panel_blit else {
            return;
        };
        panel_blit.present(frame, self.primary_surface.size);
    }

    /// Move the picks the renderer has answered since the last call into the
    /// world's [`pill_renderer_api::PickResults`].
    ///
    /// An answer arrives a few frames after its request, when the GPU
    /// finishes. A world without the resource drops the answers. Callers reach
    /// this through [`Self::present`], which delivers them after the draw.
    fn deliver_pick_results(&mut self, world: &mut World) {
        let pick_results = self.renderer.take_pick_results();
        if pick_results.is_empty() {
            return;
        }
        if let Some(delivered) = world.get_resource_mut::<pill_renderer_api::PickResults>() {
            delivered.results.extend(pick_results);
        }
    }

    /// Move the previews the renderer has read back since the last call into
    /// the world's [`pill_renderer_api::PreviewResults`].
    ///
    /// Like a pick, a preview is answered a few frames after it was asked. A
    /// world without the resource drops the result.
    fn deliver_preview_results(&mut self, world: &mut World) {
        let preview_results = self.renderer.take_preview_results();
        if preview_results.is_empty() {
            return;
        }
        if let Some(delivered) = world.get_resource_mut::<pill_renderer_api::PreviewResults>() {
            delivered.results.extend(preview_results);
        }
    }
}

// =============================================================================
// RenderingRuntime
// =============================================================================

/// A statically linked project running in a window.
///
/// Fields drop in declaration order. The engine goes first, as in every host,
/// then the backend, then the window it drew on.
pub struct RenderingRuntime {
    /// The engine and its frame state.
    runtime: Runtime,
    /// The linked renderer's backend on the window.
    attached_renderer: AttachedRenderer,
}

impl RenderingRuntime {
    /// Move rendering to a newly created window; see
    /// [`AttachedRenderer::retarget_async`].
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when the new window cannot be attached,
    /// including when this build links no renderer to move.
    pub async fn retarget_render_window<W: RendererWindow>(
        &mut self,
        window: W,
        width: u32,
        height: u32,
    ) -> Result<(), RendererError> {
        let Some((renderer, _)) = self.runtime.static_renderer() else {
            return Err(RendererError::Other {
                detail: NO_LINKED_RENDERER.to_owned(),
            });
        };
        self.attached_renderer
            .retarget_async(window, width, height, |window_data| {
                attach_linked_renderer(renderer, window_data, width, height)
            })
            .await
    }

    /// Execute one ECS frame and present its resulting world to the surface.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when the renderer fails.
    pub fn run_frame(&mut self) -> Result<(), RendererError> {
        // Step 1: Run the engine's frame, which fills the frame resource.
        self.runtime.run_frame_from(Instant::now());
        // Step 2: Draw what it left and hand back the answers the GPU finished.
        self.attached_renderer
            .present(self.runtime.engine_mut().world_mut())?;
        Ok(())
    }

    /// Live frame rate; see [`Runtime::frames_per_second`].
    pub fn frames_per_second(&self) -> f64 {
        self.runtime.frames_per_second()
    }

    /// Read-only engine access.
    pub fn engine(&self) -> &Engine {
        self.runtime.engine()
    }

    /// Mutable engine access for frame-boundary work.
    pub fn engine_mut(&mut self) -> &mut Engine {
        self.runtime.engine_mut()
    }
}

impl FrameDriver for RenderingRuntime {
    type Error = RendererError;

    fn run_frame(&mut self) -> Result<(), RendererError> {
        RenderingRuntime::run_frame(self)
    }

    fn engine_mut(&mut self) -> &mut Engine {
        RenderingRuntime::engine_mut(self)
    }
}

impl WindowedFrameDriver for RenderingRuntime {
    fn display_mut(&mut self) -> &mut AttachedRenderer {
        &mut self.attached_renderer
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Register the linked renderer and attach it to `window`.
///
/// A frontend calls this after project setup succeeds, so a slow start never
/// shows a blank surface. Asynchronous because building the renderer is: a
/// native frontend blocks on it once, a web frontend awaits it.
///
/// # Errors
///
/// Returns [`RenderingError::Renderer`] when the renderer's registration fails
/// or the backend cannot attach to the window. A build that links no renderer
/// also returns it, carrying the message that names the two ways a windowed
/// build ends up without one.
pub async fn attach_renderer<W: RendererWindow>(
    mut runtime: Runtime,
    window: W,
    width: u32,
    height: u32,
) -> Result<RenderingRuntime, RenderingError> {
    info!(
        target: telemetry_target::RENDERING,
        width,
        height,
        "attaching the engine renderer to the window surface"
    );
    // Step 1: A build with no linked renderer stops here, before any window is
    // bound. Registering nothing and failing the attach later would report the
    // same fact one step further in, as a renderer error.
    let Some((renderer, owner)) = runtime.static_renderer() else {
        return Err(RenderingError::Renderer(RendererError::Other {
            detail: NO_LINKED_RENDERER.to_owned(),
        }));
    };
    // Step 2: Register its system the way its module entry point would. The
    // renderer crate exposes one function, so its registration half is the
    // whole entry point.
    crate::static_project::run_linked_entry_point(
        runtime.engine_mut(),
        pill_engine::component_registry::no_registration,
        renderer.init,
        Some(owner),
        None,
    )
    .map_err(|status| RendererError::Other {
        detail: format!("the renderer failed to initialize with status {status}"),
    })?;
    // Step 3: Build it on the window through the bundle's function pointer.
    let attached_renderer = AttachedRenderer::attach_async(window, width, height, |window_data| {
        attach_linked_renderer(renderer, window_data, width, height)
    })
    .await?;
    Ok(RenderingRuntime {
        runtime,
        attached_renderer,
    })
}

/// Start a statically linked project and attach its renderer to `window`.
///
/// # Errors
///
/// Returns a [`RenderingError`] carrying the setup failure or the renderer's.
pub async fn setup_rendering<W: RendererWindow>(
    project: StaticProject,
    window: W,
    width: u32,
    height: u32,
) -> Result<RenderingRuntime, RenderingError> {
    let runtime = crate::setup(project)?;
    attach_renderer(runtime, window, width, height).await
}

/// Start building a backend on `window_data` from the linked renderer.
///
/// # Errors
///
/// The returned future fails when the renderer cannot attach.
fn attach_linked_renderer(
    renderer: crate::StaticRenderer,
    window_data: RawWindowData,
    width: u32,
    height: u32,
) -> AttachFuture {
    // SAFETY: every caller stores the backend in an `AttachedRenderer`, whose
    // `renderer` field is declared before `window`: the backend drops while
    // the window it was built on is alive.
    unsafe { (renderer.attach)(window_data, width, height) }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use pill_renderer_api::{FrameOutcome, RawWindowKind};

    use super::*;

    /// The calls a backend was told, one line per call.
    type RecordedCalls = Arc<Mutex<Vec<String>>>;

    /// A backend that records what it is told, so a test can see what a
    /// replacement replays.
    struct RecordingRenderer {
        calls: RecordedCalls,
    }

    impl RecordingRenderer {
        /// A backend recording into `calls`.
        fn new(calls: RecordedCalls) -> Self {
            Self { calls }
        }

        /// Append one line to the record.
        fn record(&self, line: String) {
            self.calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(line);
        }
    }

    impl PillRenderer for RecordingRenderer {
        fn resize_surface(&mut self, id: SurfaceId, width: u32, height: u32) {
            self.record(format!("resize {} {width}x{height}", id.0));
        }

        fn set_surface_viewport(&mut self, id: SurfaceId, viewport: Option<RenderViewport>) {
            self.record(format!("viewport {} {viewport:?}", id.0));
        }

        fn set_surface_ready(&mut self, id: SurfaceId, ready: bool) {
            self.record(format!("ready {} {ready}", id.0));
        }

        fn render(
            &mut self,
            _frame: &RenderFrame,
            _assets: &pill_engine::AssetManager,
        ) -> Result<FrameOutcome, RendererError> {
            Ok(FrameOutcome::Skipped)
        }

        fn invalidate_assets(&mut self) {}
    }

    /// A renderer over `renderer` and a synthetic primary window, for the tests
    /// that check what a replacement replays.
    fn attached_renderer_over(renderer: Box<dyn PillRenderer>) -> AttachedRenderer {
        AttachedRenderer::from_parts(
            renderer,
            Box::new(()),
            RawWindowData {
                kind: RawWindowKind::Win32,
                window: 0,
                window_aux: 0,
                display: 0,
                screen: 0,
            },
            800,
            600,
        )
    }

    /// A backend replacement replays the primary surface's pacing.
    ///
    /// Without the replay, a window the frontend asked to hold would start
    /// presenting again on the first frame after a renderer reload.
    #[test]
    fn a_replacement_replays_the_primary_surfaces_pacing() {
        let first_backend_calls = Arc::new(Mutex::new(Vec::new()));
        let mut attached_renderer = attached_renderer_over(Box::new(RecordingRenderer::new(
            first_backend_calls.clone(),
        )));

        // The frontend holds the primary, and sets a viewport while it does.
        attached_renderer.set_surface_ready(SurfaceId::PRIMARY, false);
        attached_renderer.set_surface_viewport(SurfaceId::PRIMARY, None);
        let recorded_before_replacement = first_backend_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(
            recorded_before_replacement
                .iter()
                .any(|line| line == "ready 0 false"),
            "the first backend was told to hold: {recorded_before_replacement:?}"
        );

        // A renderer reload replaces the backend.
        attached_renderer.detach_backend();
        let replacement_backend_calls = Arc::new(Mutex::new(Vec::new()));
        attached_renderer.replace_backend(Box::new(RecordingRenderer::new(
            replacement_backend_calls.clone(),
        )));
        let recorded_after_replacement = replacement_backend_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(
            recorded_after_replacement
                .iter()
                .any(|line| line == "ready 0 false"),
            "the replacement was told to hold the primary: {recorded_after_replacement:?}"
        );
        assert!(
            recorded_after_replacement
                .iter()
                .any(|line| line.starts_with("viewport 0")),
            "the replacement was told the viewport: {recorded_after_replacement:?}"
        );
    }

    /// A resize of the primary reaches its record and the backend.
    #[test]
    fn a_primary_resize_reaches_the_surface_and_the_backend() {
        let backend_calls = Arc::new(Mutex::new(Vec::new()));
        let mut attached_renderer =
            attached_renderer_over(Box::new(RecordingRenderer::new(backend_calls.clone())));

        attached_renderer.resize_surface(SurfaceId::PRIMARY, 1024, 768);
        assert_eq!(attached_renderer.surface_size(), (1024, 768));
        let recorded = backend_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        assert!(
            recorded.iter().any(|line| line == "resize 0 1024x768"),
            "the backend was resized: {recorded:?}"
        );
    }

    /// Replacing the backend without detaching it first trips the precondition
    /// in debug builds: assigning over a live backend drops it while the module
    /// it lives in may already be gone.
    #[test]
    #[should_panic(expected = "call detach_backend first")]
    fn replacing_a_live_backend_trips_the_precondition() {
        let mut attached_renderer = attached_renderer_over(Box::new(RecordingRenderer::new(
            Arc::new(Mutex::new(Vec::new())),
        )));
        attached_renderer.replace_backend(Box::new(RecordingRenderer::new(Arc::new(Mutex::new(
            Vec::new(),
        )))));
    }
}
