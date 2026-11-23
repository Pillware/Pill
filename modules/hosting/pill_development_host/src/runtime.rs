//! The development host: it runs a project and its extensions as DLLs, and
//! reloads them while the game keeps running.
//!
//! # Responsibilities
//!
//! - Build and load the extensions and the project, and watch their sources.
//! - Reload, patch and roll back at each frame boundary, just before the
//!   wrapped [`pill_player::Runtime`] runs its frame ([`run_frame`]).
//! - With `rendering`: load the renderer as a reloadable module, rebuild it
//!   when its data crate reloads, stop it while it would read a component
//!   layout it was not built against, and pass on every shader the watcher
//!   re-cooks ([`RenderingHost`]).
//!
//! # Design
//!
//! Compiled only with `hot_reload`. The engine, its frame and the renderer's
//! window live in `pill_player`, which a shipping build runs directly. This
//! module adds what developing needs around them: [`DevHost`] wraps the
//! runtime for the headless path, and [`RenderingHost`] adds the renderer
//! module and the surfaces it draws on. Loading that differs per backend
//! stays behind [`LoadedProject`].

// Standard library
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

// External crates
use pill_core::error::{CSharpError, HostError};
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::utils::format_error_chain;
use pill_core::warn;
use pill_core::{error, info};
// The fast path's own reports: patch outcomes, rollback requests, generations.
use pill_core::telemetry::{log_block_colored, Colorize};
use pill_engine::Engine;
use pill_engine::EngineApi;
use pill_engine::{AssetGuid, SettingsOutcome, SystemOwner, World};
use pill_player::registration::extension_owner;
#[cfg(feature = "rendering")]
use pill_player::{AttachedRenderer, RendererWindow};
use pill_player::{FrameDriver, Runtime};
#[cfg(feature = "rendering")]
use pill_renderer_api::{PillRenderer, RenderViewport, RendererError};

// Current crate
use crate::analytics;
use crate::config::project_depends_on_crate;
use crate::csharp::{BuildCollection, BuildOutcome, BuildReport, ModuleExposedComponent};
use crate::extension::{ExtensionSlot, ReloadOutcome};
use crate::hot_patch::{BeginOutcome, PatchOutcome};
use crate::native_library::cleanup_temporary_files;
use crate::notices::{self, HostNotice};
use crate::project_module::{LoadedProject, ManagedReloadStart};
use crate::watchers::{spawn_source_watcher, AssetChange};
use crate::{ExtensionConfig, HostConfig, ProjectModuleBackend, ProjectModuleConfig};

// =============================================================================
// Constants
// =============================================================================

/// The renderer module's development shader reload export, defined by the GPU
/// module's `shader_hot_reload` beside the shader tree it cooks.
#[cfg(feature = "rendering")]
const SHADER_CHANGED_EXPORT: &str = "pill_render_data_shader_changed";

/// What [`SHADER_CHANGED_EXPORT`] returns when it could not look at the assets
/// at all.
#[cfg(feature = "rendering")]
const SHADER_CHANGE_REFUSED: u32 = u32::MAX;

/// File a developer or a script drops to drive live patching, relative to the
/// workspace root.
///
/// A file is used rather than an environment variable because the request has
/// to reach a process that is already running. It is a file rather than a
/// console command because the standalone host has no input loop, and its
/// stdout is routinely redirected.
///
/// One line, in one of these forms:
///
/// - `list`: print every generation this session installed.
/// - `<function>@<generation>`: reinstall that generation.
/// - `<function>@previous`: step back one from whatever is running.
/// - `<function>@0`: the code the artifact was built with.
///
/// The file is removed before the request is acted on, so a request is
/// honoured exactly once.
const ROLLBACK_REQUEST_FILE: &str = "target/hot/rollback.request";

// =============================================================================
// Types + Impls
// =============================================================================

/// Signature of [`SHADER_CHANGED_EXPORT`]: the world, the re-cooked file's
/// path relative to the crate's `res/`, and its WGSL; returns how many shader
/// assets changed.
#[cfg(feature = "rendering")]
type ShaderChangedFunction = unsafe extern "C" fn(
    world: *mut pill_engine::World,
    path: *const u8,
    path_len: usize,
    wgsl: *const u8,
    wgsl_len: usize,
) -> u32;

/// An editor-only module whose tolerated first load failed.
///
/// Kept with the owner tag and watcher counter its first load would have had,
/// so [`DevHost::retry_extension`] can build and load it later without
/// restarting the host.
struct PendingModule {
    /// The module's configuration, reused verbatim by the retry.
    config: crate::config::ExtensionConfig,
    /// The owner tag the module's systems register under once it loads.
    owner: pill_engine::SystemOwner,
    /// The counter the module's source watcher adds one to once it runs.
    source_edit_generation: Arc<AtomicU64>,
}

/// The headless development host.
///
/// It owns the runtime (the engine and its frame), the loaded project image and
/// every extension slot, and reloads them at each frame boundary
/// ([`run_frame`]). One struct holds all of it, so the headless, windowed and
/// editor frontends get the same engine lifetime and the same hot reload.
pub struct DevHost {
    /// The directory every spawned cargo build runs in and every artifact path
    /// is resolved against.
    workspace_root: PathBuf,
    /// The project module's configuration: its backend, watched sources and
    /// build command.
    project_module_config: ProjectModuleConfig,
    /// The renderer GPU module a windowed build loads, from the project's
    /// `renderer:` setting; `None` when it selects no renderer. Its data crate
    /// is already among `extensions`.
    #[cfg(feature = "rendering")]
    renderer: Option<String>,
    /// The data crate the renderer's manifest names; `None` when the project
    /// selects no renderer. Its `res` holds the materials, and its components
    /// are the ones a renderer edit has to be built against.
    #[cfg(feature = "rendering")]
    renderer_data: Option<String>,
    /// The engine and its frame. Declared before the loaded images: the world
    /// holds values whose drop functions live in those images, so it has to be
    /// gone before any of them unmaps. The engine is boxed, so the raw pointer
    /// in `engine_api` stays valid when the host moves.
    runtime: Runtime,
    /// The C-callable table a loaded artifact is handed at every entry point.
    engine_api: EngineApi,
    /// The loaded project image, and the generations retired behind it.
    loaded_project: LoadedProject,
    /// The project's own editor crate, when it has one and this host is an
    /// editor host. It loads after the project, because it exists to change
    /// the editor the project runs in, and it reloads with it.
    project_editor: Option<ExtensionSlot>,
    /// Extensions, each with its own watcher and reload transaction.
    extensions: Vec<ExtensionSlot>,
    /// Tolerated editor-only modules whose first load failed: no slot and no
    /// watcher; [`DevHost::retry_extension`] is their only way in.
    pending_modules: Vec<PendingModule>,
    /// Counter the project watcher adds one to on every source save it cares
    /// about.
    ///
    /// The watcher is the only producer. Two consumers read it:
    /// `try_project_fast_path`, which marks a generation handled once its patch
    /// has delivered the edit, and the Step 5 reload transaction, which
    /// rebuilds from it.
    source_edit_generation: Arc<AtomicU64>,
    /// The `source_edit_generation` value the frame loop last acted on.
    last_processed_source_edit: u64,
    /// Counter the reload pipeline adds one to when it owes the project a
    /// rebuild: a module the project links directly reloaded, or a module swap
    /// changed the C# mirror surface.
    ///
    /// It is separate from [`Self::source_edit_generation`] on purpose. "A
    /// source save arrived" and "a rebuilt module has to be compiled into the
    /// project again" are different reasons to rebuild. One counter for both
    /// would let a fast-path patch of the project's own edit mark the cascade
    /// handled too, and skip the rebuild without a word.
    ///
    /// No fast path reads this counter, so the rebuild stays owed until
    /// `loaded_project.reload` runs. Only the frame thread writes and reads it,
    /// which is why it is a plain integer and not an atomic.
    queued_reload_generation: u64,
    /// The `queued_reload_generation` value the frame loop last rebuilt for.
    last_processed_queued_reload: u64,
    /// The project watcher's recent signals, so the reload this loop starts
    /// can be timed from the save that caused it.
    ///
    /// The project's watcher is the only producer: the module and renderer
    /// watchers pass `None`, because nothing times a reload from their
    /// signals. A reload takes the entries up to its generation, so each
    /// signal times exactly one reload.
    source_trigger_log: Arc<crate::watchers::TriggerLog>,
    /// Owners whose systems this host suspended because their binary was built
    /// against a shared component layout another reload has replaced.
    ///
    /// Kept so only these are resumed when they are rebuilt: an owner some
    /// other mechanism suspended (the renderer's pause) is not this code's to
    /// turn back on.
    stale_suspended_owners: Vec<pill_engine::SystemOwner>,
    /// Per-function fast path, when the project opted in with `#[pill_hot]`.
    ///
    /// `None` when the feature is off, when no function is annotated, or when
    /// the project does not also build an `rlib`. In all three cases the
    /// existing whole-module reload stays the only path.
    hot_patch: Option<crate::hot_patch::HotPatchSession>,
    /// Per-function fast path for each extension, positionally paired
    /// with `extensions`.
    ///
    /// An entry is `None` when that module annotated nothing, so a module that
    /// has not opted in costs nothing beyond one source scan at startup.
    module_hot_patch: Vec<Option<crate::hot_patch::HotPatchSession>>,
    /// The patch build running on its own thread, when one is.
    ///
    /// Every reload step defers while this is `Some`: a reload rebuilds the
    /// rlibs the build links against, and a second attempt would race the
    /// first for the same pending edit. The compile runs off the frame loop,
    /// which is the reason the thread exists.
    patch_attempt: Option<InFlightPatch>,
    /// Every patch library loaded in this process, newest last.
    ///
    /// They belong to the process, not to one session. A patch links its own
    /// copy of everything its body calls, so patching one crate also has to
    /// redirect the copies inside another crate's patches. They are never
    /// unloaded, because a jump or a slot may point into any of them for the
    /// rest of the run.
    loaded_patches: Vec<crate::hot_patch::LoadedPatch>,
    /// Watches the project's `res` and queues the assets whose source or
    /// metadata file changed; `None` when no asset directory is configured or
    /// the watch could not start.
    asset_watcher: Option<crate::watchers::AssetWatcher>,
    /// The project's `res` directory, for the asset browser; `None` when the
    /// configuration did not come from a project directory.
    asset_directory: Option<PathBuf>,
    /// The [`Self::editor_revision`] the manager settings were last applied for.
    ///
    /// A reload replaces managers, so their settings assets are applied again
    /// after one; the revision counter is the signal that something loaded or
    /// unloaded.
    manager_settings_editor_revision: u64,
    /// Whether every source asset is kept paired with a `.meta` file; see
    /// [`DevHost::set_ensure_asset_metadata`]. Off unless a frontend asks.
    ensure_asset_metadata: bool,
    /// Counter of reload, rollback and patch events, only ever increased.
    ///
    /// The editor uses it as the key for its cached engine metadata. It is not
    /// how the editor learns that entities changed: gameplay changes those
    /// every frame without a reload. It only says that the set of loaded code
    /// changed.
    editor_revision: u64,
    /// What this run's watchers are allowed to do, read once at startup.
    switches: crate::switches::HostSwitches,
    /// Whether the startup line about switched-off patching has been written.
    ///
    /// The fast path is asked on every frame, and the answer only changes once
    /// per run, so the message belongs to the first edit rather than to a log
    /// that scrolls.
    patch_switched_off_logged: bool,
}

impl DevHost {
    /// Every patch generation this process has installed, newest last.
    ///
    /// Generation zero - the code each artifact was built with - has no entry
    /// because it needs no loaded library; [`Self::rollback_patch`] still
    /// accepts it.
    pub fn patch_generations(&self) -> Vec<crate::hot_patch::PatchGeneration> {
        let mut all = Vec::new();
        if let Some(session) = &self.hot_patch {
            all.extend(session.generations());
        }
        for session in self.module_hot_patch.iter().flatten() {
            all.extend(session.generations());
        }
        all
    }

    /// Reinstall an earlier generation of one patched function.
    ///
    /// `generation` is one-based in the order patches were applied; zero
    /// restores the code the running artifact was built with. Nothing is
    /// rebuilt, reloaded or unloaded. A generation delivered through a dispatch
    /// slot is one pointer store per artifact, and a generation written over
    /// the function's own code is patched back in place. Call it between frames:
    /// it borrows the host mutably.
    ///
    /// # Errors
    ///
    /// Returns a message when no session has patched `function`, when the
    /// generation does not exist, or when an artifact refuses the address. A
    /// slot route leaves the running implementation in place; a prologue route
    /// that fails partway leaves the copies it already rewrote as it put them.
    pub fn rollback_patch(&mut self, function: &str, generation: u32) -> Result<(), String> {
        let DevHost {
            hot_patch,
            module_hot_patch,
            loaded_patches,
            extensions,
            loaded_project,
            runtime,
            ..
        } = self;
        let engine = runtime.engine_mut();

        // The owning session is whichever one patched this function; searching
        // avoids making callers know whether it came from the project or a
        // module.
        let session = hot_patch
            .iter_mut()
            .chain(module_hot_patch.iter_mut().flatten())
            .find(|session| session.knows_function(function))
            .ok_or_else(|| format!("`{function}` has not been patched in this session"))?;

        let targets = patch_targets(loaded_project, extensions);
        // A successful rollback is logged by the session itself
        // ("patch generation rolled back").
        let result = session.rollback(engine, &targets, loaded_patches, function, generation);
        drop(targets);
        result
    }

    /// Read-only engine access for rendering and diagnostics.
    pub fn engine(&self) -> &Engine {
        self.runtime.engine()
    }

    /// Mutable engine access for frontend-owned ad-hoc work.
    pub fn engine_mut(&mut self) -> &mut Engine {
        self.runtime.engine_mut()
    }

    /// Monotonic counter of reload/rollback/patch events completed by this
    /// host. Editor caches of engine metadata key on it.
    pub fn revision(&self) -> u64 {
        self.editor_revision
    }

    /// Advance the editor revision counter after a reload/rollback/patch.
    ///
    /// The increment itself is the editor's cache-invalidation signal; the
    /// log line exists so integration suites can check that the counter moved
    /// without driving a GUI.
    fn bump_editor_revision(&mut self) {
        self.editor_revision = self.editor_revision.wrapping_add(1);
        info!(
            target: telemetry_target::HOT_RELOAD,
            revision = self.editor_revision,
            "editor revision bumped"
        );
    }

    /// Names of the loaded extensions, in `SystemOwner` order.
    ///
    /// `SystemOwner::extension(i)` is `i + 1`, so index `i` of this
    /// vector labels owner `i + 1`; owner `0` is the project.
    pub fn extension_names(&self) -> Vec<String> {
        self.extensions
            .iter()
            .map(|slot| slot.name().to_string())
            .collect()
    }

    /// Every subject this host registers systems for, with its owner: the
    /// loaded extensions, the modules whose first load failed and wait for a
    /// retry, and the project.
    ///
    /// The renderer is not one of them - it belongs to [`RenderingHost`] - so
    /// the windowed host adds it before it checks.
    pub(crate) fn subject_owners(&self) -> Vec<(&str, SystemOwner)> {
        let mut owners: Vec<(&str, SystemOwner)> = self
            .extensions
            .iter()
            .map(|slot| (slot.name(), slot.owner()))
            .collect();
        owners.extend(
            self.pending_modules
                .iter()
                .map(|pending| (pending.config.name.as_str(), pending.owner)),
        );
        if let Some(project_editor) = &self.project_editor {
            owners.push((project_editor.name(), project_editor.owner()));
        }
        owners.push(("the project", SystemOwner::PROJECT));
        owners
    }

    /// Debug assertion: no two subjects register under one owner.
    ///
    /// Owners decide what a reload clears, what a pause suspends and what a
    /// stale suspension resumes. Two subjects sharing one would let a reload of
    /// either retire the other's systems, and let a resume reach a renderer that
    /// was paused against a replaced component layout.
    #[cfg(debug_assertions)]
    pub(crate) fn assert_owner_isolation(&self) {
        assert_unique_owners(&self.subject_owners());
    }

    /// Whether any watcher's work waits for a frame boundary: the editor's
    /// sleeping heartbeat takes a frame when one does, so an edit reaches a
    /// paused, idle editor without user input.
    pub fn source_work_pending(&self) -> bool {
        if self.patch_attempt.is_some() {
            return true;
        }
        if self
            .extensions
            .iter()
            .any(|slot| slot.pending_reload_generation().is_some())
        {
            return true;
        }
        if self
            .project_editor
            .as_ref()
            .is_some_and(|slot| slot.pending_reload_generation().is_some())
        {
            return true;
        }
        self.source_edit_generation.load(Ordering::Acquire) != self.last_processed_source_edit
    }

    /// Rebuild and load a module the host may have skipped, or force a
    /// rebuild of a loaded one.
    ///
    /// For a tolerated editor-only module whose first load failed, runs that
    /// load again and starts its watcher on success. For a loaded module,
    /// asks its reload path to rebuild as if its sources were saved.
    ///
    /// # Errors
    ///
    /// Returns the failure text when the module still does not build, load
    /// or register; the notice queue carries the same failure for the
    /// embedder's error UI.
    pub fn retry_extension(&mut self, module_name: &str) -> Result<(), String> {
        if let Some(slot) = self
            .extensions
            .iter_mut()
            .find(|slot| slot.name() == module_name)
        {
            slot.request_reload();
            info!(
                target: telemetry_target::HOT_RELOAD,
                module = module_name,
                "module rebuild requested"
            );
            return Ok(());
        }
        let Some(index) = self
            .pending_modules
            .iter()
            .position(|pending| pending.config.name == module_name)
        else {
            return Err(format!("no module named `{module_name}` to retry"));
        };
        let pending = self.pending_modules.remove(index);
        match ExtensionSlot::start(
            self.runtime.engine_mut(),
            &self.engine_api,
            &self.workspace_root,
            &pending.config,
            pending.owner,
            Arc::clone(&pending.source_edit_generation),
            // A retry runs long after the project has filled the world, so it
            // must not take the world with it. `ClearWorld` throws everything
            // away when the module fails to initialize: right for the first
            // thing a host loads, and the last thing a click on Retry should
            // do.
            //
            // `ClearSystemsOnly` keeps the world, but it refuses a module that
            // registers data. The module this retry serves (the editor's
            // shell) registers components like any other, so every retry would
            // fail. `ClearGenerationData` fits: it keeps the project's world,
            // drops what the failed attempt registered, and keeps the refused
            // image loaded so nothing it left behind points at unmapped code.
            crate::reload::FirstLoadFailure::ClearGenerationData,
        ) {
            Ok(slot) => {
                if self.switches.hot_reloading {
                    if let Err(error) = spawn_source_watcher(
                        self.workspace_root.clone(),
                        &pending.config.name,
                        &pending.config.watch_directory,
                        Arc::clone(&pending.source_edit_generation),
                        None,
                    ) {
                        // The module works; only its watcher failed to start, so
                        // a later edit will not reload it until a restart. Keep
                        // the module - the retry's purpose is the working load.
                        warn!(
                            target: telemetry_target::HOT_RELOAD,
                            module = pending.config.name.as_str(),
                            error = %error,
                            "the module loaded, but its source watcher did not start"
                        );
                    }
                }
                self.extensions.push(slot);
                // With patching switched off the slot stays empty: the fast
                // path has nothing to build against, so the module's edits go
                // to the reload below instead.
                let module_patch_session = if self.switches.function_hot_patching {
                    crate::hot_patch::HotPatchSession::new(
                        &self.workspace_root,
                        &pending.config.name,
                        &pending.config.watch_directory,
                        &pending.config.output_subdirectory,
                        &pending.config.build_command,
                    )
                } else {
                    None
                };
                self.module_hot_patch.push(module_patch_session);
                // The module moved out of the pending set into the loaded one
                // under the owner it reserved; the set as a whole must still be
                // one owner per subject.
                #[cfg(debug_assertions)]
                self.assert_owner_isolation();
                Ok(())
            }
            Err(error) => {
                notices::push(HostNotice::ModuleFailed {
                    module: pending.config.name.clone(),
                    output: optional_failure_output(&error),
                });
                self.pending_modules.insert(index, pending);
                Err(error.to_string())
            }
        }
    }

