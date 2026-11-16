//! Lifecycle management for the active native or managed project module.
//!
//! # Responsibilities
//!
//! - Builds and loads the selected backend during startup.
//! - Reloads a native generation through a graveyard (the retired library
//!   images the host keeps mapped), so pointers into an earlier image stay
//!   valid.
//! - Leaves managed reload polling to the C# bridge's runtime, through
//!   `crate::csharp`.
//! - Keeps a record of each generation's registrations - persistable type
//!   names, schema hashes, resource ids and every component name - so the next
//!   reload can tell what changed and what another module still declares.
//!
//! # Design
//!
//! The host keeps exactly one [`LoadedProject`] alive at a time. A native
//! reload runs through [`reload_native`], which hands the build, the private
//! copy and the swap to `crate::reload`. The replaced DLL moves into a bounded
//! graveyard and stays mapped until no column needs its tables, so
//! engine-owned pointers into the old image stay valid while changed persist
//! schemas are migrated. A managed backend leaves assembly discovery and
//! validation to the C# bridge's runtime, which reports success or rejection
//! through `poll_reload`.

// This module covers the whole loaded-project lifecycle: build, load,
// initialize, reload and retire.
pub(crate) use loaded::{LoadedProject, ManagedReloadStart};

mod loaded {
    // Standard library
    use std::path::Path;
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc;
    use std::sync::Arc;
    use std::thread;

    // External crates
    use pill_core::error::{HostError, LibraryError};
    use pill_core::platform::Instant;
    use pill_core::telemetry::telemetry_target;
    use pill_core::{error, info};
    use pill_engine::{Engine, EngineApi, SystemOwner};

    // Current crate
    use crate::build_runner::build_project_module;
    use crate::csharp::{
        BuildCollection, BuildOutcome, BuildReport, CSharpProject, FastCompileOutcome,
        FastCompiler, ManagedReloadSummary, POLL_REJECTED, POLL_RELOADED,
    };
    use crate::native_library::NativeLibrary;
    use crate::watchers::SourceTrigger;
    use crate::{ProjectModuleBackend, ProjectModuleConfig};

    // =============================================================================
    // ManagedReloadStart
    // =============================================================================

    /// What starting a reload through the managed path produced.
    pub(crate) enum ManagedReloadStart {
        /// The project's backend is native, so the caller reloads it on the
        /// frame thread.
        NotManaged,
        /// A worker thread is building the assembly. The frame keeps running,
        /// and the result is collected when
        /// [`LoadedProject::collect_managed_build`] reports the build.
        Building,
        /// The build thread could not start. The caller builds on the frame
        /// thread through [`LoadedProject::build_now`], and that report goes
        /// through the same handler as a worker's report.
        Unavailable,
    }

    // =============================================================================
    // LoadedProject
    // =============================================================================

    /// The backend-specific state the host loop keeps alive.
    ///
    /// The host holds either a mapped native library or a managed runtime,
    /// behind one interface. The backend is fixed at startup, so the variant
    /// only changes across a full restart.
    pub(crate) enum LoadedProject {
        /// A mapped native module plus any retired DLLs that must stay mapped.
        Native {
            /// The generation the host is calling into right now.
            current: NativeLibrary,
            /// The DLLs of earlier generations stay mapped on purpose:
            /// engine-owned function pointers and vtables may still point into
            /// their code.
            old_libraries: Vec<NativeLibrary>,
            /// Persistable component type names the last `pill_module_init`
            /// registered. The next reload compares them to find types the new
            /// generation no longer registers.
            registered_type_names: Vec<String>,
            /// The schema hash of each of those types, as the last
            /// `pill_module_init` declared it. Comparing them with the world
            /// finds the types whose layout another module's reload has
            /// replaced since.
            registered_schemas: Vec<(String, u64)>,
            /// Resource ids the last `pill_module_init` registered, so a
            /// resource the project stops owning can be dropped while the
            /// image that holds its drop function is still mapped.
            registered_resource_ids: Vec<pill_engine::ResourceId>,
            /// Every component type name the last `pill_module_init`
            /// registered, both plain and persistable.
            ///
            /// A module reload treats the project as a peer: a shared type the
            /// project also declares is not the module's to retire, so before
            /// a module drops a forgotten type it checks this set.
            registered_component_names: Vec<String>,
        },
        /// A collectible managed runtime hosting the C# project assembly,
        /// with the in-process compiler that rebuilds it.
        CSharp(CSharpProject),
    }

