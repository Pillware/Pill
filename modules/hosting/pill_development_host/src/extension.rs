//! Loading and reloading of extension modules.
//!
//! # Responsibilities
//!
//! - Builds, checks and loads one extension when the host starts.
//! - Reloads one module on its own: after a source edit, after a runtime crate
//!   in its folder reloads, or when a retry is requested.
//! - Clears only that module's systems when it is replaced.
//!
//! # Design
//!
//! Each loaded module has one [`ExtensionSlot`]. The slot holds the module's
//! configuration, its [`SystemOwner`], the library that is loaded now and the
//! retired libraries that are still mapped. It keeps a limited number of
//! retired libraries unless the module asks to keep every generation. A reload
//! works like the project reload: the replacement is compiled and loaded
//! before any engine state is touched, and a failure at any step leaves the
//! previous generation running.
//!
//! The module reloads on its own. Its own watcher adds to a counter that only
//! this module reads, and its systems are removed through
//! [`Engine::clear_systems_owned_by`] instead of the global clear. Reloading
//! one module therefore never disturbs the project or another module.

// Standard library
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// External crates
use pill_core::error::{HostError, ModuleError};
use pill_core::info;
use pill_core::telemetry::telemetry_target;
use pill_engine::{Engine, EngineApi, SystemOwner};

// Current crate
use crate::build_runner::build_extension;
use crate::native_library::NativeLibrary;
use crate::ExtensionConfig;

// =============================================================================
// Re-exports
// =============================================================================

/// Extension ABI revision this host understands.
///
/// The host rejects a module that reports any other revision, before it passes
/// the module a pointer into engine memory: the two sides would disagree about
/// the contract that pointer follows.
///
/// The value comes from `pill_engine`, so the host and every module cannot
/// drift apart. `#[pill_module]` generates the module's ABI export from the
/// same constant.
pub use pill_engine::module_abi::MODULE_ABI_VERSION as EXTENSION_ABI_VERSION;

// =============================================================================
// ExtensionSlot
// =============================================================================

// Everything below loads, versions and replaces a module DLL.
pub(crate) use slot::{ExtensionSlot, ReloadOutcome};

mod slot {
    use super::*;

    /// One loaded extension module, with the state a reload needs.
    pub(crate) struct ExtensionSlot {
        /// The module's configuration: how it is built, watched and loaded.
        extension_config: ExtensionConfig,
        /// The owner tag applied to every system this module registers.
        system_owner: SystemOwner,
        /// The library of the generation that is loaded now.
        current_library: NativeLibrary,
        /// The retired libraries, kept mapped because engine-owned pointers and
        /// vtables may still refer to their code.
        retired_libraries: Vec<NativeLibrary>,
        /// The watcher adds one when this module's sources change, and
        /// [`Self::request_reload`] adds one to fake a save. Only the watcher
        /// is a real producer, so the counter never mixes a source save with a
        /// rebuild the reload pipeline queues for another reason.
        source_edit_generation: Arc<AtomicU64>,
        /// The last generation the reload pass acted on.
        last_processed_source_edit_generation: u64,
        /// A rebuild that the reload pipeline queued instead of the watcher: an
        /// editor crate's, after a runtime crate in its folder reloaded. It is
        /// a flag of its own, not part of the watcher's counter, so a fast-path
        /// patch of a source edit cannot consume it by accident.
        cascade_reload_queued: bool,
        /// The persistable component type names the last `init` registered,
        /// used to detect types the next generation forgets to re-register.
        registered_type_names: Vec<String>,
        /// The schema hash of each of those types, as the last `init` declared
        /// it. Comparing them with the world's current hashes finds the types
        /// whose layout another module's reload has replaced since.
        registered_schema_hashes: Vec<(String, u64)>,
        /// The resource ids the last `init` registered. They let the reload
        /// drop a resource this module stops owning while the image that
        /// created it is still mapped.
        registered_resource_ids: Vec<pill_engine::ResourceId>,
        /// Every component type name, plain or persistable, that the last
        /// `init` registered. The C# mirror codegen binds the module's native
        /// components from this list, and the renderer's capability check uses
        /// it to name the components the active renderer does not draw.
        exposed_component_names: Vec<String>,
    }

