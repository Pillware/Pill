//! The master renderer: draws a frame's chain and instances on the device and
//! state [`State`] owns.
//!
//! # Responsibilities
//!
//! - Keep the GPU's copy of each asset up to date with the asset store,
//!   rebuilding only what changed ([`RenderingResourcesManager`] and its
//!   per-type sync helpers).
//! - Hold each surface's chain in a [`GpuChain`]. Rebuild it when the frame
//!   says the chain changed, and plan each frame's passes.
//! - Submit through [`State`]: one command encoder, one wgpu render pass per
//!   planned pass, then the present.
//! - Draw into several surfaces from one device. Each surface is an entry of
//!   one map ([`SurfaceTarget`]), and each is drawn through its own camera:
//!   the primary surface the renderer was built on and every one attached
//!   since.
//! - Draw the overlay - debug drawing, outlines, picking - over each surface
//!   after its chain (`crate::overlay`).
//!
//! # Design
//!
//! The renderer is the two halves a frame needs: an asset store kept current
//! on the GPU, and a chain turned into passes. Both are checked against
//! something cheap - the store's revision and the chain's generation - so an
//! unchanged frame costs two comparisons and no rebuilds. A resource that
//! fails to build is named in the log and skipped.
//!
//! A surface owns what is sized to it: its swapchain, depth buffer, offscreen
//! targets, their pass objects, its camera handle and the overlay's targets.
//! That holds whether it is the primary surface or one attached later. The
//! device, the asset caches and the queue are shared, and [`State`] draws a
//! surface by being handed its target, so no per-surface field is ever swapped
//! in and out.

// Standard library
use std::collections::{BTreeMap, HashMap};

// External crates
use pill_core::info;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_engine::common_components::TransformComponent;
use pill_engine::AssetManager;
use pill_renderer_api::{CameraComponent, RawWindowData, SurfaceId, SurfaceOptions};

// Current crate
use crate::{
    api::{FrameOutcome, PillRenderer, RenderCapabilities, RenderMetrics},
    chain::{GpuChain, PassOutput, PassPlan, PassSlot},
    components::RenderViewport,
    config::{
        CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX, ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
        INSTANCE_BATCH_SIZE, MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
        MATERIAL_TEXTURES_BIND_GROUP_LAYOUT_INDEX,
    },
    drawers::mesh_drawer::MeshDrawer,
    error::{capturing_validation, RendererError, Result},
    frame::RenderFrame,
    overlay::{self, Overlay, OverlayFrame, OverlayTargets},
    profiler::Profiler,
    render_queue::RenderQueueItem,
    rendering_resources_manager::RenderingResourcesManager,
    resources::{RendererCamera, RendererCameraHandle, RendererResourceStorage, RendererTexture},
    surface::Surface,
    ui::UiRenderer,
    Instance,
};

// =============================================================================
// Constants
// =============================================================================

/// The colour format of every offscreen target a chain declares.
///
/// It is half-float rather than the surface's `Unorm` format, because a lit
/// frame can produce values well above 1.0. A target that cannot hold those
/// values would clip the picture before a later pass brings it back into
/// range.
pub(crate) const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Frames between two sampled log reports: one from the GPU profiler and one
/// from the UI pass. Tracy records every frame, but these two logs report a
/// sample so they stay readable.
const GPU_REPORT_INTERVAL: u32 = 120;

/// Colour every frame starts from, whatever the first pass is.
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.15,
    g: 0.15,
    b: 0.15,
    a: 1.0,
};

/// What a frame leaves outside a viewport that does not cover the whole target.
///
/// Zero in every channel, which is transparent in both alpha conventions, so a
/// frame that draws into only part of a window does not hide whatever else
/// paints the rest of that window.
const CLEAR_TRANSPARENT: wgpu::Color = wgpu::Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// Fog values the engine parameters keep so their layout matches what a shader
/// expects.
///
/// The parameter block declares a fog colour and density first, because a
/// project's shader may read them, but no frontend sets fog yet. The frame
/// therefore writes zero density and black, rather than leaving whatever the
/// previous frame wrote.
const NO_FOG_DENSITY: f32 = 0.0;
const NO_FOG_COLOR: [f32; 3] = [0.0; 3];

// =============================================================================
// Renderer
// =============================================================================

/// The master renderer: the asset caches, a chain per surface, and the
/// submission behind the [`PillRenderer`] contract.
///
/// One renderer owns the surface of the window it was built on, every surface
/// attached since, and everything drawn to them: the shared GPU state, the
/// resource cache that keeps the GPU's copy of each asset current, and each
/// surface's chain.
pub struct Renderer {
    /// The wgpu device, queue and shared caches this renderer draws from.
    pub state: State,
    /// The GPU object behind every live asset, with the asset version it was
    /// built from. Comparing those versions is how a frame finds what changed,
    /// one asset type at a time.
    rendering_resources_manager: RenderingResourcesManager,
    /// The numbers measured for the last drawn frame: preparation and
    /// submission times, instance bytes and pass counts. Reported through
    /// [`PillRenderer::metrics`].
    metrics: RenderMetrics,
    /// The frame's instances in queue order, as uploaded. Kept between frames
    /// so its allocation is reused.
    queued_instances: Vec<Instance>,
    /// Every surface this renderer draws, by id: the one it was built on under
    /// [`SurfaceId::PRIMARY`], and each one attached since.
    surface_targets: BTreeMap<u32, SurfaceTarget>,
}

// =============================================================================
// Surface Target
// =============================================================================

/// One surface the renderer draws into, and everything sized to it.
///
/// The primary surface the renderer was built on and every surface attached
/// since are the same type in one map: a swapchain, the depth buffer sized to
/// it, the offscreen targets its chain declares, the chain's own pass objects,
/// the camera it is drawn through, and the overlay's targets.
pub(crate) struct SurfaceTarget {
    /// Which surface this is, as the frame's views, picks and UI name it.
    id: SurfaceId,
    /// The surface's swapchain.
    surface: Surface,
    /// Its depth buffer, sized to it.
    depth_texture: RendererTexture,
    /// Its offscreen targets, sized to it.
    offscreen: HashMap<String, RendererTexture>,
    /// The chain's pass objects bound to this surface's targets.
    chain: GpuChain,
    /// The camera this surface is drawn through, as a handle into the shared
    /// camera storage.
    camera: RendererCameraHandle,
    /// The overlay's targets, sized to it.
    overlay_targets: OverlayTargets,
    /// The region drawn to, or `None` for the whole surface.
    viewport: Option<RenderViewport>,
    /// Whether the surface is zero-sized.
    minimized: bool,
    /// Whether the surface may present this frame.
    ready: bool,
    /// Whether a frame has been presented to it, for the one-time line.
    ///
    /// The primary surface's first frame is not announced - it is the window
    /// the renderer was built on - so it starts out presented.
    presented: bool,
}