    impl LoadedProject {
        /// The native library the loaded project uses now, as a patch target.
        ///
        /// `None` for the C# backend, which has no native redirect slots to
        /// install into.
        pub(crate) fn native_library(&self) -> Option<&NativeLibrary> {
            match self {
                Self::Native { current, .. } => Some(current),
                Self::CSharp(_) => None,
            }
        }

        /// The persistable components the native project registered whose
        /// layout has been replaced since, by another module's reload. Empty
        /// while the layouts are current, and always empty for the C# backend,
        /// whose components are its own descriptor-defined set. See
        /// [`crate::reload::stale_components`].
        pub(crate) fn stale_components(&self, world: &pill_engine::World) -> Vec<String> {
            match self {
                Self::Native {
                    registered_schemas, ..
                } => crate::reload::stale_components(world, registered_schemas),
                Self::CSharp(_) => Vec::new(),
            }
        }

        /// Every component type name the loaded project's generation
        /// registered, both plain and persistable.
        ///
        /// A module reload reads it to decide whether a type it forgot is
        /// still declared by the project. Empty for the C# backend, whose
        /// components are its own descriptor-defined set.
        pub(crate) fn component_names(&self) -> &[String] {
            match self {
                Self::Native {
                    registered_component_names,
                    ..
                } => registered_component_names,
                Self::CSharp(_) => &[],
            }
        }

        /// Build and initialize the configured project backend.
        ///
        /// # Errors
        ///
        /// Returns `HostError` when the module fails to build, when its output
        /// artifact is missing, when the native library cannot be loaded, when the
        /// module's `pill_module_init` reports a non-zero initialization status, or
        /// when the managed runtime fails to start the C# project.
        pub(crate) fn start(
            engine: &mut Engine,
            engine_api: &EngineApi,
            workspace_root: &Path,
            config: &ProjectModuleConfig,
            module_exposed_components: &[crate::csharp::ModuleExposedComponent],
            mirror_methods: &[crate::csharp::ResolvedMirrorMethod],
        ) -> Result<Self, HostError> {
            // Step 1: Build the module through the shared command runner.
            // Build before choosing a backend, so both use the same command
            // runner, diagnostics, output validation and failure handling.
            let output_path = build_project_module(workspace_root, config, None)?;

            // Step 2: Initialize the backend-specific runtime.
            match &config.backend {
                ProjectModuleBackend::NativeLibrary { .. } => {
                    // Native build outputs cannot be loaded in place on Windows:
                    // the OS locks a mapped DLL. Load a uniquely named copy so the
                    // next compilation remains free to replace the original.
                    let library =
                        NativeLibrary::load_copy(&output_path, workspace_root, &config.name)?;

                    // Native modules register their components and systems
                    // through the stable EngineApi table before the first frame
                    // runs. The capture, init and failure handling are shared
                    // with the extension path - see `crate::reload`.
                    let generation_init = crate::reload::initialize_generation(
                        engine,
                        engine_api,
                        &config.name,
                        None,
                        SystemOwner::PROJECT,
                        &library,
                        crate::reload::FirstLoadFailure::ClearWorld,
                    );
                    if generation_init.status != 0 {
                        return Err(LibraryError::InitializationFailed {
                            status: generation_init.status,
                        }
                        .into());
                    }
                    // What this generation declared its resources' layouts are
                    // made of, so the first reload can tell a kept value from one
                    // the edit invalidated.
                    let watch_directory = workspace_root.join(&config.watch_directory);
                    let layout_record = crate::layout_record::LayoutRecordTarget::new(
                        workspace_root,
                        &watch_directory,
                        &config.name,
                    );
                    crate::layout_record::write_record(
                        engine.world(),
                        &layout_record,
                        &generation_init.registered_resource_ids,
                    );
                    Ok(Self::Native {
                        current: library,
                        old_libraries: Vec::new(),
                        registered_type_names: generation_init.registered_type_names,
                        registered_schemas: generation_init.registered_schemas,
                        registered_resource_ids: generation_init.registered_resource_ids,
                        registered_component_names: generation_init.component_names,
                    })
                }
                // The managed runtime performs assembly discovery, component
                // registration, startup commands, and system registration itself.
                ProjectModuleBackend::CSharp(config) => Ok(Self::CSharp(CSharpProject::start(
                    engine,
                    workspace_root,
                    config,
                    module_exposed_components,
                    mirror_methods,
                )?)),
            }
        }