    impl ExtensionSlot {
        /// Builds, loads and initializes one extension.
        ///
        /// `source_edit_generation` is the module's edit counter, shared with
        /// its watcher. `first_load_failure` decides what a failed `init` costs
        /// the world.
        ///
        /// # Errors
        ///
        /// Returns a [`HostError`] when the module fails to compile, cannot be
        /// loaded, reports a missing or incompatible ABI revision, returns a
        /// non-zero status from `init`, or registers data that the first-load
        /// failure policy forbids.
        pub(crate) fn start(
            engine: &mut Engine,
            engine_api: &EngineApi,
            workspace_root: &Path,
            extension_config: &ExtensionConfig,
            system_owner: SystemOwner,
            source_edit_generation: Arc<AtomicU64>,
            first_load_failure: crate::reload::FirstLoadFailure,
        ) -> Result<Self, HostError> {
            // Step 1: Compile the module through the shared command runner.
            let module_output_path = build_extension(workspace_root, extension_config, None)?;

            // Step 2: Load a uniquely named copy, so the next compilation can
            // replace the build output while this generation stays mapped.
            let module_library = NativeLibrary::load_copy(
                &module_output_path,
                workspace_root,
                &extension_config.name,
            )?;

            // Step 3: Check the contract before handing the module anything.
            module_library.check_module_abi(&extension_config.name)?;

            // Step 4: Register the module's components and systems under this
            // module's owner, so a later reload can remove exactly these
            // systems. The capture, the init call and the failure handling come
            // from `crate::reload` and are shared with the project path.
            //
            // Under `ClearSystemsOnly` the module must not register data of its
            // own, so the names and resource ids that exist before the init are
            // recorded. A shared component that the host already registered,
            // which `#[pill_module]`'s generated `init` re-registers for every
            // derived component it links, is not new data and must not count
            // against the module.
            let previous_registrations = (first_load_failure
                == crate::reload::FirstLoadFailure::ClearSystemsOnly)
                .then(|| {
                    (
                        engine.world().registered_component_names_since(0),
                        engine.world().resource_ids_registered_since(0),
                    )
                });
            let initialized_generation = crate::reload::initialize_generation(
                engine,
                engine_api,
                &extension_config.name,
                Some(system_owner),
                system_owner,
                &module_library,
                first_load_failure,
            );
            // Under both policies that keep the world, a refused image stays
            // mapped for the rest of the run, which is half of what makes
            // keeping the world sound (see `FirstLoadFailure`). That leaks one
            // image and its staged copy; the next host start cleans up the copy.
            let refuse_library = |library: NativeLibrary, error: HostError| {
                if matches!(
                    first_load_failure,
                    crate::reload::FirstLoadFailure::ClearSystemsOnly
                        | crate::reload::FirstLoadFailure::ClearGenerationData
                ) {
                    std::mem::forget(library);
                }
                Err(error)
            };
            if initialized_generation.status != 0 {
                return refuse_library(
                    module_library,
                    ModuleError::InitializationFailed {
                        module: extension_config.name.clone(),
                        status: initialized_generation.status,
                    }
                    .into(),
                );
            }
            // The other half applies to the policy that keeps the world without
            // touching what the artifact registered: a module that may keep the
            // world on failure must own no data. One that registered data nobody
            // had registered before it is therefore refused.
            // `ClearGenerationData` is exempt because it drops exactly that data
            // on the failure path, which is what lets the editor retry a shell
            // module that registers components.
            if let Some((previous_component_names, previous_resource_ids)) = &previous_registrations
            {
                let new_component_count = initialized_generation
                    .component_names
                    .iter()
                    .filter(|name| !previous_component_names.contains(name))
                    .count();
                let new_resource_count = initialized_generation
                    .registered_resource_ids
                    .iter()
                    .filter(|id| !previous_resource_ids.contains(id))
                    .count();
                if new_component_count != 0 || new_resource_count != 0 {
                    engine.clear_systems_owned_by(system_owner);
                    return refuse_library(
                        module_library,
                        ModuleError::RegisteredForbiddenData {
                            module: extension_config.name.clone(),
                            components: new_component_count,
                            resources: new_resource_count,
                        }
                        .into(),
                    );
                }
            }

            info!(
                target: telemetry_target::HOT_RELOAD,
                module = extension_config.name.as_str(),
                owner = system_owner.0,
                // The project's own editor crate is loaded by the same code
                // but is not an extension, and its log line should not say so.
                kind = if system_owner == SystemOwner::PROJECT_EDITOR {
                    "project editor"
                } else {
                    "extension"
                },
                "loadable artifact loaded"
            );

            // What this generation declared its resources' layouts are made of,
            // so the first reload can tell a kept value from one the edit
            // invalidated.
            let watch_directory = workspace_root.join(&extension_config.watch_directory);
            let layout_record = crate::layout_record::LayoutRecordTarget::new(
                workspace_root,
                &watch_directory,
                &extension_config.name,
            );
            crate::layout_record::write_record(
                engine.world(),
                &layout_record,
                &initialized_generation.registered_resource_ids,
            );

            Ok(Self {
                extension_config: extension_config.clone(),
                system_owner,
                current_library: module_library,
                retired_libraries: Vec::new(),
                source_edit_generation,
                last_processed_source_edit_generation: 0,
                cascade_reload_queued: false,
                registered_type_names: initialized_generation.registered_type_names,
                registered_schema_hashes: initialized_generation.registered_schemas,
                registered_resource_ids: initialized_generation.registered_resource_ids,
                exposed_component_names: initialized_generation.component_names,
            })
        }