impl SurfaceTarget {
    /// Rebuild what is sized to the surface for `width` by `height`.
    ///
    /// The swapchain, the depth buffer and the overlay's targets are rebuilt
    /// here. The offscreen targets are dropped and the chain is told to rebuild
    /// the passes bound to them on the next frame, because those are sized to
    /// the surface too.
    ///
    /// # Errors
    ///
    /// Returns the error when the surface refuses the new configuration, or
    /// when the new depth buffer cannot be created.
    fn resize(
        &mut self,
        device: &wgpu::Device,
        overlay: &Overlay,
        width: u32,
        height: u32,
    ) -> Result<()> {
        // Step 1: A zero-sized surface has nothing to size.
        self.minimized = width == 0 || height == 0;
        if self.minimized {
            return Ok(());
        }
        // Step 2: Resize the swapchain, then rebuild the buffers and targets
        // sized to it.
        self.surface.resize(device, width, height)?;
        // The new depth buffer is built only after the surface accepted the new
        // size, so a refused resize leaves the old surface and the old depth
        // buffer in place instead of half of each.
        let depth_texture = RendererTexture::new_depth_texture(
            device,
            self.surface.configuration(),
            "depth_texture",
        )?;
        self.depth_texture = depth_texture;
        self.overlay_targets = overlay.new_targets(device, self.surface.size());
        // Step 3: Drop the offscreen targets, so the chain rebuilds the passes
        // bound to them on the next frame.
        self.offscreen.clear();
        self.chain.invalidate_targets();
        Ok(())
    }
}

// =============================================================================
// Surface Draw
// =============================================================================

/// One surface's frame between the acquire and the present.
///
/// Everything the recording phases need that changes per frame: the acquired
/// image, the view the passes draw into, the camera, the viewport and the
/// frame itself. Holding it apart from [`SurfaceTarget`] is what lets the
/// phases be recorded in named steps without threading a dozen arguments
/// through each.
struct SurfaceDraw<'a> {
    /// Which surface this is, as the overlay and the UI name it.
    surface_id: u32,
    /// The image acquired from the surface, presented at the end.
    image: wgpu::SurfaceTexture,
    /// The view the passes draw into, through the surface's own format.
    view: wgpu::TextureView,
    /// The size of the acquired image, in pixels.
    size: (u32, u32),
    /// The region of it this frame covers.
    viewport: RenderViewport,
    /// The camera this surface is drawn through.
    camera: RendererCameraHandle,
    /// The frame being drawn.
    frame: &'a RenderFrame,
    /// The frame's queue: in it, a queue position is an instance index.
    render_queue: &'a [RenderQueueItem],
}

impl Renderer {
    /// Creates the renderer synchronously, blocking on [`Renderer::new_async`].
    ///
    /// For the renderer module's attach export, which crosses a C ABI no
    /// future can: loaded modules exist only in a native development build,
    /// which can block.
    ///
    /// # Errors
    ///
    /// Returns the errors of [`Renderer::new_async`], which does the work.
    ///
    /// # Safety
    ///
    /// As [`Renderer::new_async`]: `window` must name a live window that
    /// outlives the renderer.
    #[cfg(not(target_arch = "wasm32"))]
    pub unsafe fn new(window: RawWindowData, width: u32, height: u32) -> Result<Self> {
        // SAFETY: forwarded from this function's own contract.
        pill_core::platform::futures::block_on(unsafe { Self::new_async(window, width, height) })
    }

    /// Creates the renderer: device and surface, the default material, and the
    /// camera.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when a creation step fails: surface or
    /// adapter acquisition, device creation, surface configuration, the depth
    /// buffer, the resource storage, or the default material.
    ///
    /// # Safety
    ///
    /// `window` must name a live window, and that window must outlive the
    /// renderer: the surface is built on its raw handles and nothing here keeps
    /// it alive. A frontend holds the window for as long as it holds the
    /// renderer, and drops the renderer first.
    pub async unsafe fn new_async(window: RawWindowData, width: u32, height: u32) -> Result<Self> {
        info!(target: telemetry_target::RENDERING, "initializing the renderer");
        // Step 1: Build the shared state and the primary surface's target.
        // SAFETY: forwarded from this function's own contract.
        let (mut state, primary) = unsafe { State::new(window, width, height) }.await?;
        // Step 2: Build the cache that keeps the GPU's copy of each asset
        // current with the asset store.
        let rendering_resources_manager = RenderingResourcesManager::new(&mut state)?;
        Ok(Self {
            state,
            rendering_resources_manager,
            metrics: RenderMetrics::default(),
            queued_instances: Vec::new(),
            surface_targets: BTreeMap::from([(primary.id.0, primary)]),
        })
    }
}

impl Drop for Renderer {
    /// Wait for the GPU to finish before the renderer's objects are released.
    ///
    /// A renderer is dropped when its module reloads, while the host keeps
    /// running: work still in flight would otherwise reference buffers and
    /// pipelines that are being destroyed, and the surface would be released
    /// under a frame the driver has not presented yet.
    fn drop(&mut self) {
        if let Err(error) = self.state.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        }) {
            pill_core::warn!(
                target: telemetry_target::HOT_RELOAD,
                "the GPU did not go idle before the renderer was released: {error}"
            );
        }
    }
}

impl PillRenderer for Renderer {
    fn capabilities(&self) -> RenderCapabilities {
        let limits = self.state.device.limits();
        RenderCapabilities {
            max_texture_size: limits.max_texture_dimension_2d,
            max_buffer_bytes: limits.max_buffer_size,
            hdr: false,
            consumed_components: crate::frame::consumed_component_names(),
        }
    }

    fn metrics(&self) -> RenderMetrics {
        self.metrics
    }