        /// Rebuilds and replaces the active module, keeping a working old
        /// generation running whenever compilation, loading or registration
        /// fails.
        ///
        /// Returns whether the running image was replaced. Every failed branch
        /// (a build error, a load refusal, a rolled-back init) keeps the
        /// current image and returns `false`. The caller uses that answer to
        /// gate bookkeeping that only a replacement invalidates, such as patch
        /// records pointing into an image the graveyard will unmap.
        ///
        /// `cancel_flag` is the watcher's reload signal: a newer save during
        /// the build cancels the compilation in flight, and the next frame
        /// tries again.
        pub(crate) fn reload(
            &mut self,
            engine: &mut Engine,
            engine_api: &EngineApi,
            workspace_root: &Path,
            config: &ProjectModuleConfig,
            cancel_flag: Option<(&AtomicU64, u64)>,
            peer_component_names: &[String],
        ) -> bool {
            match self {
                // Native reload owns schema migration and DLL lifetime handling, so
                // keep that transaction isolated in one dedicated function.
                Self::Native {
                    current,
                    old_libraries,
                    registered_type_names,
                    registered_schemas,
                    registered_resource_ids,
                    registered_component_names,
                } => reload_native(
                    current,
                    old_libraries,
                    registered_type_names,
                    registered_schemas,
                    registered_resource_ids,
                    registered_component_names,
                    peer_component_names,
                    engine,
                    engine_api,
                    workspace_root,
                    config,
                    cancel_flag,
                ),
                // Managed reloads never come through here. A C# reload
                // produces a [`BuildReport`] - from the build worker, or from
                // [`Self::build_now`] when the worker could not start - and
                // the host's single `finish_managed_build` handler consumes
                // every outcome, owning the record, notify and poll steps and
                // their log lines. Reaching this arm would mean a caller
                // bypassed that handler.
                Self::CSharp(_) => {
                    debug_assert!(false, "managed reload bypassed finish_managed_build");
                    false
                }
            }
        }

        /// Polls the managed loader after its assembly debounce (the short
        /// wait that groups a burst of file changes into one batch).
        ///
        /// Returns whether this poll swapped in a new assembly. A managed
        /// reload completes in one of two ways. An in-process compile calls
        /// `NotifyAssemblyReplaced`, which ends the loader's debounce. A
        /// rebuild through the `dotnet build` fallback leaves no completion
        /// signal of its own, so the host polls the loader every frame to give
        /// it a chance to notice the settled assembly. The debounce can last
        /// past the frame that started the build, which is why the swap - and
        /// the bookkeeping it invalidates - is checked here as well as when
        /// the build completes.
        pub(crate) fn poll_managed_reload(&mut self, engine: &mut Engine) -> bool {
            // Source and assembly watchers have independent debounce windows.
            // Poll every frame so a successful build is eventually observed,
            // even when the assembly was not ready during the source-triggered
            // reload call.
            if let Self::CSharp(project) = self {
                return poll_managed_assembly_swap(project, engine);
            }
            false
        }