    /// The project's `res` directory, when the host knows it.
    pub fn asset_directory(&self) -> Option<&Path> {
        self.asset_directory.as_deref()
    }

    /// The workspace root every spawned build ran from.
    ///
    /// Compiler diagnostics name sources relative to it, so a frontend
    /// handing a stripped path to another tool resolves it against this.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// The project's `res` tree with each file's asset type, load state and
    /// guid; empty when the host has no `res` directory.
    pub fn asset_entries(&self) -> Vec<crate::asset_browser::AssetEntry> {
        match &self.asset_directory {
            Some(directory) => {
                crate::asset_browser::list_assets(self.runtime.engine().world(), directory)
            }
            None => Vec::new(),
        }
    }

    /// The import settings (or standalone document) of the asset at `path`,
    /// relative to `res`, as JSON.
    ///
    /// # Errors
    ///
    /// A message when no registered type imports the path or its file cannot
    /// be read.
    pub fn asset_settings(&self, path: &str) -> Result<serde_json::Value, String> {
        crate::asset_browser::asset_settings(self.runtime.engine().world(), path)
    }

    /// Save edited import settings for the asset at `path`; the asset watcher
    /// reimports it at the next frame. Returns the normalized settings written.
    ///
    /// # Errors
    ///
    /// A message when the settings do not fit the type or the file cannot be
    /// written.
    pub fn save_asset_settings(
        &mut self,
        path: &str,
        settings: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let directory = self
            .asset_directory
            .clone()
            .ok_or("the host has no project `res` directory")?;
        crate::asset_browser::save_asset_settings(
            self.runtime.engine_mut().world_mut(),
            &directory,
            path,
            settings,
        )
    }

    /// Keep every source asset in `res` paired with a `.meta` file.
    ///
    /// Turning it on writes the missing files at once. Every source file that a
    /// registered asset type imports gets that type's default settings and a
    /// guid, without being decoded or loaded.
    ///
    /// While it stays on, a `.meta` deleted under a running host is written
    /// again, using the loaded asset's guid when it is loaded. A new source
    /// gets one when the watcher imports it. The editor turns this on; a game
    /// run leaves it off.
    pub fn set_ensure_asset_metadata(&mut self, enabled: bool) {
        self.ensure_asset_metadata = enabled;
        if !enabled {
            return;
        }
        let Some(directory) = self.asset_directory.clone() else {
            return;
        };
        let world = self.runtime.engine().world();
        let (Some(registry), Some(assets)) = (
            world.get_resource::<pill_engine::ImportRegistry>(),
            world.get_resource::<pill_engine::AssetManager>(),
        ) else {
            return;
        };
        let (written, failed) = registry.ensure_all_metadata(assets, &directory);
        for name in &written {
            info!(
                target: telemetry_target::HOT_RELOAD,
                asset = name.as_str(),
                "[assets] wrote missing metadata"
            );
        }
        for (name, error) in &failed {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                asset = name.as_str(),
                "[assets] could not write missing metadata: {error}"
            );
        }
        info!(
            target: telemetry_target::HOT_RELOAD,
            written = written.len(),
            failed = failed.len(),
            "[assets] every source asset has a .meta file"
        );
    }

    /// The standalone asset types a new file can be created for.
    pub fn standalone_asset_types(&self) -> Vec<crate::asset_browser::StandaloneType> {
        crate::asset_browser::standalone_types(self.runtime.engine().world())
    }

    /// Create and load a new standalone asset; see
    /// [`crate::asset_browser::create_standalone_asset`]. Returns its path
    /// relative to `res`.
    ///
    /// # Errors
    ///
    /// A message when the input is refused or the file cannot be written.
    pub fn create_standalone_asset(
        &mut self,
        type_name: &str,
        folder: &str,
        asset_name: &str,
    ) -> Result<String, String> {
        let directory = self
            .asset_directory
            .clone()
            .ok_or("the host has no project `res` directory")?;
        crate::asset_browser::create_standalone_asset(
            self.runtime.engine_mut().world_mut(),
            &directory,
            type_name,
            folder,
            asset_name,
        )
    }

    /// Move the asset at `from` to `to` (both relative to `res`) with its
    /// `.meta`; the asset watcher follows the move.
    ///
    /// # Errors
    ///
    /// A message when a path leaves `res` or the move is refused.
    pub fn move_asset(&self, from: &str, to: &str) -> Result<(), String> {
        let directory = self
            .asset_directory
            .as_deref()
            .ok_or("the host has no project `res` directory")?;
        crate::asset_browser::move_asset(directory, from, to)
    }
}

impl FrameDriver for DevHost {
    type Error = std::convert::Infallible;

    fn run_frame(&mut self) -> Result<(), Self::Error> {
        run_frame(self);
        Ok(())
    }

    fn engine_mut(&mut self) -> &mut pill_engine::Engine {
        DevHost::engine_mut(self)
    }
}

/// Why a windowed host stopped rendering; see [`RenderingHost`]'s `paused`.
#[cfg(feature = "rendering")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PauseReason {
    /// The renderer's data crate reloaded with a different component layout,
    /// and the renderer rebuilt against it failed to build, initialize or
    /// attach.
    StaleRendererAfterDataLayoutChange,
}

/// What a renderer step did, for the pause decision.
#[cfg(feature = "rendering")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RendererStep {
    /// Nothing reloaded; whatever was attached stays.
    Unchanged,
    /// A generation loaded or reloaded and is attached to the window.
    Attached,
    /// A reload or first load was attempted and did not end with a new
    /// generation attached (build, `init` or attach failed; a rollback may
    /// have re-attached the previous generation).
    Failed,
}

/// The development host with the engine renderer drawing on native surfaces.
///
/// Keeping the renderer beside [`DevHost`] makes its creation and lifetime part
/// of host setup. Executable crates never construct or retain GPU resources.
#[cfg(feature = "rendering")]
pub struct RenderingHost {
    /// The wrapped development host: the engine, the project and every
    /// extension.
    host: DevHost,
    /// The backend and the window it draws on; the backend drops first.
    /// Declared before `renderer_module` so the backend is detached while the
    /// module it lives in is still mapped.
    attached_renderer: AttachedRenderer,
    /// The renderer module the backend lives in.
    renderer_module: RendererModule,
    /// Why rendering is paused, or `None` while it runs.
    ///
    /// Set when the renderer's data crate reloaded with a different component
    /// layout, and no renderer built against the new layout could take over.
    /// The live renderer's `rendering` system and `render` would read the new
    /// rows through the old field offsets. While paused, the renderer's owner
    /// is disabled and `render` is skipped, so the window keeps its last frame.
    paused: Option<PauseReason>,
    /// Whether the frontend last reported the scene quiet. With the project
    /// also paused, a reload-free boundary and no renderer change, the frame
    /// skips the render and the surfaces keep their last image.
    scene_quiet: bool,
    /// Whether the last frame drew the surfaces; the frontend clears its
    /// scene-changed flag on the frames this is true.
    scene_drawn_last_frame: bool,
    /// Watches the renderer data crate's `res/` tree and queues
    /// what it re-cooks. `None` without a renderer, or when its data crate has
    /// no shaders or the watch could not start.
    shader_watcher: Option<crate::watchers::ShaderWatcher>,
    /// Watches the project's own shader sources under its asset directory and
    /// queues what it re-cooks. `None` when the project has no asset
    /// directory, or when the watch could not start.
    project_shader_watcher: Option<crate::watchers::ShaderWatcher>,
    /// Set whenever a renderer backend attaches; the next frame then reports
    /// the render data the renderer ignores, once per renderer generation.
    unsupported_data_check_pending: bool,
}

#[cfg(feature = "rendering")]
impl RenderingHost {
    /// Move rendering to a newly created native window surface.
    ///
    /// The existing ECS host and project module remain alive. A replacement is
    /// constructed before the old renderer is dropped, so initialization
    /// failure leaves the current surface untouched.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when the replacement window's surface cannot
    /// be created or the renderer cannot attach to it.
    pub fn retarget_render_window<W>(
        &mut self,
        window: W,
        width: u32,
        height: u32,
    ) -> Result<(), RendererError>
    where
        W: RendererWindow + 'static,
    {
        let renderer_module = &self.renderer_module;
        self.attached_renderer
            .retarget(window, width, height, |window_data| {
                attach_backend(renderer_module, window_data, width, height)
            })
    }

    /// Forward a physical resize of surface `id` to the engine renderer.
    pub fn resize_surface(&mut self, id: pill_renderer_api::SurfaceId, width: u32, height: u32) {
        self.attached_renderer.resize_surface(id, width, height);
    }

    /// Reload the renderer module when its sources changed, and keep the
    /// window drawn by whichever generation is current afterwards.
    ///
    /// The backend runs code inside the module image, so it is detached in the
    /// reload's `before_commit` hook. That runs after the replacement has built
    /// and loaded, and before the swap retires the old image. At that moment
    /// the image the backend calls into is still mapped, and the host draws
    /// nothing until the attach below.
    ///
    /// After the commit the slot holds the new generation, or the old one if
    /// the new `init` failed and the transaction rolled back. Both are
    /// attached the same way. A failed build or load leaves the backend as it
    /// was, and a failed attach leaves the window undrawn until the next
    /// source change.
    fn reload_renderer_if_changed(&mut self) -> RendererStep {
        let RenderingHost {
            host,
            attached_renderer,
            renderer_module,
            unsupported_data_check_pending,
            ..
        } = self;
        let mut detached = false;
        let peers = peer_component_names(&host.extensions, None, &host.loaded_project);
        let outcome = renderer_module.reload_if_changed(
            host.runtime.engine_mut(),
            &host.engine_api,
            &host.workspace_root,
            &peers,
            &mut || {
                // Dropping the module's backend runs its detach export inside
                // the image that is about to retire, while it is still mapped.
                attached_renderer.detach_backend();
                detached = true;
            },
        );
        // A generation that loaded for the first time - after a startup load
        // that failed - has nothing attached yet, exactly like a detached one.
        let loaded = outcome == crate::renderer_module::RendererModuleChange::Loaded;
        if !detached && !loaded {
            // A reload that failed before its commit (build or load) left the
            // backend attached; one that did not run changed nothing.
            return match outcome {
                crate::renderer_module::RendererModuleChange::Reloaded(ReloadOutcome::Failed {
                    ..
                }) => RendererStep::Failed,
                _ => RendererStep::Unchanged,
            };
        }
        // After the commit: a rolled-back reload re-attaches the previous
        // generation, which is not a renderer built for new data.
        let rolled_back = matches!(
            outcome,
            crate::renderer_module::RendererModuleChange::Reloaded(ReloadOutcome::Failed { .. })
        );
        let (width, height) = attached_renderer.surface_size();
        match attach_backend(
            renderer_module,
            attached_renderer.window_data(),
            width,
            height,
        ) {
            Ok(attached) => {
                attached_renderer.replace_backend(attached);
                // A new renderer generation: what it ignores is reported anew.
                *unsupported_data_check_pending = true;
                // Counted, not assumed: a generation whose system escaped its
                // owner would leave a second `rendering` system behind.
                let rendering_systems = host
                    .runtime
                    .engine()
                    .system_snapshots()
                    .iter()
                    .filter(|system| system.name == "rendering")
                    .count();
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    outcome = ?outcome,
                    rendering_systems,
                    "renderer reattached to the window"
                );
                if rolled_back {
                    RendererStep::Failed
                } else {
                    RendererStep::Attached
                }
            }
            Err(error) => {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    outcome = ?outcome,
                    "renderer reloaded but could not reattach; the window stays undrawn until the next renderer edit: {error}"
                );
                RendererStep::Failed
            }
        }
    }

    /// The renderer step of one frame boundary, run between the host's reload
    /// phase and its frame phase. It reloads the renderer when its own sources
    /// changed, or when its data crate just reloaded, then pauses or resumes
    /// rendering accordingly.
    ///
    /// Module builds are synchronous, so a data reload and the renderer
    /// rebuild it requests complete in the same boundary. In the normal case
    /// no frame runs between the two swaps and nothing pauses. The pause covers
    /// the rest: a layout change whose renderer could not be rebuilt,
    /// initialized or attached.
    fn renderer_step(
        &mut self,
        data_extension: Option<&str>,
        reloaded_extensions: &[String],
        schemas_before: Option<Vec<(String, Option<u64>)>>,
    ) -> RendererStep {
        // Step 1: A reloaded data crate means the renderer's compiled-in copy
        // of it is stale: rebuild the renderer against the new source.
        let data_reloaded =
            data_extension.is_some_and(|data| reloaded_extensions.iter().any(|name| name == data));
        let data_layout_changed = match (data_reloaded, data_extension, schemas_before) {
            (true, Some(data), Some(before)) => component_schemas_of(&self.host, data) != before,
            _ => false,
        };
        if data_reloaded {
            info!(
                target: telemetry_target::HOT_RELOAD,
                layout_changed = data_layout_changed,
                "the renderer's data crate reloaded; rebuilding the renderer against it"
            );
            self.renderer_module.request_rebuild();
        }

        // Step 2: The renderer reload itself, from its own edit or the request.
        let step = self.reload_renderer_if_changed();

        // Step 3: Pause or resume.
        let owner = self.renderer_module.owner();
        let next = pause_after_boundary(self.paused, data_layout_changed, step);
        if next != self.paused {
            let enabled = next.is_none();
            self.host
                .runtime
                .engine_mut()
                .set_systems_enabled_for_owner(owner, enabled);
            match next {
                Some(reason) => warn!(
                    target: telemetry_target::HOT_RELOAD,
                    reason = ?reason,
                    "rendering paused: the renderer is not built against the current data layout; the window keeps its last frame until a renderer edit builds and loads"
                ),
                None => info!(
                    target: telemetry_target::HOT_RELOAD,
                    "rendering resumed: the renderer is built against the current data layout"
                ),
            }
            self.paused = next;
        } else if next.is_some() {
            // Still paused: a generation that registered during the pause
            // starts disabled already (the owner stays disabled).
            self.host
                .runtime
                .engine_mut()
                .set_systems_enabled_for_owner(owner, false);
        }

        step
    }

    /// Deliver everything the shader watchers re-cooked since the last frame.
    ///
    /// The renderer's shaders and the project's own each have a tree of their
    /// own, and their outputs reach the running world by different routes, so
    /// each half is handled on its own. The result says whether anything was
    /// delivered, which is one of the things that keeps an idle frame from
    /// skipping its redraw.
    fn deliver_shader_changes(&mut self) -> bool {
        let renderer_delivered = self.deliver_renderer_shader_changes();
        let project_delivered = self.deliver_project_shader_changes();
        renderer_delivered || project_delivered
    }

    /// Hand every shader the watcher re-cooked to the renderer module's
    /// `pill_render_data_shader_changed`, which puts the WGSL into the shader
    /// assets built from it.
    ///
    /// The export is looked up again for every delivery, in the renderer
    /// generation that is current at this frame boundary. A shader edit made
    /// while the renderer was rebuilding therefore reaches the generation that
    /// replaced it. A renderer without the export (a shipping build, where this
    /// path is compiled out) leaves the edits undelivered and logs a warning.
    fn deliver_renderer_shader_changes(&mut self) -> bool {
        let Some(watcher) = &self.shader_watcher else {
            return false;
        };
        let cooked = watcher.drain();
        if cooked.is_empty() {
            return false;
        }
        let Some(slot) = self.renderer_module.slot() else {
            return false;
        };
        let Some(address) = slot.export_address(SHADER_CHANGED_EXPORT) else {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = slot.name(),
                export = SHADER_CHANGED_EXPORT,
                "the renderer module does not export the shader reload function; shader edits are not applied"
            );
            return false;
        };
        let engine = self.host.runtime.engine_mut();
        // SAFETY: the renderer exports this name with exactly
        // `ShaderChangedFunction`'s signature. A function pointer and a data
        // pointer have the same size on every target this engine builds for.
        let shader_changed =
            unsafe { std::mem::transmute_copy::<*const (), ShaderChangedFunction>(&address.0) };
        let world: *mut pill_engine::World = engine.world_mut();
        let mut delivered = false;
        for shader in cooked {
            // SAFETY: `world` is the live engine world, used by nothing else for
            // the call, and both buffers are the owned strings' bytes.
            let changed = unsafe {
                shader_changed(
                    world,
                    shader.relative_path.as_ptr(),
                    shader.relative_path.len(),
                    shader.wgsl.as_ptr(),
                    shader.wgsl.len(),
                )
            };
            if changed == SHADER_CHANGE_REFUSED {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    path = shader.relative_path.as_str(),
                    "the renderer module refused a re-cooked shader"
                );
            } else {
                delivered = true;
            }
        }
        delivered
    }

    /// Reimport every project shader whose cooked text the project watcher
    /// replaced.
    ///
    /// A project's shader asset reads the `.wgsl` cooked beside its `.hlsl`
    /// source at import time, so the cook alone changes nothing: the asset has
    /// to be decoded again for the new text to reach its slot and for the
    /// renderer to rebuild the shader from it. `ImportRegistry::reimport` is
    /// that decode, and it imports a source nothing has loaded yet.
    ///
    /// The registry and the store are read from the world on every delivery,
    /// so a reloaded renderer data crate's registrations are the ones used.
    fn deliver_project_shader_changes(&mut self) -> bool {
        let Some(watcher) = &self.project_shader_watcher else {
            return false;
        };
        let cooked = watcher.drain();
        if cooked.is_empty() {
            return false;
        }
        let world: &mut pill_engine::World = self.host.runtime.engine_mut().world_mut();
        // A clone, so the registry can be called against the asset manager,
        // which is a second resource of the same world.
        let Some(registry) = world.get_resource::<pill_engine::ImportRegistry>().cloned() else {
            return false;
        };
        let Some(assets) = world.get_resource_mut::<pill_engine::AssetManager>() else {
            return false;
        };
        let mut delivered = false;
        for shader in cooked {
            let path = Path::new(&shader.source_relative_path);
            match registry.reimport(assets, path) {
                Ok(outcome) => {
                    delivered = true;
                    info!(
                        target: telemetry_target::HOT_RELOAD,
                        asset = shader.source_relative_path.as_str(),
                        guid = %guid_text(outcome.guid),
                        content_version_before = ?outcome.previous_content_version,
                        content_version = ?outcome.content_version,
                        "[assets] project shader reimported"
                    );
                }
                Err(error) => warn!(
                    target: telemetry_target::HOT_RELOAD,
                    asset = shader.source_relative_path.as_str(),
                    "[assets] project shader reimport failed; keeping the loaded value: {error}"
                ),
            }
        }
        delivered
    }

    /// Warn once about each render component the attached renderer ignores.
    ///
    /// Only shared components are candidates, and only those the data crate
    /// registered and at least one entity carries. They have to be shared:
    /// that is how render data reaches the renderer module at all. The
    /// module's own copy of a type finds the world's column by the shared
    /// identity. A plain component the data crate registers along the way is
    /// not render data.
    ///
    /// A candidate the renderer's `RenderCapabilities::consumed_components`
    /// does not name draws nothing, and the project should hear that rather
    /// than wonder why its directional light has no effect.
    fn report_unsupported_render_data(&self, data_extension: Option<&str>) {
        let Some(slot) = data_extension
            .and_then(|data| self.host.extensions.iter().find(|slot| slot.name() == data))
        else {
            return;
        };
        let consumed_component_names = self
            .attached_renderer
            .renderer()
            .capabilities()
            .consumed_components;
        let ignored = unsupported_render_components(
            self.host.engine().world(),
            slot.exposed_component_names(),
            &consumed_component_names,
        );
        for (component, entities) in ignored {
            warn!(
                target: telemetry_target::RENDERING,
                component = component.as_str(),
                entities,
                renderer = self.host.renderer.as_deref().unwrap_or_default(),
                "entities carry a render component the active renderer does not draw; it has no effect on the picture"
            );
        }
    }

    /// Restrict engine drawing to a region of surface `id`.
    ///
    /// Use `None` for full-surface rendering. Embedded frontends can leave the
    /// corresponding WebView region transparent and keep surrounding UI
    /// panels opaque.
    pub fn set_surface_viewport(
        &mut self,
        id: pill_renderer_api::SurfaceId,
        viewport: Option<RenderViewport>,
    ) {
        self.attached_renderer.set_surface_viewport(id, viewport);
    }

    /// Tell the host the frontend has nothing for the scene surfaces to draw.
    /// With the project also paused and nothing reloading, the next frame skips
    /// the render and the surfaces keep their last image. The frontend clears
    /// the hint on anything that changes them.
    pub fn set_scene_quiet(&mut self, quiet: bool) {
        self.scene_quiet = quiet;
    }

    /// Whether the last frame drew the surfaces; the frontend clears its
    /// scene-changed flag on the frames this is true.
    pub fn scene_drawn_last_frame(&self) -> bool {
        self.scene_drawn_last_frame
    }

    /// Execute one ECS frame and present its resulting world to the surface.
    ///
    /// # Errors
    ///
    /// Returns a [`RendererError`] when the renderer cannot draw the frame.
    pub fn run_frame(&mut self) -> Result<(), RendererError> {
        let frame_start = Instant::now();

        // Step 1: The data crate's schemas before any reload, when it has an
        // edit pending: compared after the reload phase to tell a layout change
        // from a body-only one.
        let data_extension = self.host.renderer_data.clone();
        let schemas_before = data_extension.as_deref().and_then(|data| {
            let pending = self
                .host
                .extensions
                .iter()
                .any(|slot| slot.name() == data && slot.pending_reload_generation().is_some());
            pending.then(|| component_schemas_of(&self.host, data))
        });

        // Step 2: Extension and project reloads, then the renderer step, all
        // before any system runs. The renderer this frame draws with is built
        // against the data this frame holds, or rendering is paused.
        let reloaded_extensions = run_reload_phase(&mut self.host);
        let renderer_step = self.renderer_step(
            data_extension.as_deref(),
            &reloaded_extensions,
            schemas_before,
        );
        // Re-cooked shaders go to the renderer generation that is current now,
        // after every swap of this boundary.
        let shaders_delivered = self.deliver_shader_changes();

        // Step 3: The frame's systems.
        run_frame_phase(&mut self.host, frame_start);

        // Step 4: While paused the live renderer must not read the world.
        if self.paused.is_some() {
            self.scene_drawn_last_frame = false;
            return Ok(());
        }
        // Step 5: An idle editor draws nothing. The surfaces keep their last
        // presented image when three things hold: the frontend reports its
        // scene quiet, the project's systems are paused, and nothing in this
        // boundary touched the renderer. With the world not advancing, that
        // image cannot be stale.
        // Anything that can change the picture - input, commands, reloads, a
        // renderer rebuild, re-cooked shaders - clears the frontend's flag or
        // is checked here.
        if self.scene_quiet
            && self.host.runtime.engine().systems_paused()
            && reloaded_extensions.is_empty()
            && matches!(renderer_step, RendererStep::Unchanged)
            && !shaders_delivered
        {
            self.scene_drawn_last_frame = false;
            return Ok(());
        }
        // After the frame's systems, so entities a startup system spawns are
        // counted too; while paused the check waits for the renderer that
        // resumes drawing.
        if self.unsupported_data_check_pending {
            self.unsupported_data_check_pending = false;
            self.report_unsupported_render_data(data_extension.as_deref());
        }
        // Draw, then hand back the answers the GPU finished: one call, so the
        // order cannot differ between the two windowed drivers.
        self.attached_renderer
            .present(self.host.runtime.engine_mut().world_mut())?;
        self.scene_drawn_last_frame = true;
        Ok(())
    }

    /// The wrapped development host.
    ///
    /// The editor and other embedded frontends reach the engine, assets and
    /// diagnostics through this one accessor. One passthrough method per call
    /// would be a second API to keep aligned with `DevHost`'s, and the methods
    /// it would mirror protect no invariant - they are straight calls.
    pub fn host(&self) -> &DevHost {
        &self.host
    }

    /// The wrapped development host, mutably.
    ///
    /// Used for the frame-boundary work frontends own: applying command
    /// batches and asset edits between frames.
    pub fn host_mut(&mut self) -> &mut DevHost {
        &mut self.host
    }
}

