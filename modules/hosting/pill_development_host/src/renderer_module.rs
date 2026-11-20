//! The GPU renderer as a loaded module: starting it, and driving the backend
//! it builds on a window.
//!
//! # Responsibilities
//!
//! - Build, load, initialize and reload the renderer module with the same
//!   machinery as an extension, under an owner of its own
//!   ([`RendererModule`]).
//! - Attach it to a window through its `pill_renderer_attach` export, and wrap
//!   the backend that export returns in [`ModuleRenderer`].
//!
//! # Design
//!
//! The host links no renderer: wgpu exists only inside the module. The module
//! returns the backend as a `Box<Box<dyn PillRenderer>>` behind a raw pointer.
//! [`ModuleRenderer`] calls through that pointer and, when dropped, hands it
//! back to the module's `pill_renderer_detach`, so the renderer is freed by the
//! image that allocated it while that image is still mapped. A `ModuleRenderer`
//! must therefore be dropped before the module's slot, and `RenderingHost`
//! declares its fields in that order.
//!
//! The module is kept out of the host's extension list. That list reloads its
//! members on a source change, and a renderer reload has to detach the backend
//! first. `RenderingHost` reloads it instead: it detaches through the reload's
//! `before_commit` hook and attaches whichever generation is current after.
//!
//! A failed load never costs the scene. The module first loads when the window
//! opens, after the project has filled the world, so it loads under
//! [`FirstLoadFailure::ClearSystemsOnly`]: a refused generation clears its own
//! systems, stays mapped, and must have registered no data.

// Standard library
use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// External crates
use pill_core::error::HostError;
use pill_core::telemetry::telemetry_target;
use pill_core::{error, info};
use pill_engine::{AssetManager, Engine, EngineApi, SystemOwner};
use pill_renderer_api::{
    FrameOutcome, PillRenderer, RawWindowData, RenderCapabilities, RenderFrame, RenderMetrics,
    RenderViewport, RendererError,
};

// Current crate
use crate::config::ExtensionConfig;
use crate::extension::{ExtensionSlot, ReloadOutcome};
use crate::reload::FirstLoadFailure;

// =============================================================================
// Constants
// =============================================================================

/// Export that builds a backend on a window.
const ATTACH_SYMBOL: &[u8] = b"pill_renderer_attach";

/// Export that drops a backend inside the module.
const DETACH_SYMBOL: &[u8] = b"pill_renderer_detach";

// =============================================================================
// Types
// =============================================================================

/// Signature of the `pill_renderer_attach` export: it builds a backend on the
/// window the data describes, or returns null.
type AttachFn = unsafe extern "C" fn(*const RawWindowData, u32, u32) -> *mut c_void;

/// Signature of the `pill_renderer_detach` export: it drops a backend inside
/// the module.
type DetachFn = unsafe extern "C" fn(*mut c_void);

// =============================================================================
// RendererModule
// =============================================================================

/// The renderer module: the slot a loaded generation lives in, and the edit
/// counter its source watcher moves. The host compares that counter to decide
/// when to reload the module or retry a failed first load.
pub(crate) struct RendererModule {
    /// The slot of the loaded generation, or `None` while no generation has
    /// loaded yet.
    slot: Option<ExtensionSlot>,
    /// How to build, watch and output the module, derived from its name (the
    /// project's `renderer:` setting). `None` when the project selects no
    /// renderer, which leaves the module unloaded for the whole run.
    extension_config: Option<ExtensionConfig>,
    /// Shared with the source watcher, which adds one on every save under the
    /// module's `src/`.
    source_edit_generation: Arc<AtomicU64>,
    /// The source-edit generation the last failed first load was tried at. It
    /// makes the host retry a failure once per edit, not once per frame.
    last_failed_source_edit_generation: u64,
    /// The engine owner every generation's systems are registered under.
    system_owner: SystemOwner,
}