        /// Whether a watcher signal is waiting to become a reload.
        ///
        /// The caller asks this before it gathers the peer declaration set, so
        /// the per-frame reload pass allocates nothing for modules that have no
        /// pending change.
        pub(crate) fn has_pending_reload(&self) -> bool {
            self.cascade_reload_queued
                || self.source_edit_generation.load(Ordering::Acquire)
                    != self.last_processed_source_edit_generation
        }

        /// Reloads this module when a source edit or a queued cascade reload is
        /// pending.
        ///
        /// Returns [`ReloadOutcome::Reloaded`] when a new generation replaced
        /// the current one, [`ReloadOutcome::Failed`] when an attempt was made
        /// and the previous generation was kept, and
        /// [`ReloadOutcome::Unchanged`] when nothing was pending. The caller
        /// decides what to do from the outcome: a failed attempt leaves every
        /// patch, baseline and record of the current generation in place.
        /// Every module has its own counter, so this never rebuilds another
        /// module or the project.
        pub(crate) fn reload_if_changed(
            &mut self,
            engine: &mut Engine,
            engine_api: &EngineApi,
            workspace_root: &Path,
            peer_component_names: &[String],
        ) -> ReloadOutcome {
            self.reload_if_changed_with(
                engine,
                engine_api,
                workspace_root,
                peer_component_names,
                &mut || {},
            )
        }