    fn render(
        &mut self,
        frame: &RenderFrame,
        assets_manager: &AssetManager,
    ) -> Result<FrameOutcome> {
        // Step 1: Apply the frame's UI textures. They belong to the update,
        // not to a surface, so they are applied here once, before any surface
        // is looked at. A frame that draws nothing keeps them for the next
        // one.
        let state = &mut self.state;
        state
            .ui_renderer
            .apply_texture_set(&state.device, &state.queue, &frame.ui.textures);
        // Step 2: Collect the surfaces that draw this frame, the primary
        // first, each through the camera the frame resolved for it. A surface
        // that is minimized, has no camera, or may not present yet is left
        // out.
        let primary_surface_id = SurfaceId::PRIMARY.0;
        let mut surfaces_to_draw: Vec<(u32, &CameraComponent, &TransformComponent)> =
            Vec::with_capacity(self.surface_targets.len());
        for (id, target) in &self.surface_targets {
            if target.minimized || !target.ready {
                continue;
            }
            // A surface with no camera this frame is not drawn, and its picks
            // are answered as undrawn below.
            let Some(view) = frame.surface_views.get(&SurfaceId(*id)) else {
                continue;
            };
            surfaces_to_draw.push((*id, &view.camera, &view.transform));
        }
        // A pick on a surface not drawn this frame is answered at once: its
        // pixel holds nothing this frame could read back.
        let drawn_surface_ids: Vec<u32> = surfaces_to_draw.iter().map(|(id, _, _)| *id).collect();
        self.state
            .overlay
            .answer_undrawn(&frame.pick_requests, &drawn_surface_ids);
        if drawn_surface_ids.is_empty() {
            // Nothing drew, so no preview can be drawn either - but every
            // request the frame took is answered, or the caller waits for a
            // preview that will never come.
            for request in &frame.preview_requests {
                self.state.preview.refuse(request.id);
            }
            return Ok(FrameOutcome::Skipped);
        }
        // Step 3: Sync the GPU with the asset store, build the frame's queue
        // and instance list, and upload the instances once for every surface.
        let prepare_started_at = Instant::now();
        let sync_zone = pill_core::profile_scope!("renderer: sync assets");
        self.rendering_resources_manager
            .sync(assets_manager, &mut self.state);
        drop(sync_zone);
        let queue_zone = pill_core::profile_scope!("renderer: build queue");
        let render_queue = self
            .rendering_resources_manager
            .build_queue(frame, &self.state);
        drop(queue_zone);
        let instances_zone = pill_core::profile_scope!(
            "renderer: build instances",
            [("{} instances", render_queue.len())]
        );
        crate::instance::build_queued_instances(
            &render_queue,
            &frame.instances,
            &mut self.queued_instances,
        );
        drop(instances_zone);
        // Once for the whole frame, before any surface draws: every geometry
        // pass of every surface reads this one buffer, so a frame that draws N
        // surfaces copies the instance bytes once rather than N times.
        let upload_zone = pill_core::profile_scope!("renderer: upload instances");
        self.state.mesh_drawer.upload_instances(
            &self.state.device,
            &self.state.queue,
            &self.queued_instances,
        );
        drop(upload_zone);
        self.metrics.prepare_micros = prepare_started_at.elapsed().as_micros() as u64;
        self.metrics.instance_bytes = (render_queue.len() * std::mem::size_of::<Instance>()) as u64;

        // Step 4: Plan the primary surface's chain.
        //
        // The frame's chain generation makes reading the chain every frame
        // cheap. The frame resolves the chain only when it changes, and the
        // chain rebuild restarts only when the generation it was built from
        // changes. An empty chain means the game asked for nothing, not that
        // it wants the built-in pass: the code that fills the frame adds that
        // pass to the chain itself.
        //
        // The primary surface's chain is rebuilt and logged on every frame
        // that gets this far, whether or not the primary is one of the drawn
        // surfaces. It is the frame's chain, so both the log line and the
        // metrics describe the frame rather than the window. The chain is
        // never missing, because it is inserted when the renderer is built and
        // detaching it is refused, so the `else` below is unreachable.
        let plan_zone = pill_core::profile_scope!("renderer: plan");
        let Some(primary_target) = self.surface_targets.get_mut(&primary_surface_id) else {
            return Ok(FrameOutcome::Skipped);
        };
        let primary_plan = {
            let chain_context = self.rendering_resources_manager.chain_context(
                &mut self.state,
                primary_target.surface.format(),
                primary_target.surface.size(),
                &primary_target.depth_texture,
                &mut primary_target.offscreen,
            );
            primary_target.chain.ensure(
                &frame.passes,
                assets_manager,
                frame.chain_generation,
                chain_context,
            );
            let plan = primary_target.chain.plan(
                &frame.passes,
                &render_queue,
                self.rendering_resources_manager.shader_handles(),
            );
            primary_target.chain.log(&plan);
            plan
        };
        self.metrics.passes_with_draws = primary_plan
            .iter()
            .filter(|entry| entry.draws() > 0)
            .count() as u32;
        self.metrics.passes = primary_plan.len() as u32;

        drop(plan_zone);

        let submit_started_at = Instant::now();
        let submit_zone = pill_core::profile_scope!("renderer: submit");
        for (id, view_camera, view_transform) in surfaces_to_draw {
            let Some(target) = self.surface_targets.get_mut(&id) else {
                continue;
            };
            // The primary's plan is the one prepared above; every other surface
            // keeps a chain of its own, rebuilt from the same frame here.
            let extra_plan;
            let plan = if id == primary_surface_id {
                primary_plan.as_slice()
            } else {
                let _extra_zone = pill_core::profile_scope!("renderer: extra surface");
                let chain_context = self.rendering_resources_manager.chain_context(
                    &mut self.state,
                    target.surface.format(),
                    target.surface.size(),
                    &target.depth_texture,
                    &mut target.offscreen,
                );
                target.chain.ensure(
                    &frame.passes,
                    assets_manager,
                    frame.chain_generation,
                    chain_context,
                );
                extra_plan = target.chain.plan(
                    &frame.passes,
                    &render_queue,
                    self.rendering_resources_manager.shader_handles(),
                );
                extra_plan.as_slice()
            };
            let surface_render = self.state.render(
                target,
                plan,
                frame,
                view_camera,
                view_transform,
                &render_queue,
            );
            match surface_render {
                Ok(()) => {
                    if !target.presented {
                        target.presented = true;
                        info!(
                            target: telemetry_target::RENDERING,
                            "[render] surface {id} first frame presented"
                        );
                    }
                }
                // One surface failing - a lost swapchain on a window that is
                // closing - never stops the others drawing. The primary's
                // failure ends the frame: without it there is nothing left to
                // draw.
                Err(error) if id == primary_surface_id => return Err(error),
                Err(error) => pill_core::warn!(
                    target: telemetry_target::RENDERING,
                    "surface {id} was not drawn: {error}"
                ),
            }
        }
        drop(submit_zone);
        self.metrics.submit_micros = submit_started_at.elapsed().as_micros() as u64;
        // Step 5: Record one asset preview, when the frame asked for one. It
        // has its own encoder and submission, after the frame's surfaces, so a
        // preview never delays a presented frame.
        record_preview(self, frame, assets_manager);
        Ok(FrameOutcome::Presented)
    }

    fn invalidate_assets(&mut self) {
        // Forget which versions the GPU objects were built from, so the next
        // sync rebuilds them all, and drop the pass objects so the rebuild
        // reaches the bind groups that reference the old ones.
        self.rendering_resources_manager.invalidate();
        for target in self.surface_targets.values_mut() {
            target.chain.invalidate();
        }
    }

    unsafe fn attach_surface(
        &mut self,
        id: SurfaceId,
        window: RawWindowData,
        width: u32,
        height: u32,
        options: SurfaceOptions,
    ) -> Result<()> {
        if id == SurfaceId::PRIMARY {
            return Err(RendererError::UnsupportedSurface {
                detail: "surface 0 is the window the renderer was built on".to_string(),
            });
        }
        // SAFETY: the window outlives the surface, which is this function's
        // own contract with its caller.
        let surface = unsafe {
            Surface::create_additional(
                &self.state.instance,
                &self.state.adapter,
                &self.state.device,
                window,
                width,
                height,
                self.state.surface_format,
                options.present,
            )
        }?;
        let depth_texture = RendererTexture::new_depth_texture(
            &self.state.device,
            surface.configuration(),
            "depth_texture",
        )?;
        let overlay_targets = self
            .state
            .overlay
            .new_targets(&self.state.device, surface.size());
        let camera = self
            .state
            .renderer_resource_storage
            .cameras
            .insert(RendererCamera::new(
                &self.state.device,
                self.state.camera_bind_group_layout.clone(),
            ));
        let target = SurfaceTarget {
            id,
            surface,
            depth_texture,
            offscreen: HashMap::new(),
            chain: GpuChain::new(),
            camera,
            overlay_targets,
            viewport: None,
            minimized: width == 0 || height == 0,
            ready: true,
            presented: false,
        };
        if let Some(replaced_target) = self.surface_targets.insert(id.0, target) {
            self.state
                .renderer_resource_storage
                .cameras
                .remove(replaced_target.camera);
        }
        info!(
            target: telemetry_target::RENDERING,
            "surface {} attached ({width}x{height})", id.0
        );
        Ok(())
    }