impl RendererModule {
    /// Watch the renderer module's sources and try its first load.
    ///
    /// A first load that fails - because it does not build, load, pass the ABI
    /// check or initialize, or because it registers data - is logged, not
    /// returned: the host keeps running with the window blank, and the next
    /// save tries again. The module's `init` registers the `rendering` system
    /// under `system_owner`; the components, assets and resources it draws
    /// belong to `pill_renderer_api` and to the renderer data crate, and other
    /// subjects registered them before it loaded.
    ///
    /// # Errors
    ///
    /// Returns a [`HostError`] only when the configuration is invalid or the
    /// sources cannot be watched. A later renderer edit fixes neither.
    pub(crate) fn start(
        renderer_name: Option<&str>,
        engine: &mut Engine,
        engine_api: &EngineApi,
        workspace_root: &Path,
        system_owner: SystemOwner,
    ) -> Result<Self, HostError> {
        // Step 1: Without a selected renderer there is nothing to watch or
        // load. The edit counter then never moves, so `reload_if_changed`
        // never reloads or retries.
        let Some(renderer_name) = renderer_name else {
            info!(
                target: telemetry_target::HOT_RELOAD,
                "the project selects no renderer; the window stays blank"
            );
            return Ok(Self {
                slot: None,
                extension_config: None,
                source_edit_generation: Arc::new(AtomicU64::new(0)),
                last_failed_source_edit_generation: 0,
                system_owner,
            });
        };
        // Step 2: Build the configuration and start the watcher before the
        // first load, because the watcher is what retries a load that fails
        // below.
        let extension_config =
            ExtensionConfig::located_workspace_member(workspace_root, renderer_name);
        extension_config.validate()?;
        let source_edit_generation = Arc::new(AtomicU64::new(0));
        crate::watchers::spawn_source_watcher(
            workspace_root.to_path_buf(),
            &extension_config.name,
            &extension_config.watch_directory,
            Arc::clone(&source_edit_generation),
            None,
        )?;
        let mut module = Self {
            slot: None,
            extension_config: Some(extension_config),
            source_edit_generation,
            last_failed_source_edit_generation: 0,
            system_owner,
        };
        // Step 3: Try the first generation now. A failure is logged and leaves
        // the module unloaded.
        module.try_first_load(engine, engine_api, workspace_root, 0);
        Ok(module)
    }

    /// The slot of the loaded generation, if one has loaded.
    pub(crate) fn slot(&self) -> Option<&ExtensionSlot> {
        self.slot.as_ref()
    }

    /// The engine owner every generation's systems are registered under.
    pub(crate) fn owner(&self) -> SystemOwner {
        self.system_owner
    }