        /// Same as [`Self::reload_if_changed`], but runs `on_before_commit` once
        /// the replacement has built, loaded and passed the ABI check, just
        /// before it is swapped in. It does not run at all when an earlier step
        /// refuses the replacement.
        ///
        /// Use it for a module whose caller holds state built on the current
        /// image that has to be released while that image is still mapped: the
        /// renderer module's GPU backend.
        ///
        /// `peer_component_names` lists every component name the other live
        /// subjects declare. A type that only they name is not this module's to
        /// drop when the new generation forgets it.
        pub(crate) fn reload_if_changed_with(
            &mut self,
            engine: &mut Engine,
            engine_api: &EngineApi,
            workspace_root: &Path,
            peer_component_names: &[String],
            on_before_commit: &mut dyn FnMut(),
        ) -> ReloadOutcome {
            // Step 1: Nothing to do when no signal is pending; otherwise one
            // attempt. The generation is read before the build, so a save that
            // arrives during the build stays pending (see Step 3).
            let generation = self.source_edit_generation.load(Ordering::Acquire);
            if generation == self.last_processed_source_edit_generation
                && !self.cascade_reload_queued
            {
                return ReloadOutcome::Unchanged;
            }
            self.cascade_reload_queued = false;

            // Step 2: The reload itself. A refusal leaves the current
            // generation and its patch state untouched.
            info!(
                target: telemetry_target::HOT_RELOAD,
                module = self.extension_config.name.as_str(),
                generation,
                "extension reload triggered"
            );
            let outcome = self.reload(
                engine,
                engine_api,
                workspace_root,
                generation,
                peer_component_names,
                on_before_commit,
            );

            // Step 3: Record the baseline: the generation seen before the
            // reload, not a fresh read. A save during the build moves the
            // counter past this value. Recording the newer value would mark
            // that save as handled although nothing built it, and the edit
            // would sit on disk uncompiled until something else touched the
            // crate. Recording the older value leaves the newer save pending,
            // so the next frame rebuilds with it.
            //
            // This is also what the build cancellation in `run_build_command`
            // depends on: it stops the moment the counter moves, so the newer
            // sources win.
            self.last_processed_source_edit_generation = generation;
            outcome
        }

        /// The crate name of this module. It is the field the log lines carry
        /// and the key callers use to find a module.
        pub(crate) fn name(&self) -> &str {
            &self.extension_config.name
        }

        /// The extension folder holding this module's crate.
        pub(crate) fn folder(&self) -> &str {
            &self.extension_config.folder
        }

        /// Whether this module is its folder's editor crate.
        pub(crate) fn is_editor_crate(&self) -> bool {
            self.extension_config.is_editor_crate
        }

        /// Rebuilds and reloads this module on the next reload pass, although
        /// its own sources did not change. An editor crate links its folder's
        /// runtime crates, so it is out of date once one of them reloads.
        pub(crate) fn queue_cascade_reload(&mut self) {
            self.cascade_reload_queued = true;
        }

        /// Forces a full rebuild and reload on the next pass, as if a source
        /// save had arrived. The editor's safe-mode Retry rebuilds a loaded
        /// shell module this way.
        pub(crate) fn request_reload(&mut self) {
            self.source_edit_generation.fetch_add(1, Ordering::AcqRel);
        }

        /// The generation the watcher has signalled but nothing has handled
        /// yet, if any.
        ///
        /// The per-function fast path uses it to look at a pending change
        /// before [`Self::reload_if_changed`] turns the change into a full
        /// rebuild. It returns the generation itself rather than a flag, so the
        /// caller can pass the exact generation it handled to
        /// [`Self::consume_pending_reload`].
        pub(crate) fn pending_reload_generation(&self) -> Option<u64> {
            let generation = self.source_edit_generation.load(Ordering::Acquire);
            (generation != self.last_processed_source_edit_generation).then_some(generation)
        }

        /// Marks one generation as handled without rebuilding.
        ///
        /// Call this only when a patch has already delivered that edit; the
        /// reload it would otherwise cause has nothing left to do.
        ///
        /// It takes the generation the caller acted on instead of reading a
        /// fresh one, for the same reason [`Self::reload_if_changed`] records
        /// its baseline: a save that arrives while the patch is compiling moves
        /// the counter past that generation, and nothing has delivered that
        /// save. Recording it as handled would leave the edit on disk, never
        /// compiled.
        pub(crate) fn consume_pending_reload(&mut self, generation: u64) {
            self.last_processed_source_edit_generation = generation;
        }