    fn detach_surface(&mut self, id: SurfaceId) {
        if id == SurfaceId::PRIMARY {
            // The renderer is built on the primary surface: it goes when the
            // renderer does, never before.
            return;
        }
        let Some(target) = self.surface_targets.remove(&id.0) else {
            return;
        };
        // Wait for the GPU before the swapchain goes: a frame still in flight
        // may be presenting into it.
        if let Err(error) = self.state.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        }) {
            pill_core::warn!(
                target: telemetry_target::RENDERING,
                "the GPU did not go idle before surface {} was released: {error}", id.0
            );
        }
        self.state
            .renderer_resource_storage
            .cameras
            .remove(target.camera);
        drop(target);
        info!(
            target: telemetry_target::RENDERING,
            "surface {} detached", id.0
        );
    }

    fn resize_surface(&mut self, id: SurfaceId, width: u32, height: u32) {
        let Some(target) = self.surface_targets.get_mut(&id.0) else {
            return;
        };
        if let Err(error) = target.resize(&self.state.device, &self.state.overlay, width, height) {
            // Warned instead of panicking: a size the driver will not take is
            // not worth killing a running game over, and the next resize event
            // tries again.
            pill_core::warn!(
                target: telemetry_target::RENDERING,
                "surface {} resize to {width}x{height} failed: {error}", id.0
            );
        }
    }

    fn set_surface_viewport(&mut self, id: SurfaceId, viewport: Option<RenderViewport>) {
        if let Some(target) = self.surface_targets.get_mut(&id.0) {
            target.viewport = viewport;
        }
    }

    fn set_surface_ready(&mut self, id: SurfaceId, ready: bool) {
        if let Some(target) = self.surface_targets.get_mut(&id.0) {
            target.ready = ready;
        }
    }

    fn take_pick_results(&mut self) -> Vec<pill_renderer_api::PickResult> {
        self.state.overlay.take_results(&self.state.device)
    }

    fn take_preview_results(&mut self) -> Vec<pill_renderer_api::PreviewImage> {
        self.state.preview.collect(&self.state.device)
    }
}

// =============================================================================
// State
// =============================================================================

/// The wgpu state every surface shares: the device, the queue, the caches and
/// the painters.
///
/// It lives in a field of [`Renderer`] rather than as loose fields, so the
/// shared GPU objects have one home and the asset caches stay separate.
/// Nothing sized to a single surface lives here; that belongs to
/// [`SurfaceTarget`], one per surface.
pub struct State {
    /// The per-type cache of every GPU resource - shaders, textures, meshes,
    /// materials and cameras - and the engine parameters table.
    pub(crate) renderer_resource_storage: RendererResourceStorage,
    /// The colour format every pipeline that renders to a surface declares.
    ///
    /// All surfaces use one format. A surface attached later is refused if it
    /// cannot be drawn in this format, so each asset's GPU object can be built
    /// once, whatever surfaces exist.
    pub(crate) surface_format: wgpu::TextureFormat,
    /// The device every GPU object in this module is created from.
    pub(crate) device: wgpu::Device,
    /// The queue every upload and frame submission goes through.
    pub(crate) queue: wgpu::Queue,
    /// Format of the depth buffer each surface owns, and of every geometry
    /// pass's depth attachment.
    pub(crate) depth_format: wgpu::TextureFormat,
    /// Records the geometry passes and holds the instance buffer they read.
    mesh_drawer: MeshDrawer,
    /// GPU timestamps and pipeline statistics, when `PILL_GPU_PROFILE` asked
    /// for them and the device has the features.
    gpu_profiler: Option<Profiler>,
    /// Frames rendered since the GPU profiler last logged its numbers.
    frames_since_gpu_report: u32,
    /// Layout every camera bind group is built from.
    pub(crate) camera_bind_group_layout: wgpu::BindGroupLayout,
    /// The instance the surfaces were created from; a surface attached later
    /// needs it.
    pub(crate) instance: wgpu::Instance,
    /// The adapter the device runs on; a surface attached later is checked
    /// against it.
    pub(crate) adapter: wgpu::Adapter,
    /// Debug drawing, outlines and picking, drawn over every surface's chain.
    pub(crate) overlay: Overlay,
    /// The asset preview path: one small off-screen render per request, read
    /// back between frames.
    pub(crate) preview: crate::preview::PreviewRenderer,
    /// The UI painter: the game's UI and the debug labels, over every
    /// surface's overlay. Its pipeline is tied to the surfaces' colour format,
    /// and the debug font is registered in it from the start.
    pub(crate) ui_renderer: UiRenderer,
    /// UI frames since the UI pass last logged what it drew, so the log
    /// stays readable while the end-to-end suites still see the token.
    frames_since_ui_report: u32,
}