    /// Ask for a rebuild and reload on the next [`Self::reload_if_changed`], as
    /// a save under the module's sources would.
    ///
    /// Used when the renderer's data crate reloaded. The renderer compiles that
    /// crate into its own image, so it must be rebuilt against the new source
    /// before it may read the data again. A no-op when no renderer is
    /// selected.
    pub(crate) fn request_rebuild(&self) {
        if let Some(extension_config) = &self.extension_config {
            // The startup batch may already have marked the renderer's
            // artifact as built. A rebuild request means the reason to rebuild
            // (a reloaded data crate) came after that batch, so forget the
            // mark; otherwise the next load would skip the compile.
            crate::build_runner::forget_batch_validation(&extension_config.name);
            self.source_edit_generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Reload the module when its sources changed, or retry a first load that
    /// failed.
    ///
    /// `before_commit` is the reload's hook (see
    /// [`ExtensionSlot::reload_if_changed_with`]). A first load has no backend
    /// to detach, so it does not run. `peer_component_names` lists every
    /// component name the other live subjects declare. A type the new
    /// generation forgets is dropped only when no other subject declares it,
    /// so a shared type is kept.
    pub(crate) fn reload_if_changed(
        &mut self,
        engine: &mut Engine,
        engine_api: &EngineApi,
        workspace_root: &Path,
        peer_component_names: &[String],
        before_commit: &mut dyn FnMut(),
    ) -> RendererModuleChange {
        if let Some(slot) = &mut self.slot {
            return RendererModuleChange::Reloaded(slot.reload_if_changed_with(
                engine,
                engine_api,
                workspace_root,
                peer_component_names,
                before_commit,
            ));
        }
        let source_edit_generation = self.source_edit_generation.load(Ordering::Acquire);
        if source_edit_generation == self.last_failed_source_edit_generation {
            return RendererModuleChange::Unchanged;
        }
        if self.try_first_load(engine, engine_api, workspace_root, source_edit_generation) {
            RendererModuleChange::Loaded
        } else {
            RendererModuleChange::Unchanged
        }
    }

    /// Build, load and initialize the first generation from the sources as of
    /// `source_edit_generation`, and return `true` when it loaded.
    fn try_first_load(
        &mut self,
        engine: &mut Engine,
        engine_api: &EngineApi,
        workspace_root: &Path,
        source_edit_generation: u64,
    ) -> bool {
        let Some(extension_config) = &self.extension_config else {
            return false;
        };
        let module_name = extension_config.name.clone();
        match ExtensionSlot::start(
            engine,
            engine_api,
            workspace_root,
            extension_config,
            self.system_owner,
            Arc::clone(&self.source_edit_generation),
            FirstLoadFailure::ClearSystemsOnly,
        ) {
            Ok(mut slot) => {
                // A renderer image leaves state behind that nothing can
                // release: wgpu's, the graphics driver's, and the `tracing`
                // callsite registry's. Unloading one hangs the frame loop, so
                // every generation stays mapped.
                slot.keep_every_generation();
                // Mark the source edit this generation was built from as
                // handled. A save after this point is the next reload.
                slot.consume_pending_reload(source_edit_generation);
                self.slot = Some(slot);
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    module = module_name.as_str(),
                    "renderer module loaded"
                );
                true
            }
            Err(failure) => {
                self.last_failed_source_edit_generation = source_edit_generation;
                error!(
                    target: telemetry_target::HOT_RELOAD,
                    module = module_name.as_str(),
                    "the renderer module did not load; the window stays blank until the next renderer edit: {failure}"
                );
                false
            }
        }
    }
}

// =============================================================================
// RendererModuleChange
// =============================================================================

/// What [`RendererModule::reload_if_changed`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RendererModuleChange {
    /// Nothing new: no reload was pending, or the first load failed again.
    /// Whatever backend exists stays valid.
    Unchanged,
    /// A generation loaded for the first time. It needs attaching.
    Loaded,
    /// The module already had a slot, with the outcome of its reload pass:
    /// [`ReloadOutcome::Unchanged`] when nothing was pending. A reload that
    /// ran the `before_commit` hook detached the backend, so it needs
    /// attaching again, even when the reload rolled back to the old
    /// generation.
    Reloaded(ReloadOutcome),
}

// =============================================================================
// ModuleRenderer
// =============================================================================

/// A renderer backend that lives in the renderer module.
///
/// Drives the module's `dyn PillRenderer` through the pointer the attach
/// export returned, and hands it back to the module's detach export on drop.
struct ModuleRenderer {
    /// The renderer backend the module built, owned by this value.
    module_backend: NonNull<Box<dyn PillRenderer>>,
    /// The module's detach export, which frees `module_backend` inside the
    /// module.
    detach_export: DetachFn,
}

impl ModuleRenderer {
    /// The module's backend, borrowed for a call that only reads it.
    fn module_backend(&self) -> &dyn PillRenderer {
        // SAFETY: `module_backend` came from the module's attach export and is
        // owned by this value until drop, and the module stays mapped for that
        // long.
        unsafe { self.module_backend.as_ref() }.as_ref()
    }

    /// The module's backend, borrowed for a call that changes it.
    fn module_backend_mut(&mut self) -> &mut dyn PillRenderer {
        // SAFETY: the same reasoning as in `module_backend`; `&mut self` makes
        // this the only live borrow.
        unsafe { self.module_backend.as_mut() }.as_mut()
    }
}

impl PillRenderer for ModuleRenderer {
    // Every method of the contract is forwarded, including the ones with
    // defaults. A method left out would silently run the trait's default
    // instead of the module's renderer.
    fn capabilities(&self) -> RenderCapabilities {
        self.module_backend().capabilities()
    }

    fn metrics(&self) -> RenderMetrics {
        self.module_backend().metrics()
    }

    fn render(
        &mut self,
        frame: &RenderFrame,
        assets: &AssetManager,
    ) -> Result<FrameOutcome, RendererError> {
        self.module_backend_mut().render(frame, assets)
    }