        /// Arms the managed backend's reload timing for the reload this
        /// signal starts.
        ///
        /// A native project has no managed reload to time, so it ignores the
        /// signal and arms nothing. The caller takes the trigger out of the
        /// watcher's log for this reload either way, so none is left pending.
        pub(crate) fn arm_managed_reload_timing(&mut self, trigger: Option<SourceTrigger>) {
            if let Self::CSharp(project) = self {
                project.arm_reload_timing(trigger);
            }
        }

        /// Drops an armed managed reload timing whose reload produced no swap.
        pub(crate) fn abandon_managed_reload_timing(&mut self) {
            if let Self::CSharp(project) = self {
                project.abandon_reload_timing();
            }
        }

        /// Finishes the managed backend's armed reload timing at the swap.
        ///
        /// A native project has no managed reload to time, so it returns
        /// `None`; the caller records the swap with zeros.
        pub(crate) fn finish_managed_reload_timing(&mut self) -> Option<ManagedReloadSummary> {
            if let Self::CSharp(project) = self {
                return project.finish_reload_timing();
            }
            None
        }

        /// Records a managed assembly rebuild that finished at `finished_at`,
        /// and what produced it.
        pub(crate) fn record_assembly_rebuilt_at(&mut self, finished_at: Instant) {
            if let Self::CSharp(project) = self {
                project.record_assembly_rebuilt_at(finished_at);
            }
        }

        /// Collapses the managed loader's debounce after the in-process
        /// compiler wrote a settled assembly.
        pub(crate) fn notify_assembly_replaced(&mut self) {
            if let Self::CSharp(project) = self {
                project.notify_assembly_replaced();
            }
        }

        /// Starts a managed reload: the assembly builds on its own thread
        /// while the frame loop keeps running, and the result is collected
        /// when the build reports. See [`ManagedReloadStart`].
        pub(crate) fn start_managed_reload(
            &mut self,
            workspace_root: &Path,
            config: &ProjectModuleConfig,
            cancel_flag: Option<(Arc<AtomicU64>, u64)>,
        ) -> ManagedReloadStart {
            match self {
                Self::CSharp(project) => {
                    start_csharp_assembly_build(project, workspace_root, config, cancel_flag)
                }
                Self::Native { .. } => ManagedReloadStart::NotManaged,
            }
        }

        /// Tries to collect a finished managed assembly build without waiting.
        ///
        /// A native project never has one.
        pub(crate) fn collect_managed_build(&mut self) -> BuildCollection {
            match self {
                Self::CSharp(project) => project.collect_build(),
                Self::Native { .. } => BuildCollection::Pending,
            }
        }

        /// Builds the managed assembly on the frame thread and returns the
        /// report.
        ///
        /// The synchronous twin of the build worker: the same compiler/MSBuild
        /// selection, wrapped as a [`BuildReport`] so the caller consumes it
        /// through the one outcome handler the worker's reports also go
        /// through. Used when the worker could not start, or when one stopped
        /// without reporting. `None` for a native project, which has no
        /// assembly to build.
        pub(crate) fn build_now(
            &mut self,
            workspace_root: &Path,
            config: &ProjectModuleConfig,
            cancel_flag: Option<(Arc<AtomicU64>, u64)>,
        ) -> Option<BuildReport> {
            match self {
                Self::CSharp(project) => Some(BuildReport {
                    outcome: build_csharp_outcome(
                        project.compiler_handle(),
                        workspace_root,
                        config,
                        cancel_flag,
                    ),
                    finished_at: Instant::now(),
                }),
                Self::Native { .. } => None,
            }
        }

        /// Whether a managed assembly build is running on its own thread.
        pub(crate) fn managed_build_in_flight(&self) -> bool {
            match self {
                Self::CSharp(project) => project.build_in_flight(),
                Self::Native { .. } => false,
            }
        }
    }