#[cfg(feature = "rendering")]
impl FrameDriver for RenderingHost {
    type Error = RendererError;

    fn run_frame(&mut self) -> Result<(), RendererError> {
        RenderingHost::run_frame(self)
    }

    fn engine_mut(&mut self) -> &mut pill_engine::Engine {
        self.host.engine_mut()
    }
}

#[cfg(feature = "rendering")]
impl pill_player::WindowedFrameDriver for RenderingHost {
    fn display_mut(&mut self) -> &mut AttachedRenderer {
        &mut self.attached_renderer
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Debug assertion: every entry of `subjects` names a distinct owner.
///
/// The subjects are the things a reload, a pause or a stale-suspension resume
/// can retire or reach: the extensions, the pending modules, the renderer and
/// the project. One owner shared by two of them means one of them can clear or
/// resume the other's systems.
#[cfg(debug_assertions)]
fn assert_unique_owners(subjects: &[(&str, SystemOwner)]) {
    for (index, (subject, owner)) in subjects.iter().enumerate() {
        let shared = subjects[index + 1..]
            .iter()
            .find(|(_, candidate)| candidate == owner);
        if let Some((other, _)) = shared {
            debug_assert!(
                false,
                "{subject} and {other} register under the same system owner {owner}"
            );
        }
    }
}

/// Every `include/` directory below the asset roots the loaded modules
/// declared.
///
/// A project shader may include a header a module ships, so the project's
/// cook searches these after the project's own include directories, in
/// module declaration order. A root with no `include/` below it contributes
/// nothing.
fn module_shader_include_directories() -> Vec<PathBuf> {
    let mut include_directories = Vec::new();
    for root in pill_engine::asset_store::module_roots() {
        if let Ok(layout) = pill_assets::shader_tree_layout(&root.directory) {
            include_directories.extend(layout.include_directories);
        }
    }
    include_directories
}

/// Cook every stale shader source under the project's asset directory, beside
/// itself, and log what it did.
///
/// The project's shaders follow the cooker's conventions: a source is a stage
/// by its file name, and a directory named `include` holds the headers it can
/// `#include`. The loaded modules' include directories join that path after the
/// project's own, so a project shader may include a header a module ships. The
/// output goes next to the source, which is where the shader import reads it
/// from.
///
/// A failed cook is logged and otherwise ignored. The outputs cooked before
/// the failure stay on disk, and so do the ones from an earlier run, which is
/// what the shaders already built draw.
fn cook_project_shader_sources(asset_directory: &Path) {
    let cook_started_at = Instant::now();
    let include_directories = module_shader_include_directories();
    match pill_assets::cook_shader_tree_with_include_directories(
        asset_directory,
        &include_directories,
    ) {
        Ok(report) => info!(
            target: telemetry_target::HOT_RELOAD,
            directory = %asset_directory.display(),
            include_directories = include_directories.len(),
            rebuilt = report.rebuilt_outputs.len(),
            cook_ms = cook_started_at.elapsed().as_millis() as u64,
            "[assets] cooked the project's shader sources"
        ),
        Err(error) => warn!(
            target: telemetry_target::HOT_RELOAD,
            directory = %asset_directory.display(),
            "[assets] the project's shader sources were not cooked; the cooked files on disk stay as they are: {error}"
        ),
    }
}

/// Import every known source under `asset_directory` through the world's
/// import registry, writing missing metadata files, and log what it did.
///
/// A failed asset is logged and skipped rather than failing the startup. The
/// scan is a convenience on top of the project's own loading code, which
/// reports its own errors for the assets it actually needs.
fn scan_project_assets(engine: &mut pill_engine::Engine, asset_directory: &Path) {
    let world = engine.world_mut();
    // A clone, so the registry can be called against the asset manager, which
    // is a second resource of the same world.
    let Some(registry) = world.get_resource::<pill_engine::ImportRegistry>().cloned() else {
        info!(
            target: telemetry_target::ECS,
            "[assets] scan_on_start: no imported asset types are registered; nothing to scan"
        );
        return;
    };
    let Some(assets) = world.get_resource_mut::<pill_engine::AssetManager>() else {
        return;
    };
    let report = registry.scan(
        assets,
        asset_directory,
        pill_engine::MetadataPolicy::CreateIfMissing,
    );
    for (name, error) in &report.failed {
        warn!(
            target: telemetry_target::ECS,
            asset = name.as_str(),
            "[assets] scan_on_start could not import an asset: {error}"
        );
    }
    info!(
        target: telemetry_target::ECS,
        directory = %asset_directory.display(),
        imported = report.imported.len(),
        already_loaded = report.already_loaded.len(),
        metadata_created = report.metadata_created.len(),
        failed = report.failed.len(),
        unknown_extensions = ?report.unknown_extensions,
        "[assets] scan_on_start finished"
    );
}

/// Build the world's [`pill_engine::AssetDatabase`] from the project's asset
/// directory, replacing any earlier one, and log what it indexed.
///
/// A guid two files claim is logged once per file. The second file needs a
/// guid of its own, which a `.meta` copied along with its source does not
/// have.
fn index_project_assets(engine: &mut pill_engine::Engine, asset_directory: &Path) {
    let world = engine.world_mut();
    let registry = world
        .get_resource::<pill_engine::ImportRegistry>()
        .cloned()
        .unwrap_or_default();
    let database = pill_engine::AssetDatabase::from_directory(asset_directory, &registry);
    for conflict in database.conflicts() {
        warn!(
            target: telemetry_target::ECS,
            guid = %conflict.guid,
            kept = conflict.kept.as_str(),
            refused = conflict.refused.as_str(),
            "[assets] two files claim one guid; the second is not indexed"
        );
    }
    info!(
        target: telemetry_target::ECS,
        directory = %asset_directory.display(),
        assets = database.len(),
        conflicts = database.conflicts().len(),
        "[assets] database indexed"
    );
    world.insert_resource(database);
}

/// Report a setup failure and hand the error back to the caller.
///
/// The host state built so far unwinds as soon as the error is returned, and
/// what it owns is loaded module images. Dropping one unmaps code the engine
/// may still be running, and a fault in that unmap would destroy the only copy
/// of this error. Reporting first is what keeps the failure legible when the
/// unload then crashes.
fn report_setup_failure(error: HostError) -> HostError {
    let cause_chain = format_error_chain(&error);
    // "Host setup failed" is what the suites wait for when a start must fail.
    error!(
        target: telemetry_target::ENGINE,
        error = %cause_chain,
        "Host setup failed"
    );
    error
}

/// Report a setup failure, then tear the half-built host down world-first.
///
/// Locals drop in reverse declaration order, so the loaded module images would
/// unmap while the world is still alive. A resource a module inserted keeps its
/// drop function in that module's image, so the world's teardown would call
/// into unmapped code. That is an access violation, and it can crash a failed
/// startup after it has already printed the reason.
///
/// Dropping the world first runs those drops while the images are still
/// mapped; the slots then unmap with nothing pointing into them. The error is
/// reported before either drop, so it survives whatever the teardown does.
fn fail_setup(runtime: Runtime, error: HostError) -> HostError {
    let reported = report_setup_failure(error);
    drop(runtime);
    reported
}

/// The text a tolerated first-load failure reports to the notice queue. It is
/// the captured compiler output when the build itself failed, and the editor's
/// Problems rows read `file:line` locations from it. Otherwise it is the
/// error's own words.
fn optional_failure_output(error: &HostError) -> String {
    match error {
        HostError::Build(_) => crate::build_runner::recent_build_output(),
        other => other.to_string(),
    }
}

/// Builds and loads every module, creates the engine, and starts the source
/// watchers.
///
/// # Errors
///
/// Returns a typed [`HostError`] naming the failing subsystem: configuration,
/// build, library loading, watcher startup, or managed backend startup.
pub fn setup(host_config: impl Into<HostConfig>) -> Result<DevHost, HostError> {
    // Step 1: Reject inconsistent configurations before any build or load,
    // and apply the project's log levels so the build and load below are
    // already logged at them.
    let host_config = host_config.into();
    if let Err(error) = pill_player::apply_logging_settings(&host_config.logging) {
        pill_core::warn!(
            target: telemetry_target::ENGINE,
            "the project's logging settings were not applied: {error}"
        );
    }
    let project_module_config = host_config.project;
    project_module_config.validate()?;
    for module in &host_config.extensions {
        module.validate()?;
    }

    // Step 2: Prepare the workspace. Find its root, delete what an earlier run
    // left behind, create the engine, and build every native module.
    //
    // The engine owns this root (see [`crate::config::engine_workspace_root`]),
    // so it does not depend on the directory the process was launched from.
    let workspace_root = crate::config::engine_workspace_root()?;

    print_welcome(
        &host_config.name,
        host_config.project_directory.as_deref(),
        &project_module_config,
    );
    print_startup_configuration(&workspace_root, &project_module_config);
    // Which watchers this run may start, read once: a hard case to debug when
    // it is silent, so every start says it out loud.
    let switches = crate::switches::HostSwitches::from_environment();
    switches.log();

    // Delete what an earlier run staged, whichever backend this is.
    //
    // The staging directory holds copies of loaded modules, the engine
    // libraries staged beside them, and the hot-patch scratch images. None of
    // it belongs to the build starting now. A C# host stages native copies
    // for its extensions too, so the sweep cannot depend on the backend.
    cleanup_temporary_files(&workspace_root);

    // Construct the engine and its stable API table.
    // EngineApi stores a raw pointer into the runtime's boxed engine, whose
    // address does not change when the runtime moves.
    let mut runtime = Runtime::new();
    runtime.engine_mut().set_parallel_execution(true);
    // The settings' startup scene is queued here, before any artifact loads.
    // The project's entry point takes it between component registration and
    // its own `init`, so a project that seeds its world finds the scene
    // already in it instead of duplicating it.
    if let Some(startup_scene) = host_config.startup_scene.as_deref() {
        pill_engine::queue_startup_scene(runtime.engine_mut().world_mut(), startup_scene);
    }
    // Debug drawing is always on in the development host; the settings'
    // `debug_draw:` key is what a shipping build starts with.
    pill_player::insert_debug_resources(runtime.engine_mut().world_mut(), true);
    // The pass that draws what a component declares as its own gizmo. An
    // editor's views show it as chrome, which a game surface never draws; a
    // game window shows it as plain debug drawing.
    pill_engine::gizmos::install(
        runtime.engine_mut(),
        if host_config.load_editor_crates {
            pill_engine::gizmos::GizmoChrome::Editor
        } else {
            pill_engine::gizmos::GizmoChrome::Game
        },
    );
    let engine_api = EngineApi::new(runtime.engine_mut());

    // Announce every module this start builds, in build order: the
    // extensions, the project, and - in a windowed host - the renderer's GPU
    // module, which loads once the window opens.
    let mut planned_builds: Vec<crate::build_progress::PlannedBuild> = host_config
        .extensions
        .iter()
        .map(|extension| crate::build_progress::PlannedBuild {
            module: extension.name.clone(),
            kind: "extension",
        })
        .collect();
    planned_builds.push(crate::build_progress::PlannedBuild {
        module: project_module_config.name.clone(),
        kind: "project",
    });
    if cfg!(feature = "rendering") {
        if let Some(renderer) = &host_config.renderer {
            planned_builds.push(crate::build_progress::PlannedBuild {
                module: renderer.clone(),
                kind: "renderer",
            });
        }
    }
    if let Some(project_editor) = &host_config.project_editor {
        planned_builds.push(crate::build_progress::PlannedBuild {
            module: project_editor.name.clone(),
            kind: "editor",
        });
    }
    crate::build_progress::announce_plan(&host_config.name, planned_builds);

    // Build every native module in one cargo invocation: the extensions, the
    // project, and in a windowed host the renderer's wrapper, an ordinary
    // wrapper build whose load waits for the window. Built one at a time, each
    // module would pay cargo's start-up and dependency resolution again, so
    // startup time would grow with the number of modules. A module the batch
    // built skips its own build when it loads below (Step 3); a later edit
    // still rebuilds it alone.
    crate::build_runner::build_extension_batch(
        &workspace_root,
        &host_config.extensions,
        Some(&project_module_config),
        if cfg!(feature = "rendering") {
            host_config.renderer.as_deref()
        } else {
            None
        },
        host_config.project_editor.as_ref(),
    );

    // Step 3: Build, load and watch the extensions before the project.
    // They are infrastructure, so loading them first lets the project rely on
    // whatever they register. When the project selects a renderer, its data
    // crate is the first extension (see `HostConfig::renderer`). The project
    // and the other extensions then find its components registered, and the C#
    // bridge binds them like any extension's, headless included.
    //
    // Each module gets its own owner tag and its own generation counter, which
    // is what keeps later reloads from affecting each other.
    let mut extensions = Vec::with_capacity(host_config.extensions.len());
    let mut pending_modules: Vec<PendingModule> = Vec::new();
    // One patch session per LOADED extension, index-aligned with
    // `extensions`, because the frame's fast path reads both lists by the same
    // index. A module whose first load failed - or waits for a retry - has
    // none until it loads.
    let mut module_hot_patch: Vec<Option<crate::hot_patch::HotPatchSession>> = Vec::new();
    for (index, extension_config) in host_config.extensions.iter().enumerate() {
        let module_generation = Arc::new(AtomicU64::new(0));
        let slot = match ExtensionSlot::start(
            runtime.engine_mut(),
            &engine_api,
            &workspace_root,
            extension_config,
            extension_owner(index),
            Arc::clone(&module_generation),
            crate::reload::FirstLoadFailure::ClearWorld,
        ) {
            Ok(slot) => slot,
            Err(error) => {
                // An editor-only module the embedder asked to tolerate. Report
                // the failure the way a failed reload does and carry on: the
                // editor's safe mode owns what happens next, and
                // `DevHost::retry_extension` can build it again later.
                if host_config
                    .first_load_optional
                    .iter()
                    .any(|name| name == &extension_config.name)
                {
                    warn!(
                        target: telemetry_target::HOT_RELOAD,
                        module = extension_config.name.as_str(),
                        error = %error,
                        "optional module failed to load; the host continues without it"
                    );
                    notices::push(HostNotice::ModuleFailed {
                        module: extension_config.name.clone(),
                        output: optional_failure_output(&error),
                    });
                    pending_modules.push(PendingModule {
                        config: extension_config.clone(),
                        owner: extension_owner(index),
                        source_edit_generation: module_generation,
                    });
                    continue;
                }
                return Err(fail_setup(runtime, error));
            }
        };
        // A run with source watching switched off still loads the extension:
        // the switch stops the watcher, not the posture.
        if switches.hot_reloading {
            if let Err(error) = spawn_source_watcher(
                workspace_root.clone(),
                &extension_config.name,
                &extension_config.watch_directory,
                module_generation,
                None,
            ) {
                return Err(fail_setup(runtime, error.into()));
            }
        }
        extensions.push(slot);
        let module_patch_session = if switches.function_hot_patching {
            crate::hot_patch::HotPatchSession::new(
                &workspace_root,
                &extension_config.name,
                &extension_config.watch_directory,
                &extension_config.output_subdirectory,
                &extension_config.build_command,
            )
        } else {
            None
        };
        module_hot_patch.push(module_patch_session);
    }
    // A project's shaders name their vertex stage under a renderer's
    // `res/shaders`, and the project initializes before any renderer module
    // loads - a `renderer: none` project loads none at all. Declaring the tree
    // here is what makes those paths answerable in every posture; the renderer
    // declares the same root when it loads, which adds the files it cannot run
    // without and hides the cooked output. A packed build has no module
    // directory, and its pack already carries these files.
    let shader_tree_crate = host_config
        .renderer
        .as_deref()
        .unwrap_or(crate::config::DEFAULT_RENDERER);
    let resources_directory = module_resources_directory(&workspace_root, shader_tree_crate);
    if resources_directory.is_dir() {
        pill_engine::asset_store::declare_module_root(shader_tree_crate, resources_directory, &[]);
    }
    // By now every extension has declared the read-only resource folders it
    // ships. Check them here, not inside the module initialization, which runs
    // under `catch_unwind`. A file that went missing then stops the host and
    // names the module and the path, instead of leaving a scene that draws
    // nothing and says nothing.
    pill_engine::asset_store::verify_module_roots();

    // Step 4: Build and load the project module, then watch its sources.
    // Extensions load first so the C# backend can be handed their native
    // components. A module's registered type names become C# names by
    // replacing `::` with `.`. That is the same stable identity a mirror
    // struct in `project_cs` uses.
    //
    // Those mirror files have to exist before the project build compiles
    // `project_cs`. Write them here, managed backend only, one per extension
    // that exposes components. They come from each module's real registered
    // layout; nothing in the project is written manually.
    let mut module_exposed_components: Vec<ModuleExposedComponent> = Vec::new();
    let mut all_mirror_methods: Vec<crate::csharp::ResolvedMirrorMethod> = Vec::new();
    // Which module's file carries which component, decided once over the whole
    // load order: a shared component appears in every linking module's log.
    let mirror_ownership = module_mirror_ownership(runtime.engine(), &extensions);
    if let ProjectModuleBackend::CSharp(_) = &project_module_config.backend {
        for (index, slot) in extensions.iter().enumerate() {
            // Regenerate the module's C# mirror from its current generation.
            // The returned change flag is ignored at startup, because the
            // mirror is always written before the project compiles. The reload
            // path uses it to decide whether to queue a C# project rebuild.
            let (exposed, methods, accessors, _changed) = regenerate_module_csharp_mirror(
                &workspace_root,
                slot,
                mirror_ownership[index].clone(),
            )?;
            module_exposed_components.extend(exposed);
            all_mirror_methods.extend(methods);
            // Container accessors are appended to the mirror-method table, so
            // the managed runtime resolves them through the same lookup and the
            // same per-reload refresh as value-type methods.
            all_mirror_methods.extend(crate::csharp::accessor_rows(&accessors));
        }
    }
    // The project's shaders have no build script of their own to cook them, and
    // the shader importer reads the cooked file rather than the source. Cook
    // them before the project module initializes, which is when the project
    // imports the shaders it names.
    if let Some(asset_directory) = &host_config.asset_directory {
        cook_project_shader_sources(asset_directory);
    }
    let loaded_project = match LoadedProject::start(
        runtime.engine_mut(),
        &engine_api,
        &workspace_root,
        &project_module_config,
        &module_exposed_components,
        &all_mirror_methods,
    ) {
        Ok(project) => project,
        Err(error) => return Err(fail_setup(runtime, error)),
    };

    // Step 4b: the project's own editor crate, when it has one. It loads after
    // the project because it exists to change the editor the project runs in,
    // and its failure costs the editor part only: the project, its world and
    // every extension keep running, so a person can fix the crate without
    // losing the session. The failed image stays loaded and only what it
    // registered is cleared, which is what `ClearGenerationData` says.
    let project_editor_generation = Arc::new(AtomicU64::new(0));
    let project_editor = match host_config.project_editor.as_ref() {
        None => None,
        Some(configuration) => {
            let loaded = ExtensionSlot::start(
                runtime.engine_mut(),
                &engine_api,
                &workspace_root,
                configuration,
                SystemOwner::PROJECT_EDITOR,
                Arc::clone(&project_editor_generation),
                crate::reload::FirstLoadFailure::ClearGenerationData,
            );
            match loaded {
                Ok(slot) => {
                    info!(
                        target: telemetry_target::HOT_RELOAD,
                        module = configuration.name.as_str(),
                        "the project's editor crate loaded"
                    );
                    Some(slot)
                }
                Err(error) => {
                    warn!(
                        target: telemetry_target::HOT_RELOAD,
                        module = configuration.name.as_str(),
                        error = %error,
                        "the project's editor crate failed to load; the project keeps running without it"
                    );
                    notices::push(HostNotice::ModuleFailed {
                        module: configuration.name.clone(),
                        output: optional_failure_output(&error),
                    });
                    None
                }
            }
        }
    };

    let source_edit_generation = Arc::new(AtomicU64::new(0));
    let source_trigger_log = Arc::new(crate::watchers::TriggerLog::new());
    if switches.hot_reloading {
        if let Err(error) = spawn_source_watcher(
            workspace_root.clone(),
            &project_module_config.name,
            &project_module_config.watch_directory,
            Arc::clone(&source_edit_generation),
            Some(Arc::clone(&source_trigger_log)),
        ) {
            return Err(fail_setup(runtime, error.into()));
        }
    } else {
        info!(
            target: telemetry_target::ENGINE,
            variable = crate::switches::HOT_RELOADING_VARIABLE,
            "source watching is switched off; edits are not noticed until the host restarts"
        );
    }

    // The project's own editor crate gets its own watcher, so an edit inside
    // it reloads that crate alone instead of waiting for the project to change.
    if switches.hot_reloading {
        if let Some(configuration) = host_config.project_editor.as_ref() {
            if let Err(error) = spawn_source_watcher(
                workspace_root.clone(),
                &configuration.name,
                &configuration.watch_directory,
                Arc::clone(&project_editor_generation),
                Some(Arc::clone(&source_trigger_log)),
            ) {
                return Err(fail_setup(runtime, error.into()));
            }
        }
    }

    // Import the project's source assets when its settings ask for it, once
    // every module has registered its imported asset types.
    if host_config.assets.scan_on_start {
        if let Some(asset_directory) = &host_config.asset_directory {
            scan_project_assets(runtime.engine_mut(), asset_directory);
        }
    }

    // Index every asset of the project by guid and by path, loaded or not, so
    // a reference can find and load one. After the module and project
    // registrations, which say which extensions are standalone.
    if let Some(asset_directory) = &host_config.asset_directory {
        index_project_assets(runtime.engine_mut(), asset_directory);
    }

    // Step 5: Snapshot host memory and print the startup analytics report.
    // Every module has been built, staged, loaded and initialized by now, so
    // the table carries the complete startup picture.
    analytics::record_host_memory();
    analytics::print_startup_report();

    info!(
        target: telemetry_target::ENGINE,
        "Entering project loop. Edit {}/**/* to hot-reload",
        project_module_config.watch_directory
    );

    // Say how to drive rollback, once, next to where the fast path announces
    // itself - an interface nothing mentions is one nobody uses. Rollback only
    // exists for patches, so it is not mentioned when patching is off.
    if switches.function_hot_patching {
        info!(
            target: telemetry_target::HOT_RELOAD,
            "Patch rollback: write `function@generation`, `function@previous` or `list` to {}",
            ROLLBACK_REQUEST_FILE
        );
    }

    // Arm the per-function fast path. It reads the project's sources for
    // `#[pill_hot]` annotations and returns `None` when there is nothing to do,
    // so a project that has not opted in pays nothing. A run with patching
    // switched off arms no session at all: the fast path then has nothing to
    // build against and every edit takes the reload below.
    let hot_patch = if switches.function_hot_patching {
        crate::hot_patch::HotPatchSession::new(
            &workspace_root,
            &project_module_config.name,
            &project_module_config.watch_directory,
            crate::build_runner::PROJECT_HOT_OUTPUT_SUBDIRECTORY,
            &project_module_config.build_command,
        )
    } else {
        None
    };

    // The per-extension fast path's sessions were built with the slots they
    // mirror (see the load loop). The fast path reads `module_hot_patch` beside
    // `extensions`, so the two stay index-aligned.

    // The project's asset directory is mounted before anything reads
    // through it: relative asset paths resolve below it first, and the
    // authoring commands (the editor's asset panel) write into it. A host
    // without one - a shipped bundle, a project whose root is not known -
    // keeps reading through the packs and the development fallbacks.
    if let Some(directory) = &host_config.asset_directory {
        pill_engine::asset_store::mount_directory(directory.clone());
    }
    // The project's managers take their settings now: every manager exists by
    // this point, because the project's `init` is the last artifact to
    // register one. A project whose `res/settings` holds no file for a manager
    // gets one written from the manager's own document defaults.
    apply_manager_settings_at_startup(
        runtime.engine_mut().world_mut(),
        host_config.asset_directory.as_deref(),
    );
    let host = DevHost {
        workspace_root,
        project_module_config,
        #[cfg(feature = "rendering")]
        renderer: host_config.renderer.clone(),
        #[cfg(feature = "rendering")]
        renderer_data: host_config.renderer_data.clone(),
        runtime,
        engine_api,
        loaded_project,
        project_editor,
        extensions,
        pending_modules,
        source_edit_generation,
        last_processed_source_edit: 0,
        queued_reload_generation: 0,
        last_processed_queued_reload: 0,
        source_trigger_log,
        stale_suspended_owners: Vec::new(),
        hot_patch,
        module_hot_patch,
        patch_attempt: None,
        loaded_patches: Vec::new(),
        asset_watcher: start_asset_watcher(
            host_config.asset_directory.as_deref(),
            switches.assets_hot_reloading,
        ),
        asset_directory: host_config.asset_directory.clone(),
        manager_settings_editor_revision: 0,
        ensure_asset_metadata: false,
        editor_revision: 0,
        switches,
        patch_switched_off_logged: false,
    };

    #[cfg(debug_assertions)]
    host.assert_owner_isolation();
    Ok(host)
}

/// Sets up the engine, project module, hot reload, and renderer together.
///
/// A frontend owns its platform event loop and supplies its cloneable window
/// handle. The engine creates exactly one surface for that window, while the
/// returned [`RenderingHost`] owns the renderer for the rest of its lifetime.
///
/// # Errors
///
/// Returns the composed [`RenderingError`](pill_player::RenderingError),
/// which transparently carries either a [`HostError`] from setup or a
/// [`RendererError`] from surface creation.
#[cfg(feature = "rendering")]
pub fn setup_rendering<W>(
    project: impl Into<HostConfig>,
    window: W,
    width: u32,
    height: u32,
) -> Result<RenderingHost, pill_player::RenderingError>
where
    W: RendererWindow + 'static,
{
    let host = setup(project.into())?;
    attach_renderer(host, window, width, height)
}

/// Whether rendering should be paused after one frame boundary.
///
/// `paused` is the state before it. `data_layout_changed` says whether the
/// renderer's data crate reloaded this boundary with a different component
/// layout. `step` is what the renderer step did.
///
/// A renderer that attached successfully always resumes. A layout change
/// without one pauses, and anything else keeps the current state.
#[cfg(feature = "rendering")]
fn pause_after_boundary(
    paused: Option<PauseReason>,
    data_layout_changed: bool,
    step: RendererStep,
) -> Option<PauseReason> {
    match step {
        RendererStep::Attached => None,
        RendererStep::Failed | RendererStep::Unchanged if data_layout_changed => {
            Some(PauseReason::StaleRendererAfterDataLayoutChange)
        }
        RendererStep::Failed | RendererStep::Unchanged => paused,
    }
}

/// The structural hash of every component the named extension registered,
/// by component name, or empty when no such extension is loaded.
///
/// Taken before and after a reload of the renderer's data crate. It tells a
/// layout change, where the renderer must not read the rows until it is
/// rebuilt, from a body-only one, where it may.
#[cfg(feature = "rendering")]
fn component_schemas_of(host: &DevHost, extension: &str) -> Vec<(String, Option<u64>)> {
    let Some(slot) = host.extensions.iter().find(|slot| slot.name() == extension) else {
        return Vec::new();
    };
    let world = host.runtime.engine().world();
    let mut schemas: Vec<(String, Option<u64>)> = slot
        .exposed_component_names()
        .iter()
        .map(|name| {
            let schema = world
                .resolve_component_id_by_name_any(name)
                .ok()
                .flatten()
                .and_then(|component| world.component_field_layout(component))
                .filter(|fields| !fields.is_empty())
                .map(pill_engine::component::component_schema_hash);
            (name.clone(), schema)
        })
        .collect();
    schemas.sort();
    schemas.dedup();
    schemas
}

/// The components of `world` that at least one entity carries, that
/// `registered_component_names` lists, and that `consumed_component_names` does
/// not name. Each comes with its entity count, sorted by name and listed
/// once.
///
/// See [`RenderingHost::report_unsupported_render_data`] for why only shared
/// components are candidates.
#[cfg(feature = "rendering")]
fn unsupported_render_components(
    world: &pill_engine::World,
    registered_component_names: &[String],
    consumed_component_names: &[String],
) -> Vec<(String, usize)> {
    let mut names: Vec<&String> = registered_component_names.iter().collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .filter(|name| !consumed_component_names.contains(name))
        .filter_map(|name| {
            let component = world
                .resolve_component_id_by_name_any(name)
                .ok()
                .flatten()?;
            if !matches!(component, pill_engine::ComponentId::Shared(_)) {
                return None;
            }
            let entities = world.live_row_count(component);
            (entities > 0).then(|| (name.clone(), entities))
        })
        .collect()
}

/// The `res` directory of the module `module`, whether or not it is loaded yet.
///
/// The renderer module loads last - only a windowed host loads it, and it
/// attaches to a window the project has already started - while a project's
/// shaders name stages under the renderer's `res/shaders`. Both the startup
/// declaration of that tree and the shader watcher resolve it from the crate
/// directory, which the workspace layout fixes.
fn module_resources_directory(workspace_root: &Path, module: &str) -> PathBuf {
    workspace_root
        .join(ExtensionConfig::located_workspace_member(workspace_root, module).crate_directory)
        .join(crate::config::MODULE_RESOURCES_DIRECTORY)
}

/// Start watching the renderer module's shader sources, when the host has a
/// renderer and it has a `res/` directory.
///
/// A watch that cannot start is reported and leaves the host without shader
/// reload; nothing else depends on it.
#[cfg(feature = "rendering")]
fn start_shader_watcher(host: &DevHost) -> Option<crate::watchers::ShaderWatcher> {
    if !host.switches.assets_hot_reloading {
        info!(
            target: telemetry_target::HOT_RELOAD,
            variable = crate::switches::ASSETS_HOT_RELOADING_VARIABLE,
            "shader hot reloading is switched off; shader edits are not noticed until the host restarts"
        );
        return None;
    }
    let renderer = host.renderer.clone()?;
    // The tree the renderer cooks from, its `res` at the crate root. The build
    // script and the watcher have to name the same directory, or a shader edit
    // changes a file that nothing rebuilds, and the paths the watcher reports
    // relative to it are the stages' asset paths.
    let shaders_directory = module_resources_directory(&host.workspace_root, &renderer);
    match crate::watchers::ShaderWatcher::spawn(&renderer, shaders_directory, Vec::new()) {
        Ok(watcher) => watcher,
        Err(error) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = renderer.as_str(),
                "shader reload is off: the shader watcher could not start: {error}"
            );
            None
        }
    }
}