impl State {
    /// The shared state, beside the primary surface's target.
    ///
    /// The surface is one of the surfaces the state draws rather than part of
    /// its own state, so it comes back as a [`SurfaceTarget`] like every
    /// surface attached later.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when the surface or device cannot be
    /// created, the depth buffer cannot be built, the camera bind group layout
    /// is refused, or the overlay or the resource storage cannot be created.
    ///
    /// # Safety
    ///
    /// As [`Surface::create`]: `window` must outlive the state.
    async unsafe fn new(
        window: RawWindowData,
        width: u32,
        height: u32,
    ) -> Result<(Self, SurfaceTarget)> {
        // Step 1: Build the surface and the device, queue, instance and
        // adapter it needs.
        // SAFETY: forwarded from this function's own contract.
        let (surface, device, queue, instance, adapter) =
            unsafe { Surface::create(window, width, height) }.await?;
        // Step 2: Record the formats, and build the depth buffer and the
        // camera bind group layout every pass shares.
        // One definition for both sides of the contract: the attachment is
        // created with this format and every pipeline that renders into it
        // declares it, so the constant is the only place it is written.
        let depth_format = RendererTexture::DEPTH_FORMAT;
        let surface_format = surface.format();
        let depth_texture =
            RendererTexture::new_depth_texture(&device, surface.configuration(), "depth_texture")?;
        // Run inside validation capture, like every other creation here:
        // without it, a refused layout would reach the uncaptured-error
        // handler and take the host down before a `RendererError` exists to
        // explain it.
        let camera_bind_group_layout = capturing_validation(&device, || {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("camera_parameters_bind_group_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            })
        })
        .map_err(|detail| RendererError::Other {
            detail: format!("camera bind group layout: {detail}"),
        })?;
        // Step 3: Create the shared painters, caches and profiler.
        let overlay = Overlay::new(
            &device,
            surface_format,
            depth_format,
            &camera_bind_group_layout,
        )?;
        let mut ui_renderer = UiRenderer::new(&device, surface_format);
        ui_renderer.apply_texture_set(&device, &queue, &overlay::font::texture_set());
        let mut renderer_resource_storage = RendererResourceStorage::new(&device, &queue)?;
        let mesh_drawer = MeshDrawer::new(&device, INSTANCE_BATCH_SIZE);
        let gpu_profiler = create_gpu_profiler(&device, &queue, surface.backend());
        let preview = crate::preview::PreviewRenderer::new(&device);
        let overlay_targets = overlay.new_targets(&device, surface.size());
        let camera = renderer_resource_storage
            .cameras
            .insert(RendererCamera::new(
                &device,
                camera_bind_group_layout.clone(),
            ));
        // Step 4: Build the primary surface's target and the state that draws
        // it.
        let primary = SurfaceTarget {
            id: SurfaceId::PRIMARY,
            surface,
            depth_texture,
            offscreen: HashMap::new(),
            chain: GpuChain::new(),
            camera,
            overlay_targets,
            viewport: None,
            minimized: width == 0 || height == 0,
            ready: true,
            presented: true,
        };
        let state = Self {
            renderer_resource_storage,
            surface_format,
            device,
            queue,
            depth_format,
            mesh_drawer,
            gpu_profiler,
            frames_since_gpu_report: 0,
            camera_bind_group_layout,
            instance,
            adapter,
            overlay,
            preview,
            ui_renderer,
            frames_since_ui_report: 0,
        };
        Ok((state, primary))
    }

    /// Draw the debug labels and the frame's UI for `surface_id`.
    ///
    /// The labels go first, in surface pixels, then the game's lists that name
    /// `surface_id`, so the game's UI stays on top. Every surface records
    /// through this same path with its own id, and all of a surface's lists go
    /// to the painter in one call. [`Renderer::render`] applied the frame's
    /// textures before this ran, so nothing here uploads.
    fn record_ui(
        &mut self,
        surface_id: u32,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size: (u32, u32),
        frame: &RenderFrame,
        labels: &pill_ui_frame::DrawList,
    ) {
        let mut draw_lists = vec![(labels, 1.0)];
        draw_lists.extend(
            frame
                .ui
                .draws
                .iter()
                .filter(|draw| draw.surface == surface_id)
                .map(|draw| (&draw.list, draw.scale_factor)),
        );
        self.ui_renderer
            .render(&self.device, &self.queue, encoder, view, size, &draw_lists);
        if frame.ui.is_empty() {
            return;
        }
        // Observable proof for the runtime suites, sampled like the GPU
        // profiler's report so the log stays readable. It counts the game's
        // lists only; the debug labels are not part of the count.
        let draw_count = draw_lists.len() - 1;
        let vertex_count: usize = draw_lists[1..]
            .iter()
            .map(|(list, _)| list.vertices.len())
            .sum();
        self.frames_since_ui_report += 1;
        if self.frames_since_ui_report >= GPU_REPORT_INTERVAL {
            self.frames_since_ui_report = 0;
            info!(
                target: telemetry_target::UI,
                "[ui] surface={} draws={} vertices={}",
                surface_id,
                draw_count,
                vertex_count
            );
        }
    }

    /// Draw one surface's frame into `target`: the chain's passes, the overlay,
    /// the UI, and the present.
    ///
    /// The target is handed in rather than swapped into the state's own fields,
    /// so a per-surface field has one home whether the surface is the primary
    /// one or one attached later.
    ///
    /// # Errors
    ///
    /// Returns the error when the surface's image cannot be acquired, when the
    /// device was lost, or when wgpu refuses the frame's commands at
    /// submission.
    fn render(
        &mut self,
        target: &mut SurfaceTarget,
        plan: &[PassPlan],
        frame: &RenderFrame,
        camera: &CameraComponent,
        transform: &TransformComponent,
        render_queue: &[RenderQueueItem],
    ) -> Result<()> {
        // Step 1: Acquire the image and prepare everything the passes read.
        let draw = self.begin_surface_frame(target, frame, camera, transform, render_queue)?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render_encoder"),
            });
        // Step 2: Record one wgpu render pass per pass of the chain, into the
        // same encoder. The first pass clears the targets it writes. The rest
        // add to what is there.
        for entry in plan {
            self.record_pass(&mut encoder, entry, target, &draw);
        }
        // Step 3: Draw the overlay and the UI over the chain's picture.
        self.record_overlay_and_ui(&mut encoder, target, &draw);
        // Step 4: Resolve the profiler's queries, submit the commands and
        // present the image.
        self.finish_surface_frame(encoder, draw)
    }

    /// Acquire one surface's image and prepare everything the passes read.
    ///
    /// The device-loss check, the swapchain acquire, the engine parameters, the
    /// camera the surface is drawn through and the profiler's frame all happen
    /// here, so the recording phases only record.
    ///
    /// # Errors
    ///
    /// Returns the error when the device was lost, or when the surface's image
    /// cannot be acquired.
    fn begin_surface_frame<'a>(
        &mut self,
        target: &mut SurfaceTarget,
        frame: &'a RenderFrame,
        camera: &CameraComponent,
        transform: &TransformComponent,
        render_queue: &'a [RenderQueueItem],
    ) -> Result<SurfaceDraw<'a>> {
        // Step 1: Refuse the frame when the device was lost. A lost device
        // draws nothing and reports nothing, so the loss the callback latched
        // is the frame path's only chance to say so. Returning it here ends
        // the run with a reason instead of a frozen picture.
        if let Some(detail) = crate::error::device_loss() {
            return Err(RendererError::Other { detail });
        }
        // Step 2: Acquire the image and make the view the passes draw into.
        // The surface and the device are borrowed separately, so the surface
        // can reconfigure itself against the device that created it.
        let acquire_zone = pill_core::profile_scope!("renderer: acquire surface");
        let image = target.surface.acquire(&self.device)?;
        drop(acquire_zone);
        let size = target.surface.size();
        // Through the sRGB view the pipelines were built for, which is not the
        // texture's own format on a surface without sRGB formats.
        let view = image.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(target.surface.format()),
            ..Default::default()
        });
        // Step 3: Refresh the parameters and the camera this frame draws with,
        // and open the profiler's frame.
        self.renderer_resource_storage.engine_parameters.update(
            &self.queue,
            NO_FOG_DENSITY,
            NO_FOG_COLOR,
            [size.0, size.1],
            [
                frame.seconds,
                frame.delta_seconds,
                frame.sequence as f32,
                0.0,
            ],
        );
        let viewport = target
            .viewport
            .and_then(|requested_viewport| requested_viewport.clamped_to(size.0, size.1))
            .unwrap_or_else(|| RenderViewport::full(size.0, size.1));
        let camera_handle = target.camera;
        let renderer_camera = self
            .renderer_resource_storage
            .cameras
            .get_mut(camera_handle)
            .ok_or(RendererError::RendererResourceNotFound)?;
        renderer_camera.update(
            &self.queue,
            camera,
            transform,
            viewport.width as f32 / viewport.height as f32,
        );
        if let Some(profiler) = &mut self.gpu_profiler {
            profiler.begin_frame(&self.device, &self.queue);
        }
        Ok(SurfaceDraw {
            surface_id: target.id.0,
            image,
            view,
            size,
            viewport,
            camera: camera_handle,
            frame,
            render_queue,
        })
    }

    /// Record one pass of the chain into `encoder`.
    ///
    /// A pass whose target or camera is gone records nothing: the chain log
    /// already named the pass that could not be built.
    fn record_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        entry: &PassPlan,
        target: &SurfaceTarget,
        draw: &SurfaceDraw,
    ) {
        let _pass_zone = pill_core::profile_scope!(
            "renderer pass: {}",
            entry.label();
            [("{} draws", entry.draws())]
        );
        let Some(color_attachments) = self.pass_color_attachments(entry, target, draw) else {
            return;
        };
        let clear = entry.clears();
        // Whether a GPU timestamp was written before the pass, so the one after
        // it closes a pair.
        let timed = match entry {
            PassPlan::Geometry { .. } => {
                self.record_geometry_pass(encoder, entry, target, draw, &color_attachments, clear)
            }
            PassPlan::Fullscreen { .. } => {
                self.record_fullscreen_pass(encoder, entry, target, draw, &color_attachments, clear)
            }
        };
        if timed {
            write_gpu_timestamp(&mut self.gpu_profiler, encoder, entry.label());
        }
    }

    /// The colour attachments a pass writes, in the order the shader's
    /// `SV_TARGET` list names them.
    ///
    /// `None` when one of them was never created: the pass then draws nothing,
    /// because drawing into the remaining targets would leave a picture the
    /// next pass cannot read.
    fn pass_color_attachments<'a>(
        &self,
        entry: &PassPlan,
        target: &'a SurfaceTarget,
        draw: &'a SurfaceDraw,
    ) -> Option<Vec<Option<wgpu::RenderPassColorAttachment<'a>>>> {
        let (width, height) = draw.size;
        let viewport = draw.viewport;
        let mut views = Vec::with_capacity(entry.outputs().len());
        for output in entry.outputs() {
            match output {
                PassOutput::Surface => views.push(&draw.view),
                PassOutput::Offscreen(name) => {
                    views.push(&target.offscreen.get(*name)?.texture_view);
                }
            }
        }
        // `LoadOp::Clear` ignores the scissor, so an opaque colour here repaints
        // the whole swapchain image whatever the viewport says. When the frame
        // owns only part of the target, that would paint over everything else
        // sharing the window, once per frame. A transparent clear keeps those
        // pixels, and it is used only where the surface can actually carry
        // alpha. In an `Opaque` mode the same clear would be opaque black
        // instead.
        let shared_target = !covers_target(viewport, width, height) && target.surface.composites();
        let clear = entry.clears();
        Some(
            entry
                .outputs()
                .iter()
                .zip(views.iter())
                .map(|(output, view)| {
                    let load = match output {
                        PassOutput::Surface if shared_target => {
                            wgpu::LoadOp::Clear(CLEAR_TRANSPARENT)
                        }
                        _ => color_load(clear),
                    };
                    Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })
                })
                .collect(),
        )
    }

    /// Record a geometry pass: the instances it was handed, through the
    /// pipeline it built or through each instance's own material.
    ///
    /// Returns whether a GPU timestamp was written before the pass.
    fn record_geometry_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        entry: &PassPlan,
        target: &SurfaceTarget,
        draw: &SurfaceDraw,
        color_attachments: &[Option<wgpu::RenderPassColorAttachment>],
        clear: bool,
    ) -> bool {
        let PassPlan::Geometry {
            label,
            queue,
            ranges,
            pass_index,
            ..
        } = entry
        else {
            return false;
        };
        let Some(camera) = self.renderer_resource_storage.cameras.get(draw.camera) else {
            return false;
        };
        let timed = write_gpu_timestamp(&mut self.gpu_profiler, encoder, label);
        // A pass that built a pipeline draws through it: the pipeline holds the
        // targets' formats and the pass's depth and culling, none of which an
        // instance may choose.
        let pass_pipeline = pass_index.and_then(|index| match target.chain.passes().get(index) {
            Some(PassSlot::Drawable(pass)) => Some(&pass.pipeline),
            _ => None,
        });
        self.mesh_drawer.record_draw_commands(
            encoder,
            &self.renderer_resource_storage,
            label,
            pass_pipeline,
            color_attachments,
            depth_attachment(&target.depth_texture.texture_view, clear),
            camera,
            queue,
            ranges,
            draw.viewport,
            self.gpu_profiler.as_ref(),
        );
        timed
    }

    /// Record a fullscreen pass: its single triangle over the whole target,
    /// clipped to the frame's rectangle.
    ///
    /// Returns whether a GPU timestamp was written before the pass.
    fn record_fullscreen_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        entry: &PassPlan,
        target: &SurfaceTarget,
        draw: &SurfaceDraw,
        color_attachments: &[Option<wgpu::RenderPassColorAttachment>],
        clear: bool,
    ) -> bool {
        let PassPlan::Fullscreen {
            label,
            pass_index,
            tests_depth,
            ..
        } = entry
        else {
            return false;
        };
        let Some(PassSlot::Drawable(pass)) = target.chain.passes().get(*pass_index) else {
            return false;
        };
        let Some(camera) = self.renderer_resource_storage.cameras.get(draw.camera) else {
            return false;
        };
        let timed = write_gpu_timestamp(&mut self.gpu_profiler, encoder, label);
        // A post-processing step has no depth: it overwrites every pixel it
        // covers, and the depth left behind describes geometry that is not what
        // this triangle is. A skybox reads that depth without writing it, so it
        // fills only what no mesh covered; as the frame's first pass it opens
        // the buffer at the far plane like a geometry pass would.
        let depth_stencil_attachment =
            tests_depth.then(|| depth_attachment(&target.depth_texture.texture_view, clear));
        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments,
            depth_stencil_attachment,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        render_pass.set_pipeline(&pass.pipeline);
        // A fullscreen pass covers its whole target unless it is clipped, and
        // the post-processing stages - the ones that write the swapchain -
        // would otherwise spread the scene over the entire window instead of
        // the rectangle the geometry passes drew into.
        //
        // Only the scissor moves. The viewport has to stay the whole target,
        // because this pass derives its texture coordinates from its own
        // position in NDC: mapping NDC onto a sub-rectangle would stretch the
        // source into it rather than clip the picture to it. The scissor is
        // expressed in this pass's own target, which the chain may have scaled
        // down.
        let (width, height) = draw.size;
        let (target_width, target_height) = match entry.outputs().first() {
            Some(PassOutput::Offscreen(name)) => {
                target
                    .offscreen
                    .get(*name)
                    .map_or((width, height), |texture| {
                        let extent = texture.texture.size();
                        (extent.width, extent.height)
                    })
            }
            _ => (width, height),
        };
        let scissor = viewport_in_target(draw.viewport, width, height, target_width, target_height);
        render_pass.set_scissor_rect(scissor.x, scissor.y, scissor.width, scissor.height);
        if pass.reads_engine_parameters {
            render_pass.set_bind_group(
                ENGINE_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
                &self.renderer_resource_storage.engine_parameters.bind_group,
                &[],
            );
        }
        if pass.reads_camera_parameters {
            render_pass.set_bind_group(
                CAMERA_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
                &camera.bind_group,
                &[],
            );
        }
        if let Some(bind_group) = &pass.parameters_bind_group {
            render_pass.set_bind_group(
                MATERIAL_PARAMETERS_BIND_GROUP_LAYOUT_INDEX,
                bind_group,
                &[],
            );
        }
        if let Some(bind_group) = &pass.textures_bind_group {
            render_pass.set_bind_group(MATERIAL_TEXTURES_BIND_GROUP_LAYOUT_INDEX, bind_group, &[]);
        }
        render_pass.draw(0..3, 0..1);
        timed
    }

    /// Record the overlay and the UI over the chain's picture.
    fn record_overlay_and_ui(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &mut SurfaceTarget,
        draw: &SurfaceDraw,
    ) {
        let surface_id = draw.surface_id;
        // Over the chain's picture: outlines, debug lines and labels, gizmos,
        // and the pick buffer when this surface has picks waiting.
        let overlay_zone = pill_core::profile_scope!("renderer: overlay");
        let Some(camera) = self.renderer_resource_storage.cameras.get(draw.camera) else {
            return;
        };
        let labels = overlay::label_list(
            &draw.frame.debug,
            &camera.parameters_data.view_projection_matrix,
            draw.viewport,
        );
        self.overlay.record(
            &self.device,
            &self.queue,
            encoder,
            &mut target.overlay_targets,
            OverlayFrame {
                surface: surface_id,
                color_view: &draw.view,
                depth_view: &target.depth_texture.texture_view,
                viewport: draw.viewport,
                camera,
                frame: draw.frame,
                render_queue: draw.render_queue,
                instance_buffer: self.mesh_drawer.instance_buffer(),
                storage: &self.renderer_resource_storage,
            },
        );
        drop(overlay_zone);
        // The debug labels and the game's UI, over the chain's picture,
        // through the [`ui`](crate::ui) painter.
        self.record_ui(
            surface_id, encoder, &draw.view, draw.size, draw.frame, &labels,
        );
    }

    /// Resolve the profiler's queries, submit the frame's commands and present
    /// the image.
    ///
    /// # Errors
    ///
    /// Returns the error when wgpu refuses the frame's command buffer at
    /// submission.
    fn finish_surface_frame(
        &mut self,
        mut encoder: wgpu::CommandEncoder,
        draw: SurfaceDraw,
    ) -> Result<()> {
        // Step 1: Resolve the profiler's queries into the frame it closes.
        if let Some(profiler) = &mut self.gpu_profiler {
            profiler.resolve_timestamp_queries(&self.device, &mut encoder);
            profiler.resolve_pipeline_statistics_queries(&self.device, &mut encoder);
            profiler.resolve_occlusion_queries(&self.device, &mut encoder);
        }
        // Step 2: Submit the frame's commands. This is the last creation-class
        // failure a frame can hit. wgpu validates a command buffer when it is
        // submitted, and a validation failure would otherwise reach the
        // uncaptured-error handler as a panic.
        let queue_submit_zone = pill_core::profile_scope!("renderer: queue submit");
        capturing_validation(&self.device, || {
            self.queue.submit(std::iter::once(encoder.finish()));
        })
        .map_err(|detail| RendererError::Other {
            detail: format!("frame submission: {detail}"),
        })?;
        drop(queue_submit_zone);
        // Step 3: Hand the image to the surface, then close the profiler's
        // frame and log its numbers when this is a report frame.
        self.overlay.after_submit();
        let present_zone = pill_core::profile_scope!("renderer: present");
        self.queue.present(draw.image);
        drop(present_zone);
        if let Some(profiler) = &mut self.gpu_profiler {
            profiler.end_frame();
            self.frames_since_gpu_report += 1;
            if self.frames_since_gpu_report >= GPU_REPORT_INTERVAL {
                self.frames_since_gpu_report = 0;
                profiler
                    .log_latest(u64::from(draw.viewport.width) * u64::from(draw.viewport.height));
            }
        }
        Ok(())
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Draw one asset preview, when the frame asked for one and the previous
/// preview has been handed over.
///
/// A texture is drawn from its file, which the preview path decodes and
/// uploads on first use. A material is drawn as a sphere in the first colour
/// parameter of its parameter map, and a mesh from its own file through the
/// preview's mesh pipeline.
///
/// Every request the frame took is answered: the first is drawn if the
/// renderer is free, and anything not drawn is refused. A caller that waits
/// for an answer that never comes stops asking.
///
/// The preview has its own encoder and submission, after the frame's surfaces,
/// so it never delays a presented frame.
fn record_preview(renderer: &mut Renderer, frame: &RenderFrame, assets_manager: &AssetManager) {
    let mut drawable = renderer.state.preview.ready();
    for request in &frame.preview_requests {
        if drawable {
            drawable = false;
            if let Err(reason) = draw_preview(renderer, request, assets_manager) {
                pill_core::info!(
                    target: telemetry_target::RENDERING,
                    id = request.id,
                    reason,
                    "[render] preview not drawn"
                );
            } else {
                continue;
            }
        }
        renderer.state.preview.refuse(request.id);
    }
}

/// Draw one preview, naming what stopped it when it cannot be drawn.
///
/// # Errors
///
/// Returns the reason as text when the asset's file cannot be decoded.
fn draw_preview(
    renderer: &mut Renderer,
    request: &pill_renderer_api::PreviewRequest,
    assets_manager: &AssetManager,
) -> std::result::Result<(), &'static str> {
    // What the fragment shader draws, and the source it samples: a texture
    // file decoded here, or the preview's own white texture for a material.
    let (mode, color, source) = match &request.asset {
        pill_renderer_api::PreviewAsset::Texture { path } => {
            let source = renderer
                .state
                .preview
                .texture(&renderer.state.device, &renderer.state.queue, path)
                .ok_or("the texture file could not be decoded")?;
            (
                crate::preview::PreviewMode::Texture,
                [1.0f32; 4],
                Some(source),
            )
        }
        pill_renderer_api::PreviewAsset::Material { guid } => {
            let color = pill_engine::AssetGuid::parse(guid)
                .and_then(|guid| {
                    assets_manager
                        .get_by_guid::<crate::assets::Material>(guid)
                        .and_then(|material| {
                            material
                                .parameters
                                .values()
                                .find_map(|parameter| match parameter {
                                    pill_renderer_api::MaterialParameter::Color(color) => {
                                        Some([color[0], color[1], color[2], 1.0])
                                    }
                                    _ => None,
                                })
                        })
                })
                .unwrap_or([0.72, 0.72, 0.72, 1.0]);
            (crate::preview::PreviewMode::Material, color, None)
        }
        pill_renderer_api::PreviewAsset::Mesh { path } => {
            let mesh = renderer
                .state
                .preview
                .mesh(&renderer.state.device, &renderer.state.queue, path)
                .ok_or("the mesh file could not be decoded")?;
            let mut encoder =
                renderer
                    .state
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("preview_mesh_encoder"),
                    });
            renderer.state.preview.record_mesh(
                &renderer.state.device,
                &renderer.state.queue,
                &mut encoder,
                request.id,
                &mesh,
            );
            renderer
                .state
                .queue
                .submit(std::iter::once(encoder.finish()));
            renderer.state.preview.after_submit();
            return Ok(());
        }
    };
    let mut encoder =
        renderer
            .state
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("preview_encoder"),
            });
    renderer.state.preview.record(
        &renderer.state.device,
        &renderer.state.queue,
        &mut encoder,
        request.id,
        mode,
        color,
        source,
    );
    renderer
        .state
        .queue
        .submit(std::iter::once(encoder.finish()));
    renderer.state.preview.after_submit();
    Ok(())
}