    // =============================================================================
    // Free Functions
    // =============================================================================

    /// Polls the managed loader and returns whether it swapped the assembly.
    ///
    /// The function exists so a poll error is reported instead of dropped.
    /// `poll_reload` logs the statuses it understands, but a `CSharpError`
    /// returned through `?` reaches the caller as a plain `Err`, which its
    /// callers would otherwise discard. The one that matters is
    /// re-registration refusing the arriving assembly's systems: a project
    /// left with no systems and nothing on the console is the worst outcome
    /// available here, so the error is logged loudly.
    fn poll_managed_assembly_swap(project: &mut CSharpProject, engine: &mut Engine) -> bool {
        match project.poll_reload(engine) {
            // A swap is reported by `note_managed_swap`, which is the one place
            // that finishes the timing and records the reload in the shared
            // report, whichever site saw the swap.
            Ok(status) if status == POLL_RELOADED => true,
            Ok(status) => {
                if status == POLL_REJECTED {
                    // The loader refused the arriving assembly, so this
                    // attempt ends without a swap.
                    project.abandon_reload_timing();
                }
                false
            }
            Err(error) => {
                error!(
                    target: telemetry_target::HOT_RELOAD,
                    error = %error,
                    "the managed reload poll failed; the project may now be running without its systems"
                );
                false
            }
        }
    }