/// The renderer module a windowed host loads: the extension slot it lives in.
#[cfg(feature = "rendering")]
type RendererModule = crate::renderer_module::RendererModule;

/// Start watching the project's own shader sources.
///
/// A project has no build script to cook its shaders, so the watcher that
/// keeps the renderer data crate's tree current keeps the project's too. The
/// tree is the project's asset directory, the same root the asset watcher
/// watches and the same conventions the cooker applies to a module's `res/`.
///
/// A watch that cannot start is reported and leaves the host running the
/// shaders it already loaded.
#[cfg(feature = "rendering")]
fn start_project_shader_watcher(host: &DevHost) -> Option<crate::watchers::ShaderWatcher> {
    if !host.switches.assets_hot_reloading {
        info!(
            target: telemetry_target::HOT_RELOAD,
            variable = crate::switches::ASSETS_HOT_RELOADING_VARIABLE,
            "project shader hot reloading is switched off; shader edits are not noticed until the host restarts"
        );
        return None;
    }
    let shaders_directory = host.asset_directory()?.to_owned();
    let tree_label = host.project_module_config.name.clone();
    let include_directories = module_shader_include_directories();
    match crate::watchers::ShaderWatcher::spawn(&tree_label, shaders_directory, include_directories)
    {
        Ok(watcher) => watcher,
        Err(error) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = tree_label.as_str(),
                "project shader reload is off: the shader watcher could not start: {error}"
            );
            None
        }
    }
}