        /// The library of the generation that is loaded now. It is a patch
        /// target, and the renderer module resolves its attach and detach
        /// exports from it.
        pub(crate) fn current_library(&self) -> &NativeLibrary {
            &self.current_library
        }

        /// The address of the current generation's export named `export_name`,
        /// if it has one. The windowed host uses this to find the renderer data
        /// crate's shader reload function.
        ///
        /// The address is valid only while this generation stays current, so
        /// callers look it up again after every reload instead of keeping it
        /// across one.
        #[cfg_attr(not(feature = "rendering"), allow(dead_code))]
        pub(crate) fn export_address(
            &self,
            export_name: &str,
        ) -> Option<pill_engine::component_registry::ExportAddress> {
            // SAFETY: the symbol is read as a plain address and never called
            // here; whoever calls it states its signature.
            let address = unsafe {
                self.current_library
                    .resolve_export::<*const ()>(export_name.as_bytes())
            }?;
            Some(pill_engine::component_registry::ExportAddress(address))
        }

        /// The persistable components this module registered whose layout
        /// another module's reload has replaced since. The list is empty while
        /// this generation is current. See [`crate::reload::stale_components`].
        pub(crate) fn stale_components(&self, world: &pill_engine::World) -> Vec<String> {
            crate::reload::stale_components(world, &self.registered_schema_hashes)
        }

        /// The owner tag this module's systems are registered under.
        pub(crate) fn owner(&self) -> SystemOwner {
            self.system_owner
        }

        /// Every component type name the current generation registered, for
        /// the C# mirror codegen and the renderer's capability check.
        pub(crate) fn exposed_component_names(&self) -> &[String] {
            &self.exposed_component_names
        }

        /// The `#[derive(PillMirror)]` value-type descriptors of this module's
        /// own crate in the current generation, which the C# codegen uses to
        /// resolve nested struct tags.
        ///
        /// Only the descriptors this module's own package declared. An artifact
        /// also carries the declarations of every crate it links (an extension
        /// can depend on another extension), and those belong to the module that
        /// owns them. Emitting them here would add a second C# type with the
        /// same name to the owner's generated file.
        pub(crate) fn value_type_descriptors(
            &self,
        ) -> Vec<pill_engine::component_registry::PillValueTypeDescriptor> {
            let module_name = self.name();
            self.current_library
                .value_type_descriptors()
                .into_iter()
                .filter(|descriptor| descriptor.crate_name == module_name)
                .collect()
        }

        /// The `#[pill_mirror_method]` entries of the current generation, each
        /// with the resolved address of its C-ABI trampoline. The C# codegen
        /// and the managed runtime's method table use them.
        ///
        /// Filtered to this module's own declarations for the same reason as
        /// [`Self::value_type_descriptors`]. The managed table is keyed by the
        /// declaring names, and a linked dependency's copy of the same
        /// trampoline would hide the owner's address. A patch reaches the
        /// owner's copy, so a managed call through that address would run stale
        /// code.
        pub(crate) fn mirror_methods(&self) -> Vec<crate::csharp::ResolvedMirrorMethod> {
            let module_name = self.name();
            self.current_library
                .mirror_methods()
                .into_iter()
                .filter(|method| method.crate_name == module_name)
                .collect()
        }

        /// The heap-field accessors of the current generation, each with the
        /// resolved addresses of the trampolines for its operations. The C#
        /// mirror codegen and the managed runtime's method table use them.
        pub(crate) fn field_accessors(&self) -> Vec<crate::csharp::ResolvedFieldAccessor> {
            self.current_library.field_accessors()
        }