/// The GPU profiler `PILL_GPU_PROFILE` asks for, or `None` when it was not
/// asked for or the device lacks the features.
fn create_gpu_profiler(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    backend: wgpu::Backend,
) -> Option<Profiler> {
    if !crate::profiler::gpu_profiling_requested() {
        return None;
    }
    if !device
        .features()
        .contains(crate::profiler::GPU_PROFILE_FEATURES)
    {
        pill_core::warn!(
            target: telemetry_target::RENDERING,
            "PILL_GPU_PROFILE is set, but this device has no timestamp or pipeline statistics queries"
        );
        return None;
    }
    pill_core::info!(
        target: telemetry_target::RENDERING,
        "GPU profiling on: pass timings every {GPU_REPORT_INTERVAL} frames"
    );
    let mut profiler = Profiler::new(
        device,
        queue,
        // A timestamp before and after each pass.
        64,
        // One per geometry pass.
        8,
        wgpu::PipelineStatisticsTypes::VERTEX_SHADER_INVOCATIONS
            | wgpu::PipelineStatisticsTypes::CLIPPER_INVOCATIONS
            | wgpu::PipelineStatisticsTypes::CLIPPER_PRIMITIVES_OUT
            | wgpu::PipelineStatisticsTypes::FRAGMENT_SHADER_INVOCATIONS,
        // One occlusion query per geometry pass: its visible coverage.
        8,
    );
    // The pass timings also go to Tracy, on a GPU track of their own.
    let gpu_api = match backend {
        wgpu::Backend::Vulkan => pill_core::profiling::GpuApi::Vulkan,
        wgpu::Backend::Dx12 => pill_core::profiling::GpuApi::Direct3D12,
        wgpu::Backend::Gl => pill_core::profiling::GpuApi::OpenGL,
        _ => pill_core::profiling::GpuApi::Other,
    };
    profiler.attach_timeline(gpu_api);
    Some(profiler)
}