/// Load the renderer module and attach it to `window`.
///
/// The module registers under the engine's reserved renderer owner. Its
/// `rendering` system is then cleared and registered again with the module
/// instead of living forever under the engine's owner. No extension in the
/// same host can share that owner.
///
/// A renderer that did not load, or loaded but could not attach, leaves the
/// window blank instead of ending the host. The scene keeps running, and the
/// next renderer edit loads and attaches again.
///
/// # Errors
///
/// Returns a [`HostError`] when the module cannot be started, and a
/// [`RendererError`] when the window cannot give its handles.
#[cfg(feature = "rendering")]
fn attach_renderer_module<W: RendererWindow>(
    host: &mut DevHost,
    window: W,
    width: u32,
    height: u32,
) -> Result<(RendererModule, AttachedRenderer), pill_player::RenderingError> {
    let renderer_module = crate::renderer_module::RendererModule::start(
        host.renderer.as_deref(),
        host.runtime.engine_mut(),
        &host.engine_api,
        &host.workspace_root,
        pill_player::registration::renderer_owner(),
    )?;
    // The renderer is one more subject. Its owner must be outside every
    // extension's, including the ones a failed first load still holds in
    // reserve.
    #[cfg(debug_assertions)]
    {
        let mut subjects = host.subject_owners();
        subjects.push(("the renderer", renderer_module.owner()));
        assert_unique_owners(&subjects);
    }
    let attached_renderer = AttachedRenderer::attach(window, width, height, |window_data| {
        if renderer_module.slot().is_none() {
            // Already logged by the failed load.
            return Ok(Box::new(pill_renderer_api::HeadlessRenderer));
        }
        Ok(
            attach_backend(&renderer_module, window_data, width, height).unwrap_or_else(
                |failure| {
                    error!(
                        target: telemetry_target::HOT_RELOAD,
                        "the renderer could not attach to the window; it stays blank until the next renderer edit: {failure}"
                    );
                    Box::new(pill_renderer_api::HeadlessRenderer)
                },
            ),
        )
    })?;
    Ok((renderer_module, attached_renderer))
}

/// Build a backend on `window_data` from the loaded renderer module.
///
/// # Errors
///
/// Returns a [`RendererError`] when the module cannot attach.
#[cfg(feature = "rendering")]
fn attach_backend(
    renderer_module: &RendererModule,
    window_data: pill_renderer_api::RawWindowData,
    width: u32,
    height: u32,
) -> Result<Box<dyn PillRenderer>, RendererError> {
    // SAFETY: every caller stores the backend in the `AttachedRenderer` of a
    // `RenderingHost`, which drops the backend before its window. Its
    // `attached_renderer` field is declared before `renderer_module`, so the
    // backend drops while the window is alive and the module is mapped.
    unsafe {
        crate::renderer_module::attach_module_renderer(renderer_module, window_data, width, height)
    }
}

/// Complete rendering setup from an already-built [`DevHost`], attaching the
/// engine renderer to a supplied native window.
///
/// Frontends that must finish project setup (building and loading the project
/// module) before any window exists call this after [`setup`], so a slow first
/// build never shows a blank surface. The standalone runner is one.
///
/// # Errors
///
/// Returns the composed [`RenderingError`](pill_player::RenderingError):
/// a [`HostError`] when the renderer module cannot be built, loaded or
/// initialized, or a [`RendererError`] when attaching it to the window fails.
#[cfg(feature = "rendering")]
pub fn attach_renderer<W>(
    mut host: DevHost,
    window: W,
    width: u32,
    height: u32,
) -> Result<RenderingHost, pill_player::RenderingError>
where
    W: RendererWindow + 'static,
{
    info!(
        target: telemetry_target::RENDERING,
        width,
        height,
        "attaching the engine renderer to the window surface"
    );
    let (renderer_module, attached_renderer) =
        attach_renderer_module(&mut host, window, width, height)?;
    let shader_watcher = start_shader_watcher(&host);
    let project_shader_watcher = start_project_shader_watcher(&host);
    Ok(RenderingHost {
        host,
        attached_renderer,
        renderer_module,
        paused: None,
        scene_quiet: false,
        scene_drawn_last_frame: true,
        shader_watcher,
        project_shader_watcher,
        unsupported_data_check_pending: true,
    })
}

/// Forget the patch records a replaced image invalidated: the saved prologue
/// bytes and every active generation.
///
/// A prologue patch overwrote bytes inside a loaded artifact. Once that
/// artifact is replaced, the recorded addresses point into an image the
/// graveyard will unmap. Writing saved bytes back to one is then a write into
/// freed, possibly reused memory, not a rollback.
///
/// Every session is cleared, not only the one whose artifact reloaded. Patches
/// fan out across every loaded artifact, so a module's records include
/// addresses inside the project image and cannot be cleared one at a time. A
/// partial restore would leave two copies of one function disagreeing.
///
/// The active generation of every function goes too. The rebuilt artifact
/// ships its own body, and an entry left behind would name a generation that
/// is not running. Calling it twice is safe.
fn forget_prologue_records(host: &mut DevHost) {
    if let Some(session) = host.hot_patch.as_mut() {
        session.forget_prologue_patches();
    }
    for session in host.module_hot_patch.iter_mut().flatten() {
        session.forget_prologue_patches();
    }
}

/// Arm the thread that owns the frame boundary as the only one allowed to
/// rewrite live code.
///
/// Declared per frame rather than at setup because setup may run on a different
/// thread; idempotent, and a single relaxed load once declared.
fn arm_patching_thread() {
    pill_engine::hot_patch::declare_patching_thread();
}

/// Which session an in-flight patch build belongs to.
#[derive(Clone, Copy)]
enum PatchSubject {
    /// The project's own session.
    Project,
    /// The extension at this index.
    Module(usize),
}

/// One patch build running on its own thread.
///
/// The build compiles and maps the patch; installing it stays on the frame
/// thread, which is what [`advance_patch_attempt`] does when the report
/// arrives. The thread is joined on collection, on teardown, and nowhere else -
/// a build that outlived its host would be a thread still running code that is
/// about to be unloaded.
struct InFlightPatch {
    /// Which session the build is for, and who consumes its pending
    /// generation once the patch is live.
    subject: PatchSubject,
    /// The generation counter captured when the build started. A save that
    /// arrives while it compiles advances the counter past this value and must
    /// stay pending.
    pending_generation: u64,
    /// The build's report, once it has one.
    receiver: mpsc::Receiver<(
        crate::hot_patch::PatchAttempt,
        Vec<crate::hot_patch::BodyOutcome>,
    )>,
    /// The worker, joined when the report is collected or this value drops.
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for InFlightPatch {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Start a patch build on its own thread.
///
/// The attempt travels through its own channel rather than the closure's
/// capture. A thread that cannot start therefore leaves it here to build
/// inline, instead of losing it with the closure.
fn start_patch_build(
    attempt: crate::hot_patch::PatchAttempt,
    subject: PatchSubject,
    pending_generation: u64,
) -> InFlightPatch {
    let (report_sender, receiver) = mpsc::channel();
    let fallback_sender = report_sender.clone();
    let (job_sender, job_receiver) = mpsc::channel::<crate::hot_patch::PatchAttempt>();
    let thread = thread::Builder::new()
        .name("pill-patch-build".to_string())
        .spawn(move || {
            let Ok(attempt) = job_receiver.recv() else {
                return;
            };
            let report = crate::hot_patch::build_attempt(attempt);
            // A receiver that has gone away means the host is shutting down;
            // the report's job is then nobody's.
            let _ = report_sender.send(report);
        });
    match thread {
        Ok(thread) => {
            // Sent after the thread exists; it blocks on this receive.
            let _ = job_sender.send(attempt);
            InFlightPatch {
                subject,
                pending_generation,
                receiver,
                thread: Some(thread),
            }
        }
        Err(_) => {
            // Building on this thread blocks the frame, which is the safe
            // direction for a spawn failure.
            let report = crate::hot_patch::build_attempt(attempt);
            let _ = fallback_sender.send(report);
            InFlightPatch {
                subject,
                pending_generation,
                receiver,
                thread: None,
            }
        }
    }
}

/// Collect a finished patch build and install it at this frame boundary.
///
/// Activation happens here rather than on the worker, because a dispatch slot
/// write belongs to the thread that owns the frame boundary. The worker only
/// compiled and mapped the image. While the build runs, nothing marks the edit
/// handled, so the frame loop still owes it a reload.
///
/// Returns `true` when this frame must not start another attempt. A build that
/// came back without installing its patch leaves the edit for the reload path
/// below, and without that a patch that cannot compile is retried forever. A
/// collected success returns `false`, because a save that arrived while the
/// build ran is a fresh edit the fast path may handle immediately.
///
/// The second element is the attempt's own start, so this frame's analytics
/// total can span the build. It is `None` when nothing was collected.
fn advance_patch_attempt(host: &mut DevHost) -> (bool, Option<Instant>) {
    let Some(mut in_flight) = host.patch_attempt.take() else {
        return (false, None);
    };
    let (attempt, outcomes) = match in_flight.receiver.try_recv() {
        Ok(report) => report,
        Err(mpsc::TryRecvError::Empty) => {
            // Still compiling; put the state back and let the frame run.
            host.patch_attempt = Some(in_flight);
            return (false, None);
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            // The worker died without a report. Nothing has marked the edit
            // handled, so the frame loop falls back to the reload path, where
            // a refusal would have led. A fresh attempt must not race it this
            // frame.
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "the patch build stopped unexpectedly; falling back to a reload"
            );
            return (true, None);
        }
    };
    if let Some(thread) = in_flight.thread.take() {
        let _ = thread.join();
    }
    // The transaction this frame's analytics line describes began when this
    // attempt did: the build ran between frames, so the frame collecting it
    // cannot time the total from its own start.
    let attempt_started = attempt.started;

    let patched;
    {
        let DevHost {
            hot_patch,
            module_hot_patch,
            extensions,
            loaded_patches,
            loaded_project,
            runtime,
            ..
        } = &mut *host;
        let engine = runtime.engine_mut();
        let session = match in_flight.subject {
            PatchSubject::Project => hot_patch.as_mut(),
            PatchSubject::Module(index) => module_hot_patch
                .get_mut(index)
                .and_then(|slot| slot.as_mut()),
        };
        let Some(session) = session else {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "the patch build's session is gone; falling back to a reload"
            );
            return (true, Some(attempt_started));
        };
        let targets = patch_targets(loaded_project, extensions);
        let outcome = session.activate_attempt(engine, &targets, loaded_patches, attempt, outcomes);
        // The borrow of the module list ends here, so the slot below can be
        // updated.
        drop(targets);
        patched = report_patch_outcome(outcome);
        if patched {
            // The generation this build was started for is handled now; a
            // save that arrived while it compiled advanced the counter past it
            // and stays pending.
            if let PatchSubject::Module(index) = in_flight.subject {
                if let Some(slot) = extensions.get_mut(index) {
                    slot.consume_pending_reload(in_flight.pending_generation);
                }
            }
        }
    }
    if patched {
        if matches!(in_flight.subject, PatchSubject::Project) {
            host.last_processed_source_edit = in_flight.pending_generation;
        }
        // A patch replaced live code; the editor must refresh its metadata.
        host.bump_editor_revision();
    }
    (!patched, Some(attempt_started))
}

/// Whether a patch build is running on its own thread right now.
fn patch_attempt_in_flight(host: &DevHost) -> bool {
    host.patch_attempt.is_some()
}

/// Whether the patch fast path may run for this edit.
///
/// `PILL_FUNCTION_HOT_PATCHING=0` turns it off for the whole run: every watched
/// edit is left to the rebuild-and-reload path, which is what makes a
/// breakpoint in a system body keep binding. The answer is the same on every
/// frame, so it is said once, when the first edit asks.
fn fast_patching_allowed(host: &mut DevHost) -> bool {
    if host.switches.function_hot_patching {
        return true;
    }
    if !host.patch_switched_off_logged {
        host.patch_switched_off_logged = true;
        info!(
            target: telemetry_target::HOT_RELOAD,
            variable = crate::switches::FUNCTION_HOT_PATCHING_VARIABLE,
            "per-function hot patching is switched off; a watched edit rebuilds and reloads instead"
        );
    }
    false
}

/// Try to deliver every pending extension edit by patching, not rebuilding.
///
/// A module's plain functions are compiled into every artifact that links the
/// crate, so one patch is offered to all of them at once. That is what makes the
/// cascading project reload a module swap normally queues unnecessary: the
/// project's embedded copy is redirected too.
///
/// Anything patching refuses falls straight through to the full rebuild the
/// frame loop performs next, so the worst case is that rebuild.
///
/// A no-op without the `development` feature, so the frame loop reads the same in
/// both configurations rather than carrying a `cfg` of its own.
fn try_module_fast_path(host: &mut DevHost, may_start: bool) {
    if !fast_patching_allowed(host) {
        return;
    }
    // `may_start` is false for the frame that just collected a failed build:
    // the reload below owes the edit, and a fresh attempt would race it
    // forever.
    if !may_start || host.patch_attempt.is_some() {
        return;
    }
    // Disjoint field borrows, as the reload steps below do.
    let DevHost {
        extensions,
        module_hot_patch,
        patch_attempt,
        ..
    } = &mut *host;

    for index in 0..module_hot_patch.len() {
        // Captured before the attempt begins. A save that arrives while it
        // compiles advances the counter past this value and must stay
        // pending, because nothing has delivered it.
        let Some(pending_generation) = extensions[index].pending_reload_generation() else {
            continue;
        };
        let Some(session) = module_hot_patch[index].as_mut() else {
            continue;
        };
        match session.begin_attempt() {
            BeginOutcome::Unchanged => {}
            BeginOutcome::NotPatchable { refusal } => {
                report_patch_outcome(PatchOutcome::NotPatchable { refusal });
            }
            BeginOutcome::Failed {
                function,
                active_generation,
                failure,
            } => {
                report_patch_outcome(PatchOutcome::Failed {
                    function,
                    active_generation,
                    failure,
                });
            }
            BeginOutcome::Ready(attempt) => {
                // The build runs on its own thread; the pending generation
                // counts as handled only once its replacement is installed,
                // and every reload step waits for that outcome.
                *patch_attempt = Some(start_patch_build(
                    attempt,
                    PatchSubject::Module(index),
                    pending_generation,
                ));
                break;
            }
        }
    }
}

/// Try to deliver a pending project edit by patching, not rebuilding.
///
/// This runs at a frame boundary. Reloads are already processed here, before
/// `process_frame`, so no system is executing while a dispatch slot is written.
/// A successful patch marks the pending generation handled, which is what skips
/// the full rebuild. Anything refused falls through to it.
fn try_project_fast_path(host: &mut DevHost, may_start: bool) {
    if !fast_patching_allowed(host) {
        return;
    }
    // As in the module path, a frame that just collected a failed build lets
    // the reload below deliver the edit instead of racing it.
    if !may_start || host.patch_attempt.is_some() {
        return;
    }
    let pending_generation = host.source_edit_generation.load(Ordering::Acquire);
    if pending_generation == host.last_processed_source_edit {
        return;
    }

    // Disjoint field borrows, as the module path does.
    let DevHost {
        hot_patch,
        patch_attempt,
        ..
    } = &mut *host;
    let Some(session) = hot_patch else {
        return;
    };
    match session.begin_attempt() {
        BeginOutcome::Unchanged => {}
        BeginOutcome::NotPatchable { refusal } => {
            report_patch_outcome(PatchOutcome::NotPatchable { refusal });
        }
        BeginOutcome::Failed {
            function,
            active_generation,
            failure,
        } => {
            report_patch_outcome(PatchOutcome::Failed {
                function,
                active_generation,
                failure,
            });
        }
        BeginOutcome::Ready(attempt) => {
            // The edit counts as handled only once the replacement is
            // installed; that is what lets a save landing mid-build stay
            // pending.
            *patch_attempt = Some(start_patch_build(
                attempt,
                PatchSubject::Project,
                pending_generation,
            ));
        }
    }
}

/// Take a new baseline snapshot for every subject a reload has just rebuilt.
///
/// The fast path decides whether an edit is "body-only" by comparing the file
/// with a snapshot of the code that is running. That snapshot normally moves
/// only when a patch is installed. A reload replaces the running code without
/// touching the snapshot, so an edit the fast path refused would stay in the
/// diff.
///
/// Each later edit would then be compared against a baseline two edits old,
/// and a change outside a hot function body would be refused again. That reads
/// as "live patching stopped working".
///
/// Call this after the reload, so a save that arrives during the build is not
/// folded into the baseline. That save has already increased the generation
/// counter, and the next frame gives it its own reload.
fn resync_patch_baselines(
    host: &mut DevHost,
    project_rebuilt: bool,
    reloaded_module_indices: &[usize],
) {
    if project_rebuilt {
        if let Some(session) = host.hot_patch.as_mut() {
            session.refresh_snapshots();
        }
    }
    for index in reloaded_module_indices {
        if let Some(Some(session)) = host.module_hot_patch.get_mut(*index) {
            session.refresh_snapshots();
        }
    }
}

/// What `regenerate_module_csharp_mirror` hands back. It carries the exposed
/// component bindings, the resolved mirror methods and heap-field accessors
/// for the managed method table, and whether the mirror file on disk changed.
type RegeneratedMirror = (
    Vec<ModuleExposedComponent>,
    Vec<crate::csharp::ResolvedMirrorMethod>,
    Vec<crate::csharp::ResolvedFieldAccessor>,
    bool,
);

/// Rewrite one extension's `generated/<module>_Components.g.cs` mirror file
/// from the generation loaded right now.
///
/// Returns what it was built from: the exposed component bindings for the C#
/// backend, the resolved mirror methods, and the heap-field accessors the
/// managed method table uses. It also reports whether the file on disk
/// changed; the reload path uses that to queue a C# project rebuild only when
/// what C# can see has moved.
///
/// # Errors
///
/// Returns a [`CSharpError::CodegenFailed`] when the mirror file cannot be
/// written.
fn regenerate_module_csharp_mirror(
    workspace_root: &Path,
    slot: &ExtensionSlot,
    exposed_components: Vec<ModuleExposedComponent>,
) -> Result<RegeneratedMirror, CSharpError> {
    let methods = slot.mirror_methods();
    let accessors = slot.field_accessors();
    let changed = crate::csharp::generate_module_components_csharp(
        workspace_root,
        slot.name(),
        &exposed_components,
        &slot.value_type_descriptors(),
        &methods,
        &accessors,
    )
    .map_err(|message| CSharpError::CodegenFailed { message })?;
    Ok((exposed_components, methods, accessors, changed))
}