        /// Rebuilds and swaps one generation, and keeps the previous one on any
        /// failure.
        ///
        /// Returns [`ReloadOutcome::Reloaded`] when the swap happened. Every
        /// refusal on the way - build, load, ABI check or a rolled-back init -
        /// returns [`ReloadOutcome::Failed`] with the generation, so the caller
        /// can skip the bookkeeping that only a replaced image justifies and
        /// still log which edit was lost.
        fn reload(
            &mut self,
            engine: &mut Engine,
            engine_api: &EngineApi,
            workspace_root: &Path,
            generation: u64,
            peer_component_names: &[String],
            on_before_commit: &mut dyn FnMut(),
        ) -> ReloadOutcome {
            // Steps 1 to 3 are shared with the project path and live in
            // `crate::reload`: compile before touching engine state (a newer
            // save during the build cancels it and the next frame retries),
            // then load a private copy and verify its ABI, and finally swap
            // through the transaction, whose own step order is load-bearing.
            // The record this transaction reads and rewrites is keyed by the
            // module and resolves declared paths against its crate root.
            let watch_directory = workspace_root.join(&self.extension_config.watch_directory);
            let transaction = crate::reload::ReloadTransaction {
                kind: crate::reload::ReloadSubjectKind::Extension,
                subject: &self.extension_config.name,
                owner: self.system_owner,
                current: &mut self.current_library,
                old_libraries: &mut self.retired_libraries,
                registered_type_names: &mut self.registered_type_names,
                registered_schemas: &mut self.registered_schema_hashes,
                registered_resource_ids: &mut self.registered_resource_ids,
                peer_component_names,
                layout_record: crate::layout_record::LayoutRecordTarget::new(
                    workspace_root,
                    &watch_directory,
                    &self.extension_config.name,
                ),
            };
            let Some(commit) = crate::reload::build_load_and_commit(
                engine,
                engine_api,
                workspace_root,
                |cancel_flag| build_extension(workspace_root, &self.extension_config, cancel_flag),
                Some((&self.source_edit_generation, generation)),
                crate::reload::LoadValidation::ModuleAbi,
                on_before_commit,
                transaction,
            ) else {
                // A refused step left the previous generation running.
                return ReloadOutcome::Failed { generation };
            };
            // Refresh the C#-exposed component set with what the new generation
            // registered (plain and persistable alike).
            self.exposed_component_names = commit.exposed_component_names;
            ReloadOutcome::Reloaded { generation }
        }
    }

    impl Drop for ExtensionSlot {
        /// Announces the unload of this module's images, current and retired.
        ///
        /// The fields are dropped right after this body, and each library field
        /// unmaps an image that engine-owned pointers may still point into.
        /// That is the riskiest moment in a host's life, so each module names
        /// itself in the log before its images go.
        fn drop(&mut self) {
            info!(
                target: telemetry_target::HOT_RELOAD,
                module = %self.extension_config.name,
                generations = self.retired_libraries.len() + 1,
                "unloading extension"
            );
        }
    }

    // =========================================================================
    // ReloadOutcome
    // =========================================================================

    /// What a module reload attempt did.
    ///
    /// There are three states rather than a `bool` because each one asks the
    /// caller for a different action. Only [`Reloaded`] justifies the success
    /// bookkeeping: the patch baselines are rebuilt, the recorded prologues are
    /// forgotten and the editor revision moves. A failed attempt must leave all
    /// of that alone, because the previous image is still current, its patches
    /// are still installed and its prologues are still the rollback path.
    /// "Nothing was pending" is not an attempt at all.
    ///
    /// The outcome carries the generation, so the log lines can name the edit
    /// the outcome belongs to instead of whatever the counter holds by the time
    /// they run.
    ///
    /// [`Reloaded`]: ReloadOutcome::Reloaded
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum ReloadOutcome {
        /// The watcher signalled nothing new and no cascade rebuild was
        /// queued, so no attempt was made.
        Unchanged,
        /// The new generation was built, initialized and swapped in.
        Reloaded {
            /// The source-edit generation this reload delivered.
            generation: u64,
        },
        /// The build produced nothing usable, or the swap was refused. The
        /// previous generation is still current.
        ///
        /// No error string: every refusal already logs its own reason where it
        /// is detected, and a second copy here would print it twice.
        Failed {
            /// The source-edit generation the attempt was made for.
            generation: u64,
        },
    }
}