    /// Produces one managed build outcome, using the compiler choice the build
    /// worker and the synchronous fallback share.
    ///
    /// The in-process compiler replays the compiler command line captured by
    /// the project's last full build. That takes tens of milliseconds, where a
    /// `dotnet build` of the same edit takes seconds. It cannot answer every
    /// reload - adding or removing a source file changes the command line
    /// itself - and it may not be loaded at all, so both cases fall back to
    /// the full `dotnet build`. That build also refreshes the captured command
    /// line, which is what makes the reload after it fast again.
    fn build_csharp_outcome(
        compiler: Option<Arc<FastCompiler>>,
        workspace_root: &Path,
        config: &ProjectModuleConfig,
        cancel_flag: Option<(Arc<AtomicU64>, u64)>,
    ) -> BuildOutcome {
        match compiler {
            Some(compiler) => match compiler.compile(workspace_root, &config.watch_directory) {
                FastCompileOutcome::Compiled { milliseconds } => {
                    BuildOutcome::Compiled { milliseconds }
                }
                FastCompileOutcome::Failed { diagnostics } => BuildOutcome::Failed { diagnostics },
                FastCompileOutcome::Unavailable { reason } => {
                    info!(
                        target: telemetry_target::HOT_RELOAD,
                        reason = reason.as_str(),
                        "falling back to a full C# build"
                    );
                    BuildOutcome::Full(run_full_project_build(workspace_root, config, cancel_flag))
                }
            },
            None => {
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    reason = "no in-process compiler is loaded",
                    "falling back to a full C# build"
                );
                BuildOutcome::Full(run_full_project_build(workspace_root, config, cancel_flag))
            }
        }
    }

    /// Starts a C# assembly build on its own worker thread.
    ///
    /// The worker owns everything the build needs and touches nothing that
    /// belongs to the frame boundary. It runs the in-process compiler, or the
    /// full `dotnet build` fallback when the captured command line cannot
    /// answer the reload, and reports what it produced. Compiling the project
    /// costs hundreds of milliseconds, so it happens here: the frame keeps
    /// rendering and the report is collected at a later boundary.
    fn start_csharp_assembly_build(
        project: &mut CSharpProject,
        workspace_root: &Path,
        config: &ProjectModuleConfig,
        cancel_flag: Option<(Arc<AtomicU64>, u64)>,
    ) -> ManagedReloadStart {
        let compiler = project.compiler_handle();
        let workspace = workspace_root.to_path_buf();
        let build_config = config.clone();
        let (sender, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("pill-csharp-build".to_string())
            .spawn(move || {
                let outcome =
                    build_csharp_outcome(compiler, &workspace, &build_config, cancel_flag);
                // A receiver that has gone away means the host is shutting
                // down; the report's job is then nobody's.
                let _ = sender.send(BuildReport {
                    outcome,
                    finished_at: Instant::now(),
                });
            });
        match thread {
            Ok(thread) => {
                project.track_build(receiver, thread);
                ManagedReloadStart::Building
            }
            Err(_) => {
                // Building on this thread blocks the frame, which is the safe
                // direction for a spawn failure.
                ManagedReloadStart::Unavailable
            }
        }
    }

    /// Runs the full `dotnet build` for a reload the captured command line
    /// could not answer, reporting its error as text for the reload log.
    ///
    /// # Errors
    ///
    /// Returns the build's error rendered as text when the build fails or its
    /// output artifact is missing; the caller logs it and keeps the running
    /// assembly.
    fn run_full_project_build(
        workspace_root: &Path,
        config: &ProjectModuleConfig,
        cancel_flag: Option<(Arc<AtomicU64>, u64)>,
    ) -> Result<(), String> {
        let borrowed_cancel_flag = cancel_flag
            .as_ref()
            .map(|(generation, baseline)| (generation.as_ref(), *baseline));
        build_project_module(workspace_root, config, borrowed_cancel_flag)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Reload one native generation and migrate components whose persisted schema
    /// changed across the module boundary.
    // Twelve parameters, and they are twelve distinct collaborators rather than
    // fields of an implicit struct: the six pieces of generation state this
    // mutates, the engine and its API table, where to build, what to build, the
    // cancellation signal, and the set of names the live modules declare.
    // Grouping them would name a type that exists only to satisfy the lint, and
    // would hide which of them this function mutates.
    #[allow(clippy::too_many_arguments)]
    fn reload_native(
        current: &mut NativeLibrary,
        old_libraries: &mut Vec<NativeLibrary>,
        registered_type_names: &mut Vec<String>,
        registered_schemas: &mut Vec<(String, u64)>,
        registered_resource_ids: &mut Vec<pill_engine::ResourceId>,
        registered_component_names: &mut Vec<String>,
        peer_component_names: &[String],
        engine: &mut Engine,
        engine_api: &EngineApi,
        workspace_root: &Path,
        config: &ProjectModuleConfig,
        cancel_flag: Option<(&AtomicU64, u64)>,
    ) -> bool {
        // Step 1: Build, load and commit through the shared transaction. The
        // extension path runs the same three steps in the same order, and the
        // order matters: nothing touches engine state before the compile
        // succeeds. A refused step returns `None`, which for the project means
        // nothing was replaced.
        // The record this transaction reads and rewrites is keyed by the
        // project and resolves declared paths against its crate root.
        let watch_directory = workspace_root.join(&config.watch_directory);
        let transaction = crate::reload::ReloadTransaction {
            kind: crate::reload::ReloadSubjectKind::Project,
            subject: &config.name,
            owner: SystemOwner::PROJECT,
            current,
            old_libraries,
            registered_type_names,
            registered_schemas,
            registered_resource_ids,
            peer_component_names,
            layout_record: crate::layout_record::LayoutRecordTarget::new(
                workspace_root,
                &watch_directory,
                &config.name,
            ),
            graveyard_policy: crate::reload::GraveyardPolicy::Bounded,
        };
        let Some(commit) = crate::reload::build_load_and_commit(
            engine,
            engine_api,
            workspace_root,
            |cancel_flag| build_project_module(workspace_root, config, cancel_flag),
            cancel_flag,
            crate::reload::LoadValidation::None,
            &mut || {},
            transaction,
        ) else {
            return false;
        };

        // Step 2: The swapped generation's declarations replace the previous
        // set, so a later module reload asks about the project as it stands.
        *registered_component_names = commit.exposed_component_names;
        true
    }
}