/// The components each loaded module's mirror file carries, by first-claim
/// order.
///
/// A component is mirrored once per process. A shared component is registered
/// by every module that links it, so several modules' registration logs name
/// it. Two generated files declaring the same C# `partial struct` then collide
/// on every member (`CS0102`, "already contains a definition for `Color0`").
///
/// The first module to expose a component in load order keeps it. That is
/// stable because the module list is the project's own settings order.
fn module_mirror_ownership(
    engine: &Engine,
    extensions: &[ExtensionSlot],
) -> Vec<Vec<ModuleExposedComponent>> {
    let mut claimed: std::collections::HashSet<String> = std::collections::HashSet::new();
    extensions
        .iter()
        .map(|slot| {
            crate::csharp::exposed_components_from_names(
                engine.world(),
                slot.exposed_component_names(),
            )
            .into_iter()
            .filter(|component| claimed.insert(component.csharp_name.clone()))
            .collect()
        })
        .collect()
}

/// Collect a finished C# assembly build and complete its reload at this frame
/// boundary.
///
/// The build ran on its own thread with the frame free. Everything that
/// touches the boundary happens here: telling the managed loader the compiled
/// assembly is complete, polling it for the swap, and the bookkeeping a
/// replaced image invalidates. A worker that stopped without a report falls
/// back to building on the frame thread.
fn advance_managed_build(host: &mut DevHost) {
    match host.loaded_project.collect_managed_build() {
        BuildCollection::Pending => {}
        BuildCollection::Finished(report) => finish_managed_build(host, report),
        BuildCollection::Died => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "the C# assembly build stopped unexpectedly; building on the frame thread instead"
            );
            let report = host.loaded_project.build_now(
                &host.workspace_root,
                &host.project_module_config,
                None,
            );
            if let Some(report) = report {
                finish_managed_build(host, report);
            }
        }
    }
}

/// Finish a managed reload whose assembly build reported.
///
/// Reports the compile, shortens the loader's debounce (its wait for the
/// assembly to stop changing), polls for the swap, and runs the bookkeeping a
/// replaced image invalidates. A failed compile keeps the running assembly,
/// abandons the armed timing, and still refreshes the editor revision.
fn finish_managed_build(host: &mut DevHost, report: BuildReport) {
    let mut replaced = false;
    match report.outcome {
        BuildOutcome::Compiled { milliseconds } => {
            info!(
                target: telemetry_target::HOT_RELOAD,
                module = host.project_module_config.name.as_str(),
                compile_ms = milliseconds,
                "C# compiled in-process"
            );
            host.loaded_project
                .record_assembly_rebuilt_at(report.finished_at);
            host.loaded_project.notify_assembly_replaced();
            info!(
                target: telemetry_target::HOT_RELOAD,
                "C# build complete; polling managed loader"
            );
            replaced = host
                .loaded_project
                .poll_managed_reload(host.runtime.engine_mut());
        }
        BuildOutcome::Failed { diagnostics } => {
            // Errors in the developer's own source; a full build would spend
            // seconds reaching the same diagnostics.
            warn!(
                target: telemetry_target::HOT_RELOAD,
                "{}",
                pill_core::telemetry::log_block(
                    "C# compilation failed; keeping the currently loaded C# project assembly",
                    diagnostics.lines()
                )
            );
            // The managed compiler hands its diagnostics over directly, so
            // the Problems panel needs no capture ring for this path.
            notices::push(HostNotice::ModuleFailed {
                module: host.project_module_config.name.clone(),
                output: diagnostics,
            });
            host.loaded_project.abandon_managed_reload_timing();
        }
        BuildOutcome::Full(Ok(())) => {
            host.loaded_project
                .record_assembly_rebuilt_at(report.finished_at);
            info!(
                target: telemetry_target::HOT_RELOAD,
                "C# build complete; polling managed loader"
            );
            replaced = host
                .loaded_project
                .poll_managed_reload(host.runtime.engine_mut());
        }
        BuildOutcome::Full(Err(message)) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                error = %message,
                "C# build failed; keeping the currently loaded C# project assembly"
            );
            notices::push(HostNotice::ModuleFailed {
                module: host.project_module_config.name.clone(),
                output: message,
            });
            host.loaded_project.abandon_managed_reload_timing();
        }
    }
    if replaced {
        note_managed_swap(host);
    } else {
        // Nothing was swapped, but the project still runs the sources on
        // disk, so the patch classifier's baseline has to say so and the
        // editor still refreshes its metadata.
        resync_patch_baselines(host, true, &[]);
        host.bump_editor_revision();
    }
}

/// Everything a replaced managed assembly invalidates, whichever site saw the
/// swap. The recorded prologue addresses point into the retired image, the
/// patch classifier's baseline is stale, and the editor needs its toast and a
/// new revision to drop its caches.
///
/// Both sites call this: the build completion, which shortens the loader's
/// debounce and polls, and the frame-boundary poll that catches a swap whose
/// wait outlived Step 5.
/// Reloads the project's own editor crate when its sources moved, or when the
/// project it belongs to reloaded.
///
/// The crate is not an extension and does not live in `extensions`, so the
/// module pass cannot reach it; the transaction it runs is the same one, and
/// the peers it passes are every other subject, because a component type it
/// names is not this crate's to retire.
fn reload_project_editor_if_changed(host: &mut DevHost) {
    let DevHost {
        project_editor,
        runtime,
        engine_api,
        workspace_root,
        extensions,
        loaded_project,
        ..
    } = host;
    let Some(slot) = project_editor else {
        return;
    };
    if !slot.has_pending_reload() {
        return;
    }
    let peers = peer_component_names(extensions, None, loaded_project);
    let outcome = slot.reload_if_changed(runtime.engine_mut(), engine_api, workspace_root, &peers);
    match outcome {
        ReloadOutcome::Reloaded { generation } => {
            info!(
                target: telemetry_target::HOT_RELOAD,
                module = slot.name(),
                generation,
                "project editor crate reload processed"
            );
            notices::push(HostNotice::Reloaded {
                module: slot.name().to_string(),
                build_ms: analytics::module_build_ms(slot.name()).unwrap_or(0),
            });
        }
        ReloadOutcome::Failed { generation } => {
            // The previous generation is still current, so nothing more is
            // reported here: the shared sequence already pushed the refusal.
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = slot.name(),
                generation,
                "project editor crate reload failed; keeping the previous generation"
            );
        }
        ReloadOutcome::Unchanged => {}
    }
}

fn note_managed_swap(host: &mut DevHost) {
    forget_prologue_records(host);
    // The project's editor crate follows the project's reload, as it does on
    // the native path: queued here, run by the module pass.
    if let Some(slot) = &mut host.project_editor {
        info!(
            target: telemetry_target::HOT_RELOAD,
            module = slot.name(),
            "the project reloaded; queuing its editor crate's reload"
        );
        slot.queue_cascade_reload();
    }
    // The swap ends the reload, so its timing is finished here, at the one
    // place both swap sites reach. A swap with no armed timing - an assembly
    // this host did not build - records zeros, which is what the report says
    // for a phase that did not run.
    let summary = host.loaded_project.finish_managed_reload_timing();
    let (stage_ms, build_ms, load_ms) = match &summary {
        Some(summary) => (summary.stage_ms(), summary.build_ms, summary.swap_ms),
        None => (0.0, 0.0, 0.0),
    };
    analytics::record_managed_reload(
        &host.project_module_config.name,
        stage_ms,
        build_ms,
        load_ms,
    );
    // The editor's side of the swap, mirroring the extension reload's: a toast
    // carrying the measured build and a cleared problem list for the project.
    notices::push(HostNotice::Reloaded {
        module: host.project_module_config.name.clone(),
        build_ms: if build_ms.is_finite() {
            build_ms.round() as u64
        } else {
            0
        },
    });
    resync_patch_baselines(host, true, &[]);
    host.bump_editor_revision();
}

/// Every component type name the live subjects other than
/// `excluded_extension_index` declare.
///
/// Gathered from each slot's current generation rather than from the world's
/// registration log, which is append-only. A name a module has stopped
/// registering is still in that log, so it cannot answer whether any subject
/// declares the name now. The project counts as a subject too - a shared
/// identity is registered by every artifact linking the crate, and dropping it
/// on a module reload would delete the project's rows.
fn peer_component_names(
    extensions: &[ExtensionSlot],
    excluded_extension_index: Option<usize>,
    project: &LoadedProject,
) -> Vec<String> {
    let mut names: Vec<String> = extensions
        .iter()
        .enumerate()
        .filter(|(index, _)| Some(*index) != excluded_extension_index)
        .flat_map(|(_, slot)| slot.exposed_component_names().iter().cloned())
        .collect();
    names.extend(project.component_names().iter().cloned());
    names.sort();
    names.dedup();
    names
}

/// Run every reload step of one frame. That covers the extensions' own
/// reloads, the per-function fast paths and a pending project reload. It also
/// covers suspending a subject left on a replaced component layout, and the
/// analytics line that reports all of it.
///
/// Returns the names of the extensions that reloaded, in load order, for a
/// windowed host's renderer step. A step that owes a background build leaves
/// the rest to a later frame instead of blocking this one.
fn run_reload_steps(host: &mut DevHost) -> Vec<String> {
    // Step 1: Reload any extension whose sources changed. Each module has its
    // own generation counter and clears only its own systems, so editing one
    // module never rebuilds another and never touches the project's systems,
    // entities or resources.
    //
    // The transaction is timed from here so the analytics total covers the
    // whole cascade (the edited module plus the project reload it queued), not
    // just its last step. A collected patch is the exception: its build ran
    // between frames, so the start mark moves back to when that attempt began.
    let mut reload_started = Instant::now();

    // This is the thread that owns the frame boundary, and therefore the only
    // one allowed to rewrite live code.
    arm_patching_thread();

    // Collect a background patch build that has finished. While one is still
    // running, every step below defers. A reload rebuilds the rlibs the build
    // links against, and a second attempt would race this one for the same
    // pending edit.
    //
    // Rendering and systems continue, and the build is collected on a later
    // frame. A build that came back without installing its patch also blocks
    // new attempts for this frame, so the reload below gets to deliver the
    // edit.
    let (failed_attempt, collected_patch_started) = advance_patch_attempt(host);
    if let Some(started) = collected_patch_started {
        // The collected build ran while earlier frames kept rendering. Its
        // transaction began when the attempt did, so the total spans the build
        // instead of only the milliseconds this frame spent installing it.
        if started < reload_started {
            reload_started = started;
        }
    }
    if patch_attempt_in_flight(host) {
        return Vec::new();
    }

    // Collect a background C# assembly build that has finished, and defer
    // every step below while one is running. The compile reads the generated
    // mirror files a module reload rewrites, so nothing else may reload under
    // it. Rendering and systems continue, and the build is collected on a
    // later frame.
    advance_managed_build(host);
    if host.loaded_project.managed_build_in_flight() {
        return Vec::new();
    }

    // Step 2: Honour a rollback request, at the same frame boundary the
    // patch installs use - the prologue route rewrites live code, so this is a
    // requirement rather than a convenience. A function whose patched body has
    // been panicking rolls itself back at the same boundary.
    process_rollback_request(host);
    process_patch_faults(host);

    // Step 3: Try the per-function fast path for the extensions, before
    // the reload below turns a pending change into a full module rebuild.
    try_module_fast_path(host, !failed_attempt);
    if patch_attempt_in_flight(host) {
        // A build was started; the pending edit waits for its outcome.
        return Vec::new();
    }

    // Destructure so the module list, the engine and the API table are borrowed
    // as disjoint fields rather than through the whole host.
    let DevHost {
        extensions,
        runtime,
        engine_api,
        workspace_root,
        queued_reload_generation,
        project_module_config,
        loaded_project,
        ..
    } = &mut *host;
    let engine = runtime.engine_mut();
    let mut any_module_reloaded = false;
    // The loop records which modules reloaded, not just whether any did. Each
    // carries its own patch session, and only the sessions whose sources were
    // rebuilt need a new baseline.
    let mut reloaded_modules: Vec<usize> = Vec::new();
    for index in 0..extensions.len() {
        if !extensions[index].has_pending_reload() {
            continue;
        }
        // The other live subjects' declarations, gathered before the mutable
        // borrow of the slot being reloaded. A type that only one of them names
        // is not this module's to retire.
        let peers = peer_component_names(extensions, Some(index), loaded_project);
        let outcome =
            extensions[index].reload_if_changed(engine, engine_api, workspace_root, &peers);
        // A runtime crate that reloads leaves its folder's editor crate running
        // an old embedded copy of it, so queue that crate's rebuild. Editor
        // crates load after every runtime crate, so this pass reaches it later.
        if matches!(outcome, ReloadOutcome::Reloaded { .. }) && !extensions[index].is_editor_crate()
        {
            let folder = extensions[index].folder().to_string();
            for editor_slot in extensions[index + 1..]
                .iter_mut()
                .filter(|other| other.is_editor_crate() && other.folder() == folder)
            {
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    module = editor_slot.name(),
                    "a runtime crate of the folder reloaded; queuing the editor crate's reload"
                );
                editor_slot.queue_cascade_reload();
            }
        }
        let slot = &mut extensions[index];
        match outcome {
            ReloadOutcome::Reloaded { generation } => {
                reloaded_modules.push(index);
                // The reloaded image is unpatched and every recorded prologue
                // address points into the previous one. Noted here and acted on
                // once the borrow below ends; forgetting is idempotent, so doing it
                // once after the loop is the same as doing it per reload.
                any_module_reloaded = true;
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    module = slot.name(),
                    generation,
                    "extension reload processed"
                );
                // The editor sees a toast, and the module's problem list is
                // cleared. The wall time only exists when this reload ran a
                // build (a shader-only change is a load).
                notices::push(HostNotice::Reloaded {
                    module: slot.name().to_string(),
                    build_ms: analytics::module_build_ms(slot.name()).unwrap_or(0),
                });
                // A module the project links directly is compiled into the
                // project DLL as well as its own. After the module swaps, the
                // project still runs the old embedded copy of that crate. Queue
                // a project reload so the new code reaches the project too. The
                // existing transaction below handles build, rollback and schema
                // migration.
                //
                // The check is cheap: one small manifest read.
                if project_depends_on_crate(workspace_root, project_module_config, slot.name()) {
                    info!(
                        target: telemetry_target::HOT_RELOAD,
                        module = slot.name(),
                        "module is a direct dependency of the project; queuing a project reload"
                    );
                    // Owed on the pipeline's own counter, never the watcher's:
                    // no fast path marks this one handled, so a patch of the
                    // project's own edit cannot make this rebuild be skipped.
                    *queued_reload_generation += 1;
                }
            }
            ReloadOutcome::Failed { generation } => {
                // The old generation is still current. Its patches are still
                // installed, its baselines are still accurate and its prologue
                // records are still the rollback path, so none of the success
                // bookkeeping below may run for it.
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    module = slot.name(),
                    generation,
                    "extension reload failed; keeping the previous generation and its patch state"
                );
                // The refusal itself - its wording and the compiler output -
                // was already pushed by the shared reload sequence; nothing
                // more to report here.
            }
            ReloadOutcome::Unchanged => {}
        }
    }

    // Which extensions reloaded, by name, for the caller: a windowed host
    // rebuilds its renderer when the renderer's data crate is among them.
    let reloaded_extensions: Vec<String> = reloaded_modules
        .iter()
        .map(|&index| extensions[index].name().to_owned())
        .collect();

    // Step 3b: A reloaded module may have changed what C# can see: component
    // fields, value types or mirrored methods. Even a body-only edit moves
    // every trampoline address C# calls, because the module is recompiled onto
    // a fresh base.
    //
    // For a managed project, write each affected module's mirror file again
    // and queue a C# project reload. `project_cs.dll` is then rebuilt and the
    // collectible loader swaps it in, so a mirrored method added or edited at
    // runtime becomes callable without a host restart. A module with nothing
    // C# can see skips the rebuild.
    if matches!(
        &project_module_config.backend,
        ProjectModuleBackend::CSharp(_)
    ) && any_module_reloaded
    {
        let mut needs_csharp_reload = false;
        // Recomputed over the whole list so a reloaded module keeps exactly the
        // file it owned before the reload.
        let mirror_ownership = module_mirror_ownership(engine, extensions);
        for index in &reloaded_modules {
            match regenerate_module_csharp_mirror(
                workspace_root,
                &extensions[*index],
                mirror_ownership[*index].clone(),
            ) {
                Ok((_exposed, methods, accessors, mirror_changed)) => {
                    // A mirror-content change (fields/value types/method set)
                    // always rebuilds. A module exposing mirrored methods or
                    // heap-field accessors also rebuilds on any reload, because
                    // its trampolines live at new addresses and body edits
                    // should reach C#.
                    needs_csharp_reload |=
                        mirror_changed || !methods.is_empty() || !accessors.is_empty();
                }
                Err(error) => {
                    error!(
                        target: telemetry_target::HOT_RELOAD,
                        module = extensions[*index].name(),
                        error = %error,
                        "failed to regenerate the module's C# mirror after reload"
                    );
                }
            }
        }
        // Republish the mirror-method table from every module's current
        // generation: reloaded modules expose fresh addresses, untouched ones
        // keep the addresses they were loaded with. Container accessors are
        // appended as rows so the managed side resolves them through the same
        // lookup.
        let mut rows: Vec<crate::csharp::ResolvedMirrorMethod> = extensions
            .iter()
            .flat_map(ExtensionSlot::mirror_methods)
            .collect();
        for slot in extensions.iter() {
            rows.extend(crate::csharp::accessor_rows(&slot.field_accessors()));
        }
        crate::csharp::publish_mirror_methods(&rows);
        if needs_csharp_reload {
            info!(
                target: telemetry_target::HOT_RELOAD,
                "module reload changed the C# mirror surface; queuing a C# project reload"
            );
            *queued_reload_generation += 1;
        }
    }

    // The borrow above has ended, so the records a module reload invalidated
    // can be dropped now, and the rebuilt sources become the new patch baseline.
    if any_module_reloaded {
        forget_prologue_records(host);
        resync_patch_baselines(host, false, &reloaded_modules);
        // The set of loaded code changed; editor metadata caches must drop.
        host.bump_editor_revision();
    }

    // The project's own editor crate reloads like an extension's, on the same
    // signal: it is one more loadable artifact whose running image must not
    // outlive its sources.
    reload_project_editor_if_changed(host);

    // Step 4: Try the per-function fast path for the project, before the
    // reload below turns a pending change into a full rebuild.
    try_project_fast_path(host, !failed_attempt);
    if patch_attempt_in_flight(host) {
        // A build was started; the pending edit waits for its outcome.
        return Vec::new();
    }

    // Step 5: Process a pending project reload before running systems.
    //
    // Two counters feed this. The watcher's source-edit counter says an edit
    // arrived that nothing has delivered yet. The pipeline's queued-reload
    // counter says a rebuilt module crate still has to be compiled into the
    // project image again. A producer states its reason by increasing one of
    // them; recording the values seen after the reload marks exactly those
    // reasons handled, so a save that arrives mid-build stays pending.
    let source_edits = host.source_edit_generation.load(Ordering::Acquire);
    let queued_reloads = host.queued_reload_generation;
    if source_edits != host.last_processed_source_edit
        || queued_reloads != host.last_processed_queued_reload
    {
        info!(
            target: telemetry_target::HOT_RELOAD,
            generation = source_edits,
            queued = queued_reloads,
            "hot reload triggered"
        );

        // Count the reload as caused by the watcher signal that asked for it,
        // so the managed backend can time the whole save-to-swap span. The
        // entry is taken here because it belongs to exactly this reload; a
        // cascade the pipeline queued itself has no source trigger and times
        // from the frame instead.
        host.loaded_project
            .arm_managed_reload_timing(host.source_trigger_log.take(source_edits));

        // A managed project builds its assembly on a worker thread, so the
        // frame keeps rendering while Roslyn runs. The build is collected at a
        // later boundary, which is where its poll and bookkeeping happen. A
        // native project, or a worker thread that could not start, reloads
        // right here.
        let start = host.loaded_project.start_managed_reload(
            &host.workspace_root,
            &host.project_module_config,
            Some((Arc::clone(&host.source_edit_generation), source_edits)),
        );
        match start {
            // A worker thread is building the assembly; its report is
            // collected by `advance_managed_build` at a later boundary.
            ManagedReloadStart::Building => {}
            // A native project has no assembly to build: its reload owns the
            // DLL transaction on this thread.
            ManagedReloadStart::NotManaged => {
                // A project image about to be replaced unmaps two generations
                // later, so every recorded prologue address inside it goes stale
                // the moment the swap commits. It is the same clear a module
                // reload performs, gated the same way.
                let peers = peer_component_names(&host.extensions, None, &host.loaded_project);
                let replaced = host.loaded_project.reload(
                    host.runtime.engine_mut(),
                    &host.engine_api,
                    &host.workspace_root,
                    &host.project_module_config,
                    // A save during the build advances the generation beyond this
                    // baseline, which cancels the in-flight compilation; the next
                    // frame observes the newer generation and rebuilds.
                    Some((&host.source_edit_generation, source_edits)),
                    &peers,
                );
                // A failed or refused reload keeps the current image. Its patches
                // are still installed and its recorded prologues are still the
                // rollback route, so the records are dropped only on a real swap.
                if replaced {
                    forget_prologue_records(host);
                    // The editor part exists to change the editor the project
                    // runs in, so it follows the project's reload. Queued, not
                    // run here: the module pass owns building and swapping it.
                    if let Some(slot) = &mut host.project_editor {
                        info!(
                            target: telemetry_target::HOT_RELOAD,
                            module = slot.name(),
                            "the project reloaded; queuing its editor crate's reload"
                        );
                        slot.queue_cascade_reload();
                    }
                    // The editor sees a toast and a cleared problem list, as
                    // on an extension reload. Without it, a fixed compile
                    // error would stay in the Problems panel until the next
                    // module reload.
                    notices::push(HostNotice::Reloaded {
                        module: host.project_module_config.name.clone(),
                        build_ms: analytics::module_build_ms(&host.project_module_config.name)
                            .unwrap_or(0),
                    });
                }
                // The project now runs the sources on disk, so the patch classifier's
                // baseline has to say so too. Skipping this is what makes one refused
                // patch disable the fast path for the rest of the session.
                resync_patch_baselines(host, true, &[]);
                // A project reload replaced the running image; the editor must refresh.
                host.bump_editor_revision();
            }
            // The build worker could not start, so the build runs on this
            // thread. Its report goes through the same handler the worker's
            // reports go through, so there is one outcome path with one set of
            // log lines.
            ManagedReloadStart::Unavailable => {
                let report = host.loaded_project.build_now(
                    &host.workspace_root,
                    &host.project_module_config,
                    // A save during the build advances the generation beyond this
                    // baseline, which cancels the in-flight compilation; the next
                    // frame observes the newer generation and rebuilds.
                    Some((Arc::clone(&host.source_edit_generation), source_edits)),
                );
                if let Some(report) = report {
                    finish_managed_build(host, report);
                }
            }
        }
        // Record the baseline the reload ran against, not a fresh read. A save
        // that arrived during the build has already increased the counter and
        // cancelled that compilation. Recording the newer value would mark the
        // save as handled although the build it cancelled produced nothing,
        // leaving the edit undelivered on disk. Recording the baseline lets
        // the next frame see the save and rebuild, which is why it was
        // cancelled.
        host.last_processed_source_edit = source_edits;
        // The rebuild the cascade asked for has happened; a later module swap
        // adds one to the counter again.
        host.last_processed_queued_reload = queued_reloads;
    }

    // Step 6: A reload above may have changed the layout of a shared component
    // that other subjects registered too. Those subjects still run code built
    // against the old layout, so their systems stay suspended until they are
    // rebuilt. The project is normally rebuilt in Step 5 of this same
    // boundary.
    suspend_stale_subjects(host);

    // Print the analytics line for every reload completed this frame: the
    // extensions from Step 1, the project from Step 5. One aggregate total
    // follows. Each event was recorded with its build, stage, load, init and
    // migrate timings already filled in, so this only takes the recorded
    // events and prints them.
    analytics::print_reload_events(reload_started);
    reloaded_extensions
}