    fn invalidate_assets(&mut self) {
        self.module_backend_mut().invalidate_assets();
    }

    unsafe fn attach_surface(
        &mut self,
        id: pill_renderer_api::SurfaceId,
        window: pill_renderer_api::RawWindowData,
        width: u32,
        height: u32,
        options: pill_renderer_api::SurfaceOptions,
    ) -> Result<(), RendererError> {
        // SAFETY: forwarded from this method's own contract.
        unsafe {
            self.module_backend_mut()
                .attach_surface(id, window, width, height, options)
        }
    }

    fn detach_surface(&mut self, id: pill_renderer_api::SurfaceId) {
        self.module_backend_mut().detach_surface(id);
    }

    fn resize_surface(&mut self, id: pill_renderer_api::SurfaceId, width: u32, height: u32) {
        self.module_backend_mut().resize_surface(id, width, height);
    }

    fn set_surface_viewport(
        &mut self,
        id: pill_renderer_api::SurfaceId,
        viewport: Option<RenderViewport>,
    ) {
        self.module_backend_mut().set_surface_viewport(id, viewport);
    }

    fn set_surface_ready(&mut self, id: pill_renderer_api::SurfaceId, ready: bool) {
        self.module_backend_mut().set_surface_ready(id, ready);
    }

    fn take_pick_results(&mut self) -> Vec<pill_renderer_api::PickResult> {
        self.module_backend_mut().take_pick_results()
    }

    fn take_preview_results(&mut self) -> Vec<pill_renderer_api::PreviewImage> {
        self.module_backend_mut().take_preview_results()
    }
}

impl Drop for ModuleRenderer {
    fn drop(&mut self) {
        // SAFETY: the pointer came from the module's attach export and is
        // released exactly once, here. The module is still mapped because its
        // slot is dropped after this renderer.
        unsafe { (self.detach_export)(self.module_backend.as_ptr().cast()) };
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Build a backend on `window` through the module's attach export.
///
/// # Errors
///
/// Returns [`RendererError::Other`] when no generation has loaded, when the
/// module lacks the attach or detach export, and when it reports that it could
/// not attach (it logs why).
///
/// # Safety
///
/// `window` names a live window that outlives the returned renderer, and the
/// returned renderer is dropped before `module` is.
pub(crate) unsafe fn attach_module_renderer(
    module: &RendererModule,
    window: RawWindowData,
    width: u32,
    height: u32,
) -> Result<Box<dyn PillRenderer>, RendererError> {
    // Step 1: Resolve the two exports in the image that is loaded now.
    let library = module
        .slot()
        .ok_or_else(|| RendererError::Other {
            detail: "the renderer module has not loaded".to_owned(),
        })?
        .current_library();
    let missing = |export: &str| RendererError::Other {
        detail: format!("the renderer module does not export `{export}`"),
    };
    // SAFETY: both type aliases match the exports' signatures exactly, and the
    // resolved pointers are only called while the module is mapped; the caller
    // drops the renderer before the module.
    let attach_export: AttachFn = unsafe { library.resolve_export(ATTACH_SYMBOL) }
        .ok_or_else(|| missing("pill_renderer_attach"))?;
    // SAFETY: the same reasoning as above; `pill_renderer_detach` takes one
    // pointer and returns nothing, which is what `DetachFn` states.
    let detach_export: DetachFn = unsafe { library.resolve_export(DETACH_SYMBOL) }
        .ok_or_else(|| missing("pill_renderer_detach"))?;

    // Step 2: Attach on the window and take ownership of the returned pointer.
    // SAFETY: `window` is a valid value on this stack for the call, and names a
    // live window that outlives the backend by this function's contract.
    let module_backend = unsafe { attach_export(&window, width, height) };
    let module_backend =
        NonNull::new(module_backend.cast::<Box<dyn PillRenderer>>()).ok_or_else(|| {
            RendererError::Other {
                detail: "the renderer module could not attach to the window; see the log above"
                    .to_owned(),
            }
        })?;
    Ok(Box::new(ModuleRenderer {
        module_backend,
        detach_export,
    }))
}