/// Writes a GPU timestamp named `label` between passes, when profiling.
/// Returns whether one was written.
fn write_gpu_timestamp(
    profiler: &mut Option<Profiler>,
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
) -> bool {
    profiler
        .as_mut()
        .and_then(|profiler| profiler.write_timestamp(encoder, label))
        .is_some()
}

/// The depth attachment a geometry pass writes, or a skybox reads.
///
/// A pass that opens the frame's targets clears it to the far plane, so the
/// oldest geometry is drawn wherever nothing has written yet; every later pass
/// keeps what is there.
fn depth_attachment(
    view: &wgpu::TextureView,
    clear: bool,
) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations {
            load: if clear {
                wgpu::LoadOp::Clear(1.0)
            } else {
                wgpu::LoadOp::Load
            },
            store: wgpu::StoreOp::Store,
        }),
        stencil_ops: None,
    }
}

/// The colour a pass opens its target with, or the values already in it.
fn color_load(clear: bool) -> wgpu::LoadOp<wgpu::Color> {
    if clear {
        wgpu::LoadOp::Clear(CLEAR_COLOR)
    } else {
        wgpu::LoadOp::Load
    }
}

/// Whether a frame's viewport covers every pixel of a `width` by `height` target.
///
/// Only a frame that covers the target owns it. One that does not shares the
/// window with another painter and has to leave the rest of it alone - which is
/// why every pass is clipped to the frame's rectangle, and why the swapchain
/// image is opened transparent rather than in [`CLEAR_COLOR`].
fn covers_target(viewport: RenderViewport, width: u32, height: u32) -> bool {
    viewport.x == 0 && viewport.y == 0 && viewport.width >= width && viewport.height >= height
}