/// Suspend the systems of every subject built against a shared component
/// layout a reload has since replaced, and resume the ones rebuilt since.
///
/// A stale subject's queries are already refused by the declared schema check.
/// Its systems can still reach the type in other ways, such as spawning a
/// value, which would write rows in the old layout. Suspending the owner stops
/// all of it until the subject is rebuilt against the layout now registered.
fn suspend_stale_subjects(host: &mut DevHost) {
    // Step 1: Every native subject that registers data, with what is stale in
    // it. The renderer module is not among them. It holds no slot here, and a
    // renderer built against a replaced layout is covered by its own pause.
    let world = host.runtime.engine().world();
    let mut subjects: Vec<(String, pill_engine::SystemOwner, Vec<String>)> = host
        .extensions
        .iter()
        .map(|slot| {
            (
                slot.name().to_owned(),
                slot.owner(),
                slot.stale_components(world),
            )
        })
        .collect();
    subjects.push((
        host.project_module_config.name.clone(),
        pill_engine::SystemOwner::PROJECT,
        host.loaded_project.stale_components(world),
    ));

    // Step 2: Act only on transitions, so each is logged once.
    for (subject, owner, stale) in subjects {
        let suspended = host.stale_suspended_owners.contains(&owner);
        if !stale.is_empty() && !suspended {
            host.runtime
                .engine_mut()
                .set_systems_enabled_for_owner(owner, false);
            host.stale_suspended_owners.push(owner);
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = subject.as_str(),
                components = %stale.join(", "),
                "systems suspended: this binary was built against a previous layout of these shared components; they resume once it is rebuilt"
            );
        } else if stale.is_empty() && suspended {
            host.runtime
                .engine_mut()
                .set_systems_enabled_for_owner(owner, true);
            host.stale_suspended_owners.retain(|other| *other != owner);
            info!(
                target: telemetry_target::HOT_RELOAD,
                module = subject.as_str(),
                "systems resumed: rebuilt against the current shared component layouts"
            );
        }
    }
}

/// Process hot reloads, execute one scheduler frame, and log the frame
/// statistics when the reporting window elapses.
///
/// The two phases run back to back. A windowed host runs them itself, with its
/// renderer step between them (see [`RenderingHost::run_frame`]).
pub fn run_frame(host: &mut DevHost) {
    let frame_start = Instant::now();
    run_reload_phase(host);
    run_frame_phase(host, frame_start)
}

/// Everything reloading does at a frame boundary, before any system runs:
/// extension and project reloads, patches, and the managed loader's swap.
///
/// Returns the names of the extensions that reloaded. Separate from
/// [`run_frame_phase`] so a windowed host can reload its renderer between the
/// two - after the data it draws reloaded, before any system reads it.
pub(crate) fn run_reload_phase(host: &mut DevHost) -> Vec<String> {
    // Steps 1 to 6: everything reloading does, in one call so this loop is
    // identical whether or not the machinery is compiled in.
    let reloaded_extensions = run_reload_steps(host);

    // Step 7: Poll the managed loader for an assembly swap.
    //
    // The managed loader watches the built assembly rather than source files,
    // and only a reloading build has one to poll. This poll is the fallback
    // half of the two-mechanism design described on
    // `project_module::poll_managed_reload`. An in-process compile tells the
    // loader to stop waiting through `NotifyAssemblyReplaced`. A build through
    // `dotnet build` leaves no completion signal, so the loader is polled here.
    if host
        .loaded_project
        .poll_managed_reload(host.runtime.engine_mut())
    {
        // The loader's debounce outlived Step 5, so the swap reaches this frame.
        // It is the same swap the build completion reports, so it runs the same
        // bookkeeping - the editor's toast and its new revision included.
        note_managed_swap(host);
    }

    // Step 8: Apply asset edits, after every reload of this boundary, so they
    // go through the registrations current now.
    let asset_changes = apply_asset_changes(host);
    // Step 9: The same for manager settings: a settings file is an asset, so
    // the watcher above is what notices an edit, and a reload rebuilds the
    // manager that has to take it again.
    apply_manager_settings_changes(host, asset_changes);
    reloaded_extensions
}

/// Start watching the project's asset directory, or `None` when there is none
/// or the watch cannot start (logged; the host runs on without it).
///
/// A configured directory that does not exist yet is created before the watch
/// starts. The editor imports dropped files into it, and a project that never
/// had a `res` would otherwise stay unwatched for the whole run, with files
/// sitting there unimported.
fn start_asset_watcher(
    asset_directory: Option<&Path>,
    assets_hot_reloading: bool,
) -> Option<crate::watchers::AssetWatcher> {
    if !assets_hot_reloading {
        info!(
            target: telemetry_target::HOT_RELOAD,
            variable = crate::switches::ASSETS_HOT_RELOADING_VARIABLE,
            "[assets] asset hot reloading is switched off; asset edits are not noticed until the host restarts"
        );
        return None;
    }
    let asset_directory = asset_directory?;
    if !asset_directory.is_dir() {
        if let Err(error) = std::fs::create_dir_all(asset_directory) {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                directory = %asset_directory.display(),
                error = %error,
                "[assets] could not create the project's asset directory; asset edits will not be reimported"
            );
            return None;
        }
    }
    match crate::watchers::AssetWatcher::spawn(asset_directory.to_owned()) {
        Ok(watcher) => watcher,
        Err(error) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                directory = %asset_directory.display(),
                error = %error,
                "[assets] could not watch the project's assets; asset edits will not be reimported"
            );
            None
        }
    }
}

/// A guid as its 32 hexadecimal digits, or `none` when the asset carries none
/// (a path registered under a name alone, with no metadata file).
fn guid_text(guid: Option<AssetGuid>) -> String {
    guid.map_or_else(|| "none".to_owned(), |guid| guid.to_string())
}

/// Applies every manager's settings asset once the project has loaded, writing
/// a missing file first.
///
/// Runs after the project's `init`, which is the first moment every manager
/// exists. A project whose `res/settings` holds no file for a manager gets one
/// written from the manager's own document defaults, so the file a person edits
/// already spells out every field.
///
/// A host with no project directory - a shipped bundle - creates nothing and
/// applies nothing: the manager keeps the values its document defaults give it.
fn apply_manager_settings_at_startup(world: &mut World, asset_directory: Option<&Path>) {
    let Some(directory) = asset_directory else {
        return;
    };
    // Step 1: Write a file for every manager that has none.
    for missing in world.missing_manager_settings() {
        let path = directory.join(&missing.path);
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    manager = missing.settings_name.as_str(),
                    path = %path.display(),
                    error = %error,
                    "[settings] could not create the settings directory; the manager keeps its defaults"
                );
                continue;
            }
        }
        match std::fs::write(&path, &missing.bytes) {
            Ok(()) => info!(
                target: telemetry_target::HOT_RELOAD,
                manager = missing.settings_name.as_str(),
                path = %path.display(),
                "[settings] created the manager settings asset"
            ),
            Err(error) => warn!(
                target: telemetry_target::HOT_RELOAD,
                manager = missing.settings_name.as_str(),
                path = %path.display(),
                error = %error,
                "[settings] could not create a manager's settings asset; the manager keeps its defaults"
            ),
        }
    }
    // Step 2: Apply each one that now has a file.
    for manager in world.settings_asset_manager_ids() {
        let Some(surface) = world.manager_settings_surface(manager) else {
            continue;
        };
        let Some(path) = world.manager_settings_path(manager) else {
            continue;
        };
        if !directory.join(&path).is_file() {
            continue;
        }
        match world.apply_manager_settings_asset(manager) {
            Some(Ok(outcome)) => {
                log_manager_settings_applied(
                    &surface.settings_name,
                    Path::new(&path),
                    outcome,
                    false,
                );
            }
            Some(Err(reason)) => {
                log_manager_settings_failure(&surface.settings_name, &path, &reason)
            }
            None => {}
        }
    }
}

/// Re-applies the manager settings assets whose bytes changed, and every one of
/// them after a reload.
///
/// A reload replaces managers, so the documents the project already had are
/// applied again - a manager that was rebuilt has to take them once more. The
/// editor revision is what says a reload happened; nothing here needs to know
/// which artifact it replaced.
///
/// A settings file is an asset, so the asset watcher is what notices an edit:
/// the change is read back and applied against the bytes the manager already
/// took, which is why an edit that changes nothing applies nothing.
fn apply_manager_settings_changes(host: &mut DevHost, asset_changes: usize) {
    let reloaded = host.manager_settings_editor_revision != host.editor_revision;
    if !reloaded && asset_changes == 0 {
        return;
    }
    if reloaded {
        host.manager_settings_editor_revision = host.editor_revision;
    }
    let world = host.runtime.engine_mut().world_mut();
    if reloaded {
        for manager in world.settings_asset_manager_ids() {
            let Some(surface) = world.manager_settings_surface(manager) else {
                continue;
            };
            let Some(path) = world.manager_settings_path(manager) else {
                continue;
            };
            match world.apply_manager_settings_asset(manager) {
                Some(Ok(outcome)) => {
                    log_manager_settings_applied(
                        &surface.settings_name,
                        Path::new(&path),
                        outcome,
                        false,
                    );
                }
                Some(Err(reason)) => {
                    log_manager_settings_failure(&surface.settings_name, &path, &reason);
                }
                None => {}
            }
        }
        return;
    }
    let report = world.refresh_manager_settings();
    for applied in &report.applied {
        log_manager_settings_applied(
            &applied.settings_name,
            Path::new(&applied.path),
            applied.outcome,
            true,
        );
    }
    for failure in &report.failures {
        log_manager_settings_failure(&failure.settings_name, &failure.path, &failure.reason);
    }
}

/// Logs what one manager did with its settings document.
///
/// `changed` says the file differs from the one already applied, which is what
/// says an edit reached the running manager rather than a start-up pass
/// re-reading what it already had.
fn log_manager_settings_applied(
    settings_name: &str,
    path: &Path,
    outcome: SettingsOutcome,
    changed: bool,
) {
    match outcome {
        SettingsOutcome::Applied => info!(
            target: telemetry_target::HOT_RELOAD,
            manager = settings_name,
            path = %path.display(),
            changed,
            "[settings] applied manager settings"
        ),
        SettingsOutcome::Refused => warn!(
            target: telemetry_target::HOT_RELOAD,
            manager = settings_name,
            path = %path.display(),
            "[settings] refused manager settings; the manager keeps the values it has"
        ),
    }
}

/// Logs a settings asset that could not be read, is not the manager's type, or
/// does not fit its document.
fn log_manager_settings_failure(settings_name: &str, path: &str, reason: &str) {
    warn!(
        target: telemetry_target::HOT_RELOAD,
        manager = settings_name,
        path,
        reason,
        "[settings] refused manager settings; the manager keeps the values it has"
    );
}

/// Import or reimport every asset the watcher reported, through the world's
/// import registry.
///
/// A source moved together with its metadata file is followed. The loaded
/// asset takes the new name and keeps its handle and guid, and its old path's
/// deletion is not reported. A new source is imported, writing its metadata
/// file. A loaded one is decoded again into its slot, keeping its handle and
/// guid.
///
/// Nothing here ever removes an asset. A failed reimport leaves the loaded
/// value in place, and a deleted source stays loaded until the next run,
/// because components may still hold its handle.
fn apply_asset_changes(host: &mut DevHost) -> usize {
    let Some(watcher) = &host.asset_watcher else {
        return 0;
    };
    let changes = watcher.drain();
    let change_count = changes.len();
    if changes.is_empty() {
        return 0;
    }
    let ensure_metadata = host.ensure_asset_metadata;
    // Every path the batch touched, so the asset database follows it once the
    // batch is applied (a `.meta` change is reported under its source).
    let touched: Vec<String> = changes
        .iter()
        .map(|change| match change {
            AssetChange::Edited { source, .. }
            | AssetChange::SourceRemoved { source }
            | AssetChange::MetadataRemoved { source } => source.clone(),
        })
        .collect();
    let asset_root = host.asset_directory.clone();
    let world = host.runtime.engine_mut().world_mut();
    // A clone, so the registry can be called against the asset manager, which
    // is a second resource of the same world.
    let Some(registry) = world.get_resource::<pill_engine::ImportRegistry>().cloned() else {
        return change_count;
    };
    let Some(assets) = world.get_resource_mut::<pill_engine::AssetManager>() else {
        return change_count;
    };
    // Only files a registered type imports are assets; the rest of `res`
    // (shaders, configuration, licences) is not this code's business.
    let is_asset = |source: &str| {
        Path::new(source)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| registry.type_for_extension(extension).is_some())
    };

    // Edits first: a move arrives as the new path's edit and the old path's
    // removal, and only the edit can tell it is a move.
    let (edits, removals): (Vec<AssetChange>, Vec<AssetChange>) = changes
        .into_iter()
        .partition(|change| matches!(change, AssetChange::Edited { .. }));
    let mut moved_from: Vec<String> = Vec::new();

    for change in edits.into_iter().chain(removals) {
        match change {
            AssetChange::Edited {
                source,
                through_metadata,
            } if is_asset(&source) => {
                let path = Path::new(&source);
                if let Ok(Some(old_name)) = registry.follow_move(assets, path) {
                    info!(
                        target: telemetry_target::HOT_RELOAD,
                        from = old_name.as_str(),
                        to = source.as_str(),
                        "[assets] followed a move; the asset keeps its handle and guid"
                    );
                    moved_from.push(old_name);
                    continue;
                }
                // A `.meta` written or edited for a source nothing has loaded
                // concerns no running asset; importing it here would load it
                // just because its sidecar changed.
                if through_metadata && !registry.is_loaded(assets, path) {
                    continue;
                }
                let imported =
                    registry.import(assets, path, pill_engine::MetadataPolicy::CreateIfMissing);
                match imported {
                    Ok(outcome) if !outcome.previously_loaded() => info!(
                        target: telemetry_target::HOT_RELOAD,
                        asset = source.as_str(),
                        guid = %guid_text(outcome.guid),
                        metadata = ?outcome.metadata,
                        "[assets] imported a new asset"
                    ),
                    Ok(_) => match registry.reimport(assets, path) {
                        Ok(outcome) => info!(
                            target: telemetry_target::HOT_RELOAD,
                            asset = source.as_str(),
                            guid = %guid_text(outcome.guid),
                            through_metadata,
                            content_version_before = ?outcome.previous_content_version,
                            content_version = ?outcome.content_version,
                            "[assets] reimported"
                        ),
                        Err(error) => warn!(
                            target: telemetry_target::HOT_RELOAD,
                            asset = source.as_str(),
                            "[assets] reimport failed; keeping the loaded value: {error}"
                        ),
                    },
                    Err(error) => warn!(
                        target: telemetry_target::HOT_RELOAD,
                        asset = source.as_str(),
                        "[assets] reimport failed; keeping the loaded value: {error}"
                    ),
                }
            }
            // Reported only while something is loaded under that name: after
            // a move the old name resolves to nothing, even when its removal
            // arrives in a later batch than the move.
            AssetChange::SourceRemoved { source }
                if !moved_from.contains(&source)
                    && registry.is_loaded(assets, Path::new(&source)) =>
            {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    asset = source.as_str(),
                    "[assets] source deleted; the asset stays loaded until the next run"
                )
            }
            // With metadata kept, a deleted `.meta` is written again: with the
            // loaded asset's guid when it is loaded, a new one otherwise.
            AssetChange::MetadataRemoved { source }
                if ensure_metadata && !moved_from.contains(&source) =>
            {
                match registry.ensure_metadata(assets, Path::new(&source)) {
                    Ok(Some(guid)) => info!(
                        target: telemetry_target::HOT_RELOAD,
                        asset = source.as_str(),
                        guid = %guid,
                        "[assets] wrote missing metadata"
                    ),
                    Ok(None) => {}
                    Err(error) => warn!(
                        target: telemetry_target::HOT_RELOAD,
                        asset = source.as_str(),
                        "[assets] could not write missing metadata: {error}"
                    ),
                }
            }
            AssetChange::MetadataRemoved { source }
                if !moved_from.contains(&source)
                    && registry.is_loaded(assets, Path::new(&source)) =>
            {
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    asset = source.as_str(),
                    "[assets] metadata file deleted; the loaded asset keeps its guid and settings until the next run"
                )
            }
            _ => {}
        }
    }

    // The database follows the batch after it is applied, so it sees the
    // `.meta` files the imports above just wrote. Paths go in the order the
    // watcher reported them; a move's new path comes before its old one.
    if let (Some(asset_root), Some(database)) = (
        asset_root,
        world.get_resource_mut::<pill_engine::AssetDatabase>(),
    ) {
        let conflicts_before = database.conflicts().len();
        for source in &touched {
            database.refresh(&asset_root, source, &registry);
        }
        for conflict in database.conflicts().iter().skip(conflicts_before) {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                guid = %conflict.guid,
                kept = conflict.kept.as_str(),
                refused = conflict.refused.as_str(),
                "[assets] two files claim one guid; the second is not indexed"
            );
        }
        info!(
            target: telemetry_target::HOT_RELOAD,
            paths = touched.len(),
            assets = database.len(),
            "[assets] database refreshed"
        );
    }
    change_count
}

/// One scheduler frame and its reporting, run by the wrapped runtime.
pub(crate) fn run_frame_phase(host: &mut DevHost, frame_start: Instant) {
    host.runtime.run_frame_from(frame_start);
}

/// Honour a pending request, if one was dropped.
///
/// Called at the frame boundary, where no system is executing. That is the
/// same guarantee a patch install relies on, and it is a requirement rather
/// than a convenience for the prologue route, which rewrites live code.
fn process_rollback_request(host: &mut DevHost) {
    let request_path = host.workspace_root.join(ROLLBACK_REQUEST_FILE);
    let Ok(request) = std::fs::read_to_string(&request_path) else {
        return;
    };
    let request = request.trim();

    // An empty read usually means the file is only half written. This runs
    // every frame, so it often reads the file between its creation and the
    // writer finishing. Leave it for the next frame instead of reporting an
    // error.
    if request.is_empty() {
        return;
    }

    // The file is removed before the request runs. A request that fails is
    // therefore not retried every frame, and a malformed one is reported
    // exactly once.
    let request = request.to_string();
    let _ = std::fs::remove_file(&request_path);

    if request.eq_ignore_ascii_case("list") {
        print_patch_generations(host);
        return;
    }

    let Some((function, requested_generation)) = request.rsplit_once('@') else {
        warn!(
            target: telemetry_target::HOT_RELOAD,
            "Patch rollback request `{request}` is malformed; expected \
             `function@generation`, `function@previous`, or `list`"
        );
        return;
    };
    let function = function.trim();
    let requested_generation = requested_generation.trim();

    // `previous` saves a developer from looking up numbers to undo the last
    // edit, which is nearly always what a rollback is for.
    let generation = if requested_generation.eq_ignore_ascii_case("previous") {
        match previous_patch_generation(host, function) {
            Some(previous) => previous,
            None => {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    "Patch rollback: `{function}` has no recorded generations"
                );
                print_patch_generations(host);
                return;
            }
        }
    } else {
        match requested_generation.parse::<u32>() {
            Ok(generation) => generation,
            Err(_) => {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    "Patch rollback: `{requested_generation}` is not a generation number or `previous`"
                );
                return;
            }
        }
    };

    if let Err(detail) = host.rollback_patch(function, generation) {
        warn!(
            target: telemetry_target::HOT_RELOAD,
            "Patch rollback of `{function}` to generation {generation} failed: {detail}"
        );
        // A rollback usually fails because the generation does not exist, so
        // show what does rather than making the developer guess.
        print_patch_generations(host);
    } else {
        // A rollback changed which implementation is live; refresh the editor.
        host.bump_editor_revision();
        notices::push(HostNotice::RolledBack {
            function: function.to_string(),
            to_generation: generation,
            automatic: false,
        });
    }
}

/// The generation a rollback naming `previous` resolves to for `function`.
///
/// `None` when the function has no recorded generations at all, which is what
/// a refusal reads as.
fn previous_patch_generation(host: &DevHost, function: &str) -> Option<u32> {
    host.patch_generations()
        .iter()
        .filter(|generation| generation.function == function)
        .map(|generation| generation.number)
        .max()
        .map(|newest| newest.saturating_sub(1))
}

/// Roll back a function whose patched body kept panicking.
///
/// The engine's dispatch slot counts how often a patched function panics in a
/// row and queues one report when the count crosses its threshold. The
/// rollback happens here, at the frame boundary, where no system is running -
/// the same requirement a requested rollback has, since the prologue route
/// rewrites live code.
///
/// Rolling back to the previous generation keeps the last working code. A
/// function that was patched only once rolls back to generation zero, which is
/// the code the running artifact was built with. Nothing retries the rejected
/// patch: the edit counted as handled when the patch was applied, so the
/// function stays on the working generation until the next source edit.
fn process_patch_faults(host: &mut DevHost) {
    for report in pill_engine::hot_patch::take_fault_reports() {
        let function = report.function;
        let Some(generation) = previous_patch_generation(host, &function) else {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                function = function.as_str(),
                "a patched function kept panicking, but this session records no generation to roll it back to; the panics stay contained"
            );
            continue;
        };
        match host.rollback_patch(&function, generation) {
            Ok(()) => {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    function = function.as_str(),
                    generation,
                    panic = report.message.as_str(),
                    "patch rolled back automatically after repeated panics; the previous generation keeps running"
                );
                host.bump_editor_revision();
                notices::push(HostNotice::RolledBack {
                    function,
                    to_generation: generation,
                    automatic: true,
                });
            }
            Err(detail) => warn!(
                target: telemetry_target::HOT_RELOAD,
                function = function.as_str(),
                generation,
                detail = detail.as_str(),
                "automatic rollback failed; the panicking generation keeps running"
            ),
        }
    }
}

/// Print every generation this session has installed.
///
/// Rollback is unusable without it. The request needs a number, and nothing
/// else in the host ever reports which numbers exist.
fn print_patch_generations(host: &DevHost) {
    let generations = host.patch_generations();
    if generations.is_empty() {
        info!(
            target: telemetry_target::HOT_RELOAD,
            "No patch generations yet; edit a function body to create one"
        );
        return;
    }
    let mut lines: Vec<String> = generations
        .iter()
        .map(|generation| {
            format!(
                "{:<48} generation {:<3} {}",
                generation.function,
                generation.number,
                format!("{:.0}s ago", generation.age_seconds).dimmed()
            )
        })
        .collect();
    lines.push(
        "generation 0 is the code each artifact was built with"
            .dimmed()
            .to_string(),
    );
    info!(
        target: telemetry_target::HOT_RELOAD,
        "{}",
        log_block_colored(
            format!("Patch generations ({} total)", generations.len()),
            lines
        )
    );
}

/// Every loaded artifact a plain-function patch must be offered to.
///
/// One entry per currently loaded library: the project and each extension. A
/// crate linked into several of them is compiled into each, so each holds an
/// independent redirect slot for the same function. All of them have to be
/// told about the replacement.
///
/// Retired generations are deliberately left out. They stay mapped only so
/// outstanding pointers into their code remain valid, and their systems have
/// already been cleared, so nothing calls their copy of the function.
fn patch_targets<'a>(
    loaded_project: &'a LoadedProject,
    extensions: &'a [ExtensionSlot],
) -> Vec<(&'a str, &'a crate::native_library::NativeLibrary)> {
    let mut targets = Vec::with_capacity(extensions.len() + 1);
    if let Some(library) = loaded_project.native_library() {
        targets.push(("project", library));
    }
    for slot in extensions {
        targets.push((slot.name(), slot.current_library()));
    }
    targets
}

/// Report one patch attempt, and say whether it fully handled the change.
///
/// `true` means the edit is live and the pending reload has nothing left to do.
/// Every other outcome falls through to the normal rebuild.
fn report_patch_outcome(outcome: crate::hot_patch::PatchOutcome) -> bool {
    match outcome {
        crate::hot_patch::PatchOutcome::Patched {
            function,
            generation,
            elapsed_milliseconds,
            stages,
            artifact_bytes,
            routes,
            copies,
        } => {
            // How the replacement was delivered, logged here rather than left to
            // the analytics line. A slot route is provable - the install is one
            // atomic pointer store and every call that reaches the dispatcher runs
            // the new code. The prologue route cannot make that promise: it
            // overwrites a live function's first bytes, so a caller that inlined
            // the body still runs the old one. A developer watching a change not
            // take effect needs to know which one they got.
            let delivery = routes
                .iter()
                .map(|route| route.label())
                .collect::<Vec<_>>()
                .join("+");
            let best_effort = !routes.iter().all(|route| route.is_provable());
            let note = format!(
                "(generation {generation} via {delivery}, {copies} {}{})",
                if copies == 1 { "copy" } else { "copies" },
                if best_effort { " - best effort" } else { "" }
            );
            // `[hot] <function> LIVE <ms> ms (generation <n> via <route>` is the
            // text `devops/tests/test_hot_patch_coverage.py` parses; keep it.
            let note = if best_effort {
                note.yellow()
            } else {
                note.dimmed()
            };
            info!(
                target: telemetry_target::HOT_RELOAD,
                "{}",
                log_block_colored(
                    format!(
                        "[hot] {function} {} {note}",
                        format!("LIVE {elapsed_milliseconds:.0} ms").green()
                    ),
                    [stages.to_string().dimmed()]
                )
            );
            // Recorded in the same shape a module reload is, so one parser in
            // the benchmark harness reads both.
            analytics::record_patch(
                &function,
                stages.classify + stages.generate + stages.flags,
                stages.compile as u64,
                stages.load,
                stages.activate,
                artifact_bytes,
                &routes,
                copies,
            );
            notices::push(HostNotice::PatchApplied {
                function,
                milliseconds: elapsed_milliseconds,
            });
            true
        }
        crate::hot_patch::PatchOutcome::NotPatchable { refusal } => {
            info!(
                target: telemetry_target::HOT_RELOAD,
                code = refusal.code,
                detail = refusal.detail.as_str(),
                "fast patch refused; falling back to a reload"
            );
            analytics::record_patch_refusal(refusal.code, refusal.detail.as_str());
            false
        }
        crate::hot_patch::PatchOutcome::Failed {
            function,
            active_generation,
            failure,
        } => {
            // The running implementation is intact, so the message says what is
            // still executing rather than only what did not happen.
            pill_core::warn!(
                target: telemetry_target::HOT_RELOAD,
                function = function.as_str(),
                code = failure.code,
                detail = failure.detail.as_str(),
                active_generation,
                "fast patch failed; the previous implementation is still running"
            );
            analytics::record_patch_failure(
                function.as_str(),
                failure.code,
                failure.detail.as_str(),
            );
            notices::push(HostNotice::PatchFailed {
                function,
                detail: failure.detail,
            });
            false
        }
        crate::hot_patch::PatchOutcome::Unchanged => false,
    }
}

/// Announce the project this start runs, before any build output starts
/// streaming.
///
/// The display name alone can repeat between projects, so the line also
/// carries the directory the configuration resolved it from. A configuration
/// built from a module alone has no directory, and the line then omits that
/// field rather than printing an empty one.
fn print_welcome(
    project_name: &str,
    project_directory: Option<&Path>,
    project_module_config: &ProjectModuleConfig,
) {
    let backend = project_module_config.backend.kind_name();
    match project_directory {
        Some(directory) => info!(
            target: telemetry_target::ENGINE,
            project = project_name,
            directory = %directory.display(),
            backend,
            "welcome to the Pill engine"
        ),
        None => info!(
            target: telemetry_target::ENGINE,
            project = project_name,
            backend,
            "welcome to the Pill engine"
        ),
    }
}

/// Print the selected backend before any build output starts streaming.
///
/// The backend is logged as its kind, the library it loads and the directory
/// that library is built into, so the line carries plain values.
fn print_startup_configuration(workspace_root: &Path, project_module_config: &ProjectModuleConfig) {
    let backend = project_module_config.backend.kind_name();
    let (library, output_directory) = match &project_module_config.backend {
        crate::config::ProjectModuleBackend::NativeLibrary {
            library_name,
            output_subdirectory,
        } => (library_name.as_str(), output_subdirectory.as_str()),
        crate::config::ProjectModuleBackend::CSharp(csharp_module_config) => (
            csharp_module_config.project_assembly_name.as_str(),
            csharp_module_config.project_output_subdirectory.as_str(),
        ),
    };
    info!(
        target: telemetry_target::ENGINE,
        workspace = %workspace_root.display(),
        module = project_module_config.name.as_str(),
        backend,
        library,
        output_directory,
        build_command = %project_module_config.build_command.join(" "),
        watch_directory = project_module_config.watch_directory.as_str(),
        "ECS host starting"
    );
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(all(test, feature = "rendering"))]
mod tests {
    use super::*;

    const PAUSED: Option<PauseReason> = Some(PauseReason::StaleRendererAfterDataLayoutChange);

    /// A world holding one entity with the engine's transform, and the name it
    /// is registered under.
    fn world_with_common_components() -> (pill_engine::World, String) {
        let mut world = pill_engine::World::new();
        pill_engine::register_transform_component(&mut world);
        world
            .create_entity()
            .with(pill_engine::TransformComponent::default())
            .build()
            .expect("registered components");
        let transform = pill_engine::common_components::TRANSFORM_SHARED_NAME.to_owned();
        (world, transform)
    }

    /// A shared component with entities that the renderer does not consume is
    /// reported once, with its entity count; a plain one never is.
    #[test]
    fn an_ignored_shared_component_is_reported_once() {
        let (world, transform) = world_with_common_components();
        let registered = vec![transform.clone(), transform.clone()];

        assert_eq!(
            unsupported_render_components(&world, &registered, &[]),
            vec![(transform, 1)]
        );
    }

    /// A consumed component, or one no entity carries, is not reported.
    #[test]
    fn consumed_or_unused_components_are_not_reported() {
        let (world, transform) = world_with_common_components();

        assert!(unsupported_render_components(
            &world,
            std::slice::from_ref(&transform),
            std::slice::from_ref(&transform)
        )
        .is_empty());
        let mut empty = pill_engine::World::new();
        pill_engine::register_transform_component(&mut empty);
        let transform = pill_engine::common_components::TRANSFORM_SHARED_NAME.to_owned();
        assert!(unsupported_render_components(&empty, &[transform], &[]).is_empty());
    }

    /// A data layout change pauses rendering unless a renderer rebuilt against
    /// it attached in the same boundary.
    #[test]
    fn a_layout_change_pauses_unless_the_rebuilt_renderer_attached() {
        assert_eq!(
            pause_after_boundary(None, true, RendererStep::Attached),
            None
        );
        assert_eq!(
            pause_after_boundary(None, true, RendererStep::Failed),
            PAUSED
        );
        assert_eq!(
            pause_after_boundary(None, true, RendererStep::Unchanged),
            PAUSED
        );
    }

    /// A body-only data change, or none, never pauses: the renderer's layout
    /// still matches, even when its rebuild failed.
    #[test]
    fn without_a_layout_change_nothing_pauses() {
        assert_eq!(
            pause_after_boundary(None, false, RendererStep::Failed),
            None
        );
        assert_eq!(
            pause_after_boundary(None, false, RendererStep::Unchanged),
            None
        );
        assert_eq!(
            pause_after_boundary(None, false, RendererStep::Attached),
            None
        );
    }

    /// Every managed assembly swap runs the same bookkeeping, whether the
    /// build completion saw it or the loader poll caught it after Step 5.
    /// One helper holds the four steps and both sites call it.
    ///
    /// Read from this file's own source, because a swap needs a loaded
    /// managed project and no unit test can build the host one runs on.
    #[test]
    fn every_managed_swap_site_runs_the_same_bookkeeping() {
        let source = include_str!("runtime.rs");
        // Spelled in two pieces so this test does not match itself.
        let call = concat!("note_managed_swap", "(host);");
        assert_eq!(
            source.matches(call).count(),
            2,
            "the build completion and the loader poll each call the helper once"
        );
        let helper = source
            .split("fn note_managed_swap")
            .nth(1)
            .expect("a managed swap helper")
            .split("\n}")
            .next()
            .expect("a body");
        for step in [
            "forget_prologue_records(host)",
            "HostNotice::Reloaded",
            "resync_patch_baselines(host, true, &[])",
            "bump_editor_revision()",
        ] {
            assert!(helper.contains(step), "the helper skips {step}");
        }
    }

    /// A pause lasts until a renderer attaches, and only that ends it.
    #[test]
    fn a_pause_ends_only_when_a_renderer_attaches() {
        assert_eq!(
            pause_after_boundary(PAUSED, false, RendererStep::Unchanged),
            PAUSED
        );
        assert_eq!(
            pause_after_boundary(PAUSED, false, RendererStep::Failed),
            PAUSED
        );
        assert_eq!(
            pause_after_boundary(PAUSED, false, RendererStep::Attached),
            None
        );
    }
}