/// The frame's viewport expressed in the pixels of one pass's target.
///
/// A pass may declare a target the chain scaled - the bloom prefilter writes
/// half the surface - and wgpu rejects a scissor that leaves the attachment it
/// is set on. Scaling the rectangle keeps the clip in the same place, because a
/// scaled target holds the same picture at a different resolution rather than a
/// different part of it.
fn viewport_in_target(
    viewport: RenderViewport,
    surface_width: u32,
    surface_height: u32,
    target_width: u32,
    target_height: u32,
) -> RenderViewport {
    if target_width == surface_width && target_height == surface_height {
        return viewport;
    }
    let scale = |length: u32, surface_extent: u32, target_extent: u32| -> u32 {
        ((u64::from(length) * u64::from(target_extent)) / u64::from(surface_extent.max(1))) as u32
    };
    let x = scale(viewport.x, surface_width, target_width).min(target_width.saturating_sub(1));
    let y = scale(viewport.y, surface_height, target_height).min(target_height.saturating_sub(1));
    let right = scale(viewport.x + viewport.width, surface_width, target_width);
    let bottom = scale(viewport.y + viewport.height, surface_height, target_height);
    // At least one pixel each way: an empty rectangle is not a valid scissor.
    RenderViewport::new(
        x,
        y,
        right.saturating_sub(x).max(1).min(target_width - x),
        bottom.saturating_sub(y).max(1).min(target_height - y),
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_viewport_in_a_scaled_target_is_scaled_with_it() {
        // Half the surface, which is the bloom prefilter's target.
        assert_eq!(
            viewport_in_target(RenderViewport::new(200, 100, 400, 200), 1280, 800, 640, 400),
            RenderViewport::new(100, 50, 200, 100)
        );

        // A rectangle that would leave the target is pulled back inside it: wgpu
        // rejects a scissor that hangs over the edge, which fails the frame's
        // submission instead of clipping the draw.
        let clipped =
            viewport_in_target(RenderViewport::new(900, 600, 300, 200), 1280, 800, 640, 400);
        assert!(clipped.x + clipped.width <= 640, "{clipped:?}");
        assert!(clipped.y + clipped.height <= 400, "{clipped:?}");
    }

    #[test]
    fn a_viewport_on_the_surface_is_left_alone() {
        let viewport = RenderViewport::new(200, 100, 400, 200);
        assert_eq!(viewport_in_target(viewport, 1280, 800, 1280, 800), viewport);
    }

    #[test]
    fn only_a_frame_covering_the_target_owns_it() {
        assert!(covers_target(RenderViewport::full(1280, 800), 1280, 800));
        assert!(!covers_target(
            RenderViewport::new(0, 0, 700, 800),
            1280,
            800
        ));
    }

    /// The instance buffer is uploaded once per frame, not once per surface:
    /// `State::render` runs for the primary surface and again for every extra
    /// one, all with the same queue.
    ///
    /// The upload needs a device, so this reads the frame path instead: one
    /// upload call, in `Renderer::render`, and no instance slice threaded into
    /// the per-surface path at all.
    #[test]
    fn the_instance_buffer_is_uploaded_once_per_frame() {
        let source = include_str!("renderer.rs");
        assert_eq!(
            source.matches(concat!("upload_", "instances(")).count(),
            1,
            "a frame draws N surfaces from one instance buffer, so it uploads it once"
        );
        assert!(
            !source.contains(concat!("(inst", "ances, render", "_queue)")),
            "the per-surface path takes the queue only, not the instances"
        );
    }
}
