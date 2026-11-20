//! The sequence every reload runs after its replacement image has loaded.
//!
//! # Responsibilities
//!
//! - Save the retiring generation's persistable metadata, then run the new
//!   generation's init. If init fails, put the old generation back.
//! - Drop what the new generation no longer registers, and point every
//!   surviving storage table at code that is still loaded.
//! - Migrate persistable data whose layout changed in the swap.
//! - Move the previous image into the graveyard.
//!
//! # Design
//!
//! The project and every extension run the same sequence. They differ in three
//! ways. An extension tags its registrations with its [`SystemOwner`], while
//! the project owns the scheduler and tags nothing. Their log text differs
//! because the suites in `devops/tests` wait for those exact lines, so the two
//! wordings must stay tellable apart. Both keep the component names their
//! generation registered, but only a module's names go on to the C# mirror.
//!
//! **The step order is load-bearing and must not be changed.**
//! `drop_forgotten_components` calls drop functions that live in the retiring
//! image, so it has to run while that image is still loaded.
//! `rehome_native_columns` then gives every native column (the table of
//! function pointers that operates on one component's stored rows) a table from
//! a generation that is still loaded. Both must finish before the graveyard
//! releases any image; that order is what makes a bound of two generations
//! safe. Swapping them causes a call into unloaded memory, and the crash shows
//! up two reloads later, not at once.

// Standard library
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

// External crates
use pill_core::error::BuildError;
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::{debug, error, info, warn};
use pill_engine::{ComponentId, Engine, EngineApi, SystemOwner, World};

// Current crate
use crate::analytics;
use crate::native_library::NativeLibrary;
use crate::notices::{self, HostNotice};

// =============================================================================
// Constants
// =============================================================================

/// Maximum number of retired images kept loaded for one subject.
///
/// The previous generation has to stay loaded because the engine may still hold
/// pointers into its code. Older generations can be released once the
/// forgotten-type drop and the column re-home have re-pointed everything that
/// could still reach them.
pub(crate) const MAX_GRAVEYARD_GENERATIONS: usize = 2;

/// Number of deferred evictions in a row allowed before the warning becomes an
/// error.
///
/// Waiting is correct while a column still points into a retired image. But if
/// the wait never ends, every reload leaks one loaded image and only a warning
/// says so. Raising the level shows a real stuck state while there is still
/// time to fix it, and keeps a single ordinary deferral quiet.
const MAX_DEFERRED_EVICTIONS: u32 = 3;

/// Final line of a project reload.
///
/// This constant and its three neighbours below are read by tests, not prose.
/// `devops/core/suite_common.py` and the suites in `devops/tests` wait for
/// these phrases in the host output, and `devops/tests/test_log_contract.py`
/// fails if either member of a pair stops appearing verbatim in the Rust
/// source. The project and module texts stay separate for that reason: one
/// shared message with an inserted noun would print the same text for both
/// subjects, and no suite could tell them apart.
const PROJECT_RELOAD_COMPLETE: &str = "hot reload complete";

/// Final line of an extension reload.
const MODULE_RELOAD_COMPLETE: &str = "extension hot reload complete";

/// Forgotten-type warning for a project reload.
const PROJECT_FORGOTTEN_TYPES: &str = "component type(s) no longer registered by the project; their data stays in the world but is orphaned (the new generation cannot read it)";

/// Forgotten-type warning for an extension reload.
const MODULE_FORGOTTEN_TYPES: &str = "component type(s) no longer registered by this module; their data stays in the world but is orphaned (the new generation cannot read it)";

// =============================================================================
// Types
// =============================================================================

/// Which kind of subject a reload is running for: the project or an extension.
///
/// Holds the two things that differ between them and are not part of the
/// subject's identity: whether registrations are scoped to an owner, and the
/// exact text of the log lines the Python suites wait for. They live together
/// because a caller that picks the right behaviour and the wrong text gets a
/// reload that works but is reported as the wrong subject.
pub(crate) enum ReloadSubjectKind {
    /// The game project. It owns the scheduler and tags no registrations, and
    /// its completion line is the one the migration and auto-reload suites
    /// wait for.
    Project,
    /// An extension. It adds systems next to the project and the other
    /// extensions, so its registrations are tagged with its owner.
    Extension,
}

impl ReloadSubjectKind {
    /// Whether registrations made during `init` are tagged with the subject's
    /// owner, so a later reload clears only that subject's systems.
    ///
    /// Only an extension tags them. The project owns the scheduler and leaves
    /// registrations untagged.
    fn scopes_registration(&self) -> bool {
        matches!(self, Self::Extension)
    }

    /// Log line written once the swap succeeds.
    fn completion_message(&self) -> &'static str {
        match self {
            Self::Project => PROJECT_RELOAD_COMPLETE,
            Self::Extension => MODULE_RELOAD_COMPLETE,
        }
    }

    /// Warning written when the new generation stops registering a type whose
    /// data is still in the world.
    fn forgotten_types_warning(&self) -> &'static str {
        match self {
            Self::Project => PROJECT_FORGOTTEN_TYPES,
            Self::Extension => MODULE_FORGOTTEN_TYPES,
        }
    }

    /// Failure line for a build that produced no usable replacement.
    ///
    /// Kept per kind instead of built from a template: the Python suites wait
    /// for the project's exact text, and `test_log_contract.py` fails if a
    /// token they use stops appearing in the Rust source.
    fn build_failed_message(&self) -> &'static str {
        match self {
            Self::Project => "build failed; keeping the old project module",
            Self::Extension => "build failed; keeping the old module generation",
        }
    }

    /// Failure line for a replacement that could not be loaded.
    fn load_failed_message(&self) -> &'static str {
        match self {
            Self::Project => "failed to load the new library; keeping the old project module",
            Self::Extension => "failed to load the new library; keeping the old module generation",
        }
    }

    /// Failure line for a replacement the load-time validation refused.
    fn rejected_message(&self) -> &'static str {
        match self {
            // The host builds the project from its own workspace, so the ABI
            // always matches and this arm never runs. It exists so the match is
            // complete.
            Self::Project => "rejected the new library; keeping the current generation",
            Self::Extension => "rejected the new library; keeping the old module generation",
        }
    }
}

/// Whether a subject's retired images are ever unloaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GraveyardPolicy {
    /// Keep at most [`MAX_GRAVEYARD_GENERATIONS`] retired images loaded, and
    /// release the oldest beyond that once no column still needs its tables.
    Bounded,
    /// Never unload a retired image.
    ///
    /// For a subject whose images leave pointers behind that this code cannot
    /// track or free. The renderer module is one: wgpu, the graphics driver and
    /// the `tracing` callsite registry in the shared `pill_core.dll` all hold
    /// state that points into the image that created it, and releasing a
    /// renderer image hangs the host's frame loop. The cost is one loaded image
    /// per reload, which a development session can afford.
    KeepAll,
}

/// One subject's state, borrowed for one reload.
///
/// Created for each reload instead of being stored. It borrows the caller's
/// library slot and graveyard mutably, and only for the length of the
/// transaction.
pub(crate) struct ReloadTransaction<'a> {
    /// Name that appears in every log line and analytics record.
    pub(crate) subject: &'a str,
    /// The kind of subject, which decides the log wording and whether
    /// registrations are scoped.
    pub(crate) kind: ReloadSubjectKind,
    /// The systems this reload may clear.
    pub(crate) owner: SystemOwner,
    /// The generation being retired. It stays loaded for the whole transaction.
    pub(crate) current: &'a mut NativeLibrary,
    /// Retired generations that are still loaded.
    pub(crate) old_libraries: &'a mut Vec<NativeLibrary>,
    /// Persistable type names the previous `init` registered.
    pub(crate) registered_type_names: &'a mut Vec<String>,
    /// Schema hash of each persistable type the previous `init` registered, as
    /// that generation declared it. See [`stale_components`].
    pub(crate) registered_schemas: &'a mut Vec<(String, u64)>,
    /// Resource ids the previous generation of this subject registered.
    ///
    /// Compared with what the new generation registers. That lets a resource
    /// type the subject no longer owns be dropped while the image holding its
    /// drop function is still loaded.
    pub(crate) registered_resource_ids: &'a mut Vec<pill_engine::ResourceId>,
    /// Every component type name, plain or persistable, that the other live
    /// subjects declare. The caller gathers it from their current generations.
    ///
    /// A type another artifact still declares does not belong to this subject.
    /// Every artifact that links a shared crate registers a shared identity, so
    /// dropping its columns here would erase the peer's rows and its
    /// registration. The world keeps an append-only registration log, which
    /// cannot say who declares a type now. That is why the caller passes the
    /// set in instead of this code querying it.
    pub(crate) peer_component_names: &'a [String],
    /// Where this subject's layout record lives, and what a declared source
    /// path resolves against.
    ///
    /// The record describes the generation that loaded last, so a step before
    /// the new `init` can compare the declared resources with it and drop the
    /// ones whose layout changed.
    pub(crate) layout_record: crate::layout_record::LayoutRecordTarget<'a>,
    /// Whether retired generations may be released.
    pub(crate) graveyard_policy: GraveyardPolicy,
}

/// What a committed reload registered, for the caller to keep.
pub(crate) struct ReloadCommit {
    /// Every component type name the new generation registered, plain or
    /// persistable.
    ///
    /// A module's set goes on so the C# mirror can bind its native components.
    /// A project's set replaces the project's own record, which the next module
    /// reload reads when it gathers peer names.
    pub(crate) exposed_component_names: Vec<String>,
}

/// What one generation's `init` registered while its image was loaded.
pub(crate) struct GenerationInit {
    /// Status the artifact's entry point returned. Zero means success.
    pub(crate) status: u32,
    /// Persistable component type names this generation registered, for
    /// forgotten-type detection on the next reload.
    pub(crate) registered_type_names: Vec<String>,
    /// Schema hash of each of those types as this generation declared it.
    pub(crate) registered_schemas: Vec<(String, u64)>,
    /// Resource ids this generation claimed, so retiring it releases exactly
    /// those and no other subject's.
    pub(crate) registered_resource_ids: Vec<pill_engine::ResourceId>,
    /// Every component type name registered, plain and persistable alike, for
    /// the C# backend's bindings.
    pub(crate) component_names: Vec<String>,
}

/// What a failed first load of an artifact costs the world.
// Every variant answers the same question: what gets cleared. The shared
// `Clear` prefix states that action and is not noise. Without it the variants
// would read `World`, `SystemsOnly` and `GenerationData` at the call sites,
// which is harder to follow.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FirstLoadFailure {
    /// Replace the whole world ([`clear_failed_generation`]). This is the only
    /// safe answer for an artifact that may own data: its columns and resources
    /// carry drop functions from the image that is about to be unloaded.
    ClearWorld,
    /// Clear only the artifact's systems and keep the world.
    ///
    /// Safe only if the caller keeps two promises. First, the artifact
    /// registers no data of its own. The renderer module's components, assets
    /// and resources belong to `pill_renderer_api` and to the renderer data
    /// crate, and other subjects register them. Second, the failed image is
    /// never unloaded, so anything it did touch still points at loaded code.
    /// The renderer module first loads when the window opens, after the project
    /// has filled the world, so wiping the world there would lose the scene.
    ClearSystemsOnly,
    /// Clear what the failed generation registered and keep everything else.
    ///
    /// Used for an artifact that may register data but still must not cost the
    /// world. One case is the retry an editor offers long after the project has
    /// loaded, where the artifact is the editor's own shell module and that
    /// module registers components like any other. The failed generation's
    /// systems are removed, and the resources and component columns it
    /// registered in this attempt are dropped. Every value another subject
    /// registered stays. The failed image is kept loaded instead of unloaded,
    /// so whatever survived inside it - a drop function a peer still claims, a
    /// table that has not been re-homed yet - still points at loaded code.
    ClearGenerationData,
}

/// What to check on a freshly loaded replacement before it is committed.
pub(crate) enum LoadValidation {
    /// Nothing. This host builds the subject from its own workspace, so the
    /// contract always holds.
    None,
    /// The optional module ABI revision export. Any `cargo build` can rebuild
    /// an extension, so the revision is checked instead of assumed.
    ModuleAbi,
}

impl ReloadTransaction<'_> {
    /// Starts the new generation's registration scope when this subject uses
    /// one.
    fn begin_registration(&self, engine: &mut Engine) {
        if self.kind.scopes_registration() {
            engine.begin_module_registration(self.owner);
        }
    }

    /// Ends the registration scope again.
    fn end_registration(&self, engine: &mut Engine) {
        if self.kind.scopes_registration() {
            engine.end_module_registration();
        }
    }

    /// Drops the resources whose declared layout changed since the last load,
    /// so the module rebuilds them instead of keeping a value of the old shape.
    ///
    /// Only a resource that declared its layout is considered, because only
    /// then is there a fingerprint to compare. A resource that declared
    /// nothing keeps today's behaviour: it is kept whenever the new generation
    /// registers the type again.
    ///
    /// Each dropped resource is reported once, naming what changed and what was
    /// done. The person edited a file, and the edit having a consequence for
    /// live state is worth a line at warning level.
    fn drop_resources_whose_layout_changed(&self, engine: &mut Engine) {
        let recorded = crate::layout_record::read_record(&self.layout_record);
        let changes = crate::layout_record::changed_layouts(
            engine.world(),
            &self.layout_record,
            &recorded,
            self.registered_resource_ids,
        );
        if changes.is_empty() {
            return;
        }
        let changed_resource_ids: Vec<pill_engine::ResourceId> =
            changes.iter().map(|change| change.id).collect();
        // Same two calls the retirement path makes below: the claim this
        // generation holds has to go before its value can be dropped, and the
        // claim count is what keeps a value another subject still owns.
        engine
            .world_mut()
            .release_resource_claims(&changed_resource_ids);
        let dropped_resource_count = engine.world_mut().drop_resources(&changed_resource_ids);
        for change in &changes {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                resource = change.resource.as_str(),
                sources = %change.changed_sources.join(", "),
                dependencies = %change.changed_dependencies.join(", "),
                "resource layout changed; dropped the live value so the module rebuilds it"
            );
        }
        debug!(
            target: telemetry_target::HOT_RELOAD,
            module = self.subject,
            dropped = dropped_resource_count,
            declared = changes.len(),
            "dropped the resources whose declared layout changed"
        );
    }

    /// Moves a library into the graveyard. Releases the oldest generation when
    /// the limit is passed, but only once no surviving column needs its
    /// tables.
    ///
    /// A retired image is never unloaded the moment it stops being current.
    /// Pointers into it can still be live: storage tables, patch stubs and
    /// persist metadata. The eviction log line is what marks a later fault as
    /// coming from an image released here rather than from somewhere else.
    ///
    /// The `world` argument turns "a limit of two generations is safe" into a
    /// check. A column whose component id has no factory left still holds a
    /// function table from whichever image created it. So if
    /// [`World::columns_without_factory`] returns a non-zero count, the release
    /// waits and logs a warning instead of calling into freed memory.
    fn retire_library(&mut self, library: NativeLibrary, world: &World) {
        self.old_libraries.push(library);
        if self.graveyard_policy == GraveyardPolicy::KeepAll {
            debug!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                generations = self.old_libraries.len(),
                "keeping every retired generation mapped"
            );
            return;
        }
        if self.old_libraries.len() > MAX_GRAVEYARD_GENERATIONS {
            let orphaned_column_count = world.columns_without_factory();
            if orphaned_column_count != 0 {
                let deferral_count = Self::deferred_eviction_count(self.subject, true);
                // One deferral is normal. A column is mid-migration and the next
                // reload clears it. A run of deferrals is not. Nothing is
                // releasing those columns, so every later reload adds a loaded
                // image that can never be released.
                if deferral_count >= MAX_DEFERRED_EVICTIONS {
                    error!(
                        target: telemetry_target::HOT_RELOAD,
                        module = self.subject,
                        generations = self.old_libraries.len(),
                        columns = orphaned_column_count,
                        consecutive_deferrals = deferral_count,
                        "eviction has been deferred repeatedly: native columns still reference retired images and are not being released, so every reload now leaks a mapped image"
                    );
                } else {
                    warn!(
                        target: telemetry_target::HOT_RELOAD,
                        module = self.subject,
                        generations = self.old_libraries.len(),
                        columns = orphaned_column_count,
                        "deferring eviction: native columns still reference a retired image"
                    );
                }
                return;
            }
            // The check passed, so whatever held those columns is gone.
            Self::deferred_eviction_count(self.subject, false);
            // Dropping the evicted generation unloads its image and deletes its
            // temporary file on disk. The line is logged before the drop:
            // anything still holding a pointer into that image crashes inside
            // it, and this line is what tells that apart from a crash somewhere
            // else.
            info!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                generations = self.old_libraries.len(),
                "evicting the oldest retired generation"
            );
            drop(self.old_libraries.remove(0));
        }
    }

    /// Counts consecutive deferred evictions for one subject.
    ///
    /// When `should_defer` is true this adds one and returns the new count.
    /// When it is false it clears the count and returns zero. The count is kept
    /// per subject because the project and each extension retire on their own,
    /// and one stuck subject must not hide another's state or be hidden by it.
    fn deferred_eviction_count(subject: &str, should_defer: bool) -> u32 {
        use std::collections::HashMap;
        use std::sync::Mutex;
        static DEFERRED_EVICTIONS_BY_SUBJECT: Mutex<Option<HashMap<String, u32>>> =
            Mutex::new(None);
        let mut counts_guard = DEFERRED_EVICTIONS_BY_SUBJECT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let deferred_evictions_by_subject = counts_guard.get_or_insert_with(HashMap::new);
        if !should_defer {
            deferred_evictions_by_subject.remove(subject);
            return 0;
        }
        let deferral_count = deferred_evictions_by_subject
            .entry(subject.to_string())
            .or_insert(0);
        *deferral_count = deferral_count.saturating_add(1);
        *deferral_count
    }

    /// Runs the whole sequence against a replacement that is already loaded.
    ///
    /// The compile and load that come first live in [`build_load_and_commit`].
    /// The steps below count on from those.
    ///
    /// Returns `None` when the new generation failed to initialize and the
    /// previous one was restored. In that case nothing was swapped and the
    /// caller keeps running what it had.
    pub(crate) fn commit(
        mut self,
        engine: &mut Engine,
        engine_api: &EngineApi,
        new_library: NativeLibrary,
    ) -> Option<ReloadCommit> {
        // Step 3: Save the retiring generation's persistable metadata before
        // its registrations are replaced. The new init below registers the same
        // type names again, so without this copy the old serializer pointers
        // would be gone before migration could use them. The previous DLL stays
        // loaded (Step 12), which keeps those pointers valid.
        let previous_persist_metadata_by_name = engine.world().capture_persist_type_metadata();
        let previous_persist_type_manifest = engine.world().persist_type_manifest();
        // The resource half of the same copy, needed for the same reason. A
        // resource's serializer is generated for the shape the retiring build
        // declared, and it is the only code that can read the stored value once
        // the new build changes that shape.
        let previous_resource_manifest = engine.world().persist_resource_manifest();

        // Step 4: Swap the systems. Only this subject's systems are removed, so
        // everything else in the scheduler keeps running: the project and the
        // other modules when a module reloads, the modules when the project
        // reloads.
        let removed_system_count = engine.clear_systems_owned_by(self.owner);
        debug!(
            target: telemetry_target::HOT_RELOAD,
            module = self.subject,
            removed_systems = removed_system_count,
            "cleared the retiring generation's systems"
        );

        let init_started = Instant::now();
        // Take the registration sequence before init, so the types this
        // generation registers can be compared with the previous set.
        let persist_registration_sequence = engine.world().persist_registration_sequence();
        let component_registration_sequence = engine.world().component_registration_sequence();
        let resource_registration_sequence = engine.world().resource_registration_sequence();
        // Entities alive before the new generation's init. Migration converts
        // only these. Anything init spawns already has the new schema, and the
        // retiring serializer would read it wrong while a same-layout column is
        // rebuilt.
        let pre_swap_entities = engine.world().capture_live_entities();
        // Shared components registered before this transaction. Several
        // subjects hold the same shared id, so a failed generation that
        // registers one again did not create it, and the rollback below must
        // not drop it.
        let shared_component_ids_before: HashSet<ComponentId> = engine
            .world()
            .registered_component_ids_since(0)
            .into_iter()
            .filter(|id| matches!(id, ComponentId::Shared(_)))
            .collect();
        // Step 5: Announce the retiring generation's persistable names. A
        // rebuilt image has a new `TypeId` for every type name it declares. At
        // the registration site that looks exactly like another binary claiming
        // the same name, and the world's collision check refuses that. Only the
        // host knows these names belonged to the generation being replaced, so
        // a reload that does not say so fails its own init and rolls back.
        let retiring_type_names = self.registered_type_names.clone();
        // Which of those types the retiring generation was already stale for.
        // Only a subject with the current layout of a shared component may
        // change it, so a stale generation's registrations in the old layout
        // are skipped (see [`stale_components`]). Taken once, before either
        // init runs.
        let retiring_stale_type_names = stale_components(engine.world(), self.registered_schemas);
        engine
            .world_mut()
            .supersede_persist_registrations(&retiring_type_names);
        engine
            .world_mut()
            .mark_stale_superseded_persist_registrations(&retiring_stale_type_names);
        // Step 4b: Drop every declared resource whose layout changed since the
        // record was written. It has to happen before the new init: a module
        // rebuilds a missing resource by inserting it, so a drop after the init
        // would remove the fresh value instead. It also has to happen while the
        // retiring image is mapped, because the value's drop function lives in
        // it.
        self.drop_resources_whose_layout_changed(engine);
        self.begin_registration(engine);
        // Step 4c: A later generation re-registers and does not re-initialize.
        // The artifact's registration half re-registers the systems and
        // resources whose code moved with the new image; its one-time `init`
        // ran on the first load and must not seed this world twice. An artifact
        // built before the split exports no registration half, so it keeps the
        // one entry point it always had - said on every such reload, because a
        // reload that re-initializes silently is the thing this stage exists to
        // end.
        let status = match new_library.call_register(engine_api) {
            Some(status) => {
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    module = self.subject,
                    "this artifact exports a registration entry point; its one-time init does not run again"
                );
                status
            }
            None => {
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    module = self.subject,
                    "this artifact has no registration entry point; its one-time init runs again on this reload"
                );
                new_library.call_init(engine_api)
            }
        };
        self.end_registration(engine);
        engine.world_mut().clear_superseded_persist_registrations();
        analytics::record_init(self.subject, init_started.elapsed().as_secs_f64() * 1000.0);
        if status != 0 {
            // The replacement failed to register. Roll back to the previous
            // generation: init must be idempotent, so running it again restores
            // the systems that were just cleared. No migration runs on this
            // path because the old registrations are still intact.
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                status,
                "new generation failed to initialize; rolling back"
            );
            engine.clear_systems_owned_by(self.owner);

            // What the failed generation managed to register before giving up.
            // `clear_systems_owned_by` removes systems and their dispatch slots
            // but leaves component registrations. A component's storage factory
            // holds function pointers into the image that registered it, which
            // this branch retires at its end. Ids, not names: the rollback
            // generation registers the same names again under new `TypeId`s, so
            // only the ids separate its entries from the failed generation's.
            let failed_component_ids = engine
                .world()
                .registered_component_ids_since(component_registration_sequence);
            // The resource half of the same copy. It holds the ids the failed
            // generation claimed, to compare with what the rollback generation
            // claims again below.
            let failed_resource_ids = engine
                .world()
                .resource_ids_registered_since(resource_registration_sequence);

            let rollback_component_registration_sequence =
                engine.world().component_registration_sequence();
            let rollback_resource_registration_sequence =
                engine.world().resource_registration_sequence();
            // Announced again. The failed generation used up the marks for the
            // names it managed to register, and the rollback init registers all
            // of them again.
            engine
                .world_mut()
                .supersede_persist_registrations(&retiring_type_names);
            engine
                .world_mut()
                .mark_stale_superseded_persist_registrations(&retiring_stale_type_names);
            self.begin_registration(engine);
            let rollback_status = self.current.call_init(engine_api);
            self.end_registration(engine);
            engine.world_mut().clear_superseded_persist_registrations();
            if rollback_status != 0 {
                error!(
                    target: telemetry_target::HOT_RELOAD,
                    module = self.subject,
                    status = rollback_status,
                    "rollback also failed; this module now contributes no systems"
                );
            }

            // Anything the rollback registered again is safe. Its new id holds
            // new factories that point into the generation that is still
            // loaded. What is left is an id only the failed generation
            // registered, whose factory keeps pointing into the image being
            // retired. Its rows go too. The generated wrapper checks the
            // registration-error slot after the user's `init`, so a failing
            // init can already have spawned entities with that type. Those rows
            // would be dropped through an unloaded image at the next despawn.
            let rollback_component_ids = engine
                .world()
                .registered_component_ids_since(rollback_component_registration_sequence);
            // A shared id that existed before this transaction is not the
            // failed generation's to drop. That holds even when the rollback
            // skipped it, which happens for a stale subject that keeps the
            // registered layout (see [`stale_components`]). Dropping it would
            // remove the rows and the registration of every subject that shares
            // it. Its storage factory and persist entries may now point into
            // the failed image, which was built against the registered layout.
            // That image stays loaded in the graveyard, and the next
            // registration of the component by a current subject points them at
            // its own code.
            let stranded_component_ids: Vec<ComponentId> = failed_component_ids
                .into_iter()
                .filter(|id| {
                    !rollback_component_ids.contains(id)
                        && !shared_component_ids_before.contains(id)
                })
                .collect();
            if !stranded_component_ids.is_empty() {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    module = self.subject,
                    count = stranded_component_ids.len(),
                    ids = %stranded_component_ids
                        .iter()
                        .map(|id| format!("{id:?}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    "dropping registrations from the failed generation; their storage factories point into the image being retired"
                );
                let dropped_entity_count = engine
                    .world_mut()
                    .retire_component_storage(&stranded_component_ids);
                debug!(
                    target: telemetry_target::HOT_RELOAD,
                    dropped = dropped_entity_count, "removed rows that belonged to the failed generation"
                );
            }
            // The purge above should leave nothing listing a stranded id. The
            // assert checks that, so a future change that misses a path fails
            // here in a debug build instead of crashing inside a retired image.
            debug_assert!(
                stranded_component_ids
                    .iter()
                    .all(|id| !engine.world().any_archetype_lists_component(*id)),
                "a stranded component id still has an archetype entry after the purge"
            );

            // Resources need a stronger version of the same treatment. A
            // component column's factory is re-pointed when the type is
            // registered again, but a stored resource *value* carries its own
            // drop function. So a resource the failed generation inserted under
            // an id the rollback generation does not claim must be dropped now,
            // while the failing image is still loaded. Leaving it for the
            // graveyard would leave a drop function that points at unloaded
            // code.
            let rollback_resource_ids = engine
                .world()
                .resource_ids_registered_since(rollback_resource_registration_sequence);
            let stranded_resource_ids: Vec<pill_engine::ResourceId> = failed_resource_ids
                .into_iter()
                .filter(|id| !rollback_resource_ids.contains(id))
                .collect();
            if !stranded_resource_ids.is_empty() {
                let dropped_resource_count =
                    engine.world_mut().drop_resources(&stranded_resource_ids);
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    module = self.subject,
                    dropped = dropped_resource_count,
                    "dropped resources claimed only by the failed generation"
                );
            }
            // Whatever is left still has a drop function from whichever
            // generation supplied its table. Point the tables at the rollback
            // image, the same way the success path does. Every id the failed
            // generation touched was either claimed again (the rollback init
            // refreshed its table) or dropped just above, so no table still
            // points into the failing image.
            engine.world_mut().rehome_resources();
            // Asset columns follow the same rule. The rollback init declares the
            // asset types its generation owns again, so their tables point back
            // at the image that stays loaded. A type only the failed generation
            // declared has no owner to claim it again, because the asset store
            // keeps no per-subject claims, so it keeps that generation's table.
            engine.world_mut().rehome_assets();
            // Retire the failed image instead of unloading it. The purge above is
            // meant to leave nothing pointing into it, but that is not a proof.
            // A kept image costs one `FreeLibrary` later; an unloaded one
            // crashes if any pointer survived. With the bounded policy the
            // graveyard eventually releases it, and with
            // [`GraveyardPolicy::KeepAll`] it stays loaded for good.
            self.retire_library(new_library, engine.world());
            return None;
        }

        // Find persistable component types the new generation stopped
        // registering. Migration does not wipe that data. The type is not in
        // the changed-name set, so its column and metadata stay while the new
        // generation cannot read them. Report it instead of letting the type
        // become silently orphaned.
        let newly_registered_type_names = engine
            .world()
            .persist_type_names_registered_since(persist_registration_sequence);
        // Every component name this generation registered, plain or
        // persistable. The window starts at the sequence taken before init, so
        // unlike a `since(0)` query it counts this generation's declarations
        // and not every artifact's history.
        let registered_component_names_now = engine
            .world()
            .registered_component_names_since(component_registration_sequence);
        let forgotten_type_names: Vec<String> = self
            .registered_type_names
            .iter()
            .filter(|name| {
                !newly_registered_type_names
                    .iter()
                    .any(|current| current == *name)
            })
            .cloned()
            .collect();
        if !forgotten_type_names.is_empty() {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                forgotten_types = %forgotten_type_names.join(", "),
                "{}", self.kind.forgotten_types_warning()
            );

            // Drop the orphaned columns only for types nothing declares any
            // more. This generation may still declare the type as a plain
            // component, in which case its data is live. Another artifact may
            // still register it, in which case the type is the peer's to keep.
            // This runs while the generation that last registered the type is
            // still loaded, so the drop is safe.
            let unclaimed_component_type_names = unclaimed_type_names(
                &forgotten_type_names,
                &registered_component_names_now,
                self.peer_component_names,
            );
            if !unclaimed_component_type_names.is_empty() {
                let dropped_forgotten_entity_count = engine
                    .world_mut()
                    .drop_forgotten_components(&unclaimed_component_type_names);
                debug!(
                    target: telemetry_target::HOT_RELOAD,
                    module = self.subject,
                    dropped_entities = dropped_forgotten_entity_count,
                    "dropped orphaned columns for component types no longer registered"
                );
            }
        }
        *self.registered_schemas = registered_schemas(engine.world(), &newly_registered_type_names);
        *self.registered_type_names = newly_registered_type_names;

        // The resource version of the block above. A resource holds a drop
        // function from the artifact that inserted it, and nothing clears
        // resources on reload. So a type this subject no longer registers must
        // be dropped here, while the retiring image is still loaded, instead
        // of being left for the graveyard to invalidate.
        let claimed_resource_ids_now = engine
            .world()
            .resource_ids_registered_since(resource_registration_sequence);
        let retired_resource_ids: Vec<pill_engine::ResourceId> = self
            .registered_resource_ids
            .iter()
            .filter(|id| !claimed_resource_ids_now.contains(id))
            .copied()
            .collect();
        // The claim list is the difference between generations, so the world's
        // claim counts move by the same difference. Ids this generation newly
        // claims gain one claim, and the retired ids lose theirs.
        // `drop_resources` then skips whatever another subject still claims. A
        // shared resource the project stopped registering must survive for a
        // module that did not stop.
        let newly_claimed_resource_ids: Vec<pill_engine::ResourceId> = claimed_resource_ids_now
            .iter()
            .filter(|id| !self.registered_resource_ids.contains(id))
            .copied()
            .collect();
        engine
            .world_mut()
            .retain_resource_claims(&newly_claimed_resource_ids);
        if !retired_resource_ids.is_empty() {
            engine
                .world_mut()
                .release_resource_claims(&retired_resource_ids);
            let dropped_resource_count = engine.world_mut().drop_resources(&retired_resource_ids);
            debug!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                dropped = dropped_resource_count,
                "dropped resources for types this generation no longer registers"
            );
        }
        *self.registered_resource_ids = claimed_resource_ids_now;
        // Step 5b: Record what the generation that just loaded said its
        // resources' layouts are made of, so the next load can tell a kept value
        // from one that has to be rebuilt. This runs only on the success path:
        // a rolled-back generation never describes the world.
        crate::layout_record::write_record(
            engine.world(),
            &self.layout_record,
            self.registered_resource_ids,
        );

        // Step 6: Point every native storage column at the new generation's
        // function table. Columns created by older generations hold function
        // pointers into their own DLL. Refreshing them here, while the old DLLs
        // are still loaded, keeps drops and upcasts valid after those DLLs are
        // released from the graveyard.
        engine.world_mut().rehome_native_columns();

        // Step 7: The same for resources. A resource holds a drop function from
        // the artifact that inserted it, and nothing clears resources on
        // reload. Without this step the graveyard could release the retiring
        // image while a live resource still had its destructor there.
        engine.world_mut().rehome_resources();

        // Step 8: And for asset columns. The asset store also outlives the
        // reload, and a column's per-type table (drop, upcast, take) is code
        // from whichever artifact declared or first used the type. Pointing it
        // at the new generation keeps the store usable after that artifact's
        // image leaves the graveyard.
        engine.world_mut().rehome_assets();

        // Step 9: Migrate persistable schemas that changed in the swap. Types
        // are matched by stable name, not by runtime `ComponentId`, which
        // differs between generations. A reshaped type keeps its rows and its
        // data follows the new layout. Unchanged columns keep their allocations
        // and change-detection ticks, which keeps the common reload cheap.
        let migrate_started = Instant::now();
        let current_schema_hashes_by_type_name: HashMap<String, u64> = engine
            .world()
            .persist_type_manifest()
            .into_iter()
            .map(|entry| (entry.type_name, entry.schema_hash))
            .collect();
        let changed_type_names: HashSet<String> = previous_persist_type_manifest
            .iter()
            .filter_map(|entry| {
                current_schema_hashes_by_type_name
                    .get(&entry.type_name)
                    .filter(|&&current_hash| current_hash != entry.schema_hash)
                    .map(|_| entry.type_name.clone())
            })
            .collect();

        if changed_type_names.is_empty() {
            // Avoid touching archetype storage when every persisted layout is
            // byte-for-byte identical to the previous generation.
            info!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                "schema unchanged for all persistable component types - fast path"
            );
        } else {
            debug!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                changed_types = changed_type_names.len(),
                "migrating changed persistable module types"
            );
            let migration_report = engine.world_mut().migrate_changed_persistable_components(
                &previous_persist_metadata_by_name,
                &changed_type_names,
                Some(&pre_swap_entities),
            );
            debug!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                migrated_types = migration_report.migrated_type_count,
                migrated_entities = migration_report.migrated_entity_count,
                "persistable migration complete"
            );
            if !migration_report.skipped_types.is_empty() {
                warn!(
                    target: telemetry_target::HOT_RELOAD,
                    module = self.subject,
                    skipped_types = %migration_report
                        .skipped_types
                        .iter()
                        .map(|skipped| format!("{} ({})", skipped.type_name, skipped.reason))
                        .collect::<Vec<_>>()
                        .join("; "),
                    "migration skipped some module component types"
                );
            }
        }
        // Step 10: Migrate reshaped resources. `rehome_resources` above swapped
        // each live resource's function table without touching its bytes. A
        // resource whose fields changed would then be read as the new type over
        // the old value's memory. Resources are matched by persistence name, like
        // components. This runs while the retiring image is still loaded because
        // the old serializer lives in it.
        let migrated_resources = engine
            .world_mut()
            .migrate_changed_persistable_resources(&previous_resource_manifest);
        if !migrated_resources.is_empty() {
            debug!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                migrated_resources = %migrated_resources.join(", "),
                "reshaped resources migrated"
            );
        }

        analytics::record_migrate(
            self.subject,
            migrate_started.elapsed().as_secs_f64() * 1000.0,
        );

        // Step 11: Check that every surviving native column still owns a loaded
        // function table before the retiring image can be released. A column
        // whose id has no factory is the "same name, new TypeId" case that the
        // name-keyed forgotten-type sweep cannot reach, because the name
        // resolves to the new id. It is dropped here, through the table of the
        // generation that produced it, while that generation is still loaded.
        let orphaned_column_count = engine.world_mut().drop_columns_without_factory();
        if orphaned_column_count != 0 {
            debug!(
                target: telemetry_target::HOT_RELOAD,
                module = self.subject,
                orphaned_columns = orphaned_column_count,
                "dropped native columns whose registration is gone"
            );
        }

        // Step 12: Retire the previous library without unloading it. The engine
        // may still hold pointers to component operations and persist metadata
        // that generation registered.
        let previous_library = std::mem::replace(&mut *self.current, new_library);
        self.retire_library(previous_library, engine.world());

        analytics::record_reload(self.subject);

        info!(
            target: telemetry_target::HOT_RELOAD,
            module = self.subject,
            entities = engine.world().entity_count(),
            graveyard = self.old_libraries.len(),
            "{}", self.kind.completion_message()
        );

        Some(ReloadCommit {
            exposed_component_names: registered_component_names_now,
        })
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Captures, initializes and records one native generation.
///
/// This is the part a subject's `start` shares with every other subject. The
/// project and an extension run the same sequence around their entry point.
/// Take the registration sequences, scope the registration when the subject
/// uses a scope, and call the entry point with a timer around it. Then either
/// record what the generation registered, or, on a non-zero status, release
/// what the failed image owns while it is still loaded. `first_load_failure`
/// decides how much of the world that release clears.
///
/// `scope` is the owner registrations are tagged with, or `None` for a subject
/// that owns the scheduler. `clearing_owner` is the owner whose systems a
/// failed init removes; it is the subject's own owner in both cases.
pub(crate) fn initialize_generation(
    engine: &mut Engine,
    engine_api: &EngineApi,
    subject: &str,
    scope: Option<SystemOwner>,
    clearing_owner: SystemOwner,
    library: &NativeLibrary,
    first_load_failure: FirstLoadFailure,
) -> GenerationInit {
    // Step 1: Take the sequences before the call. Then `since` compares this
    // generation with the state just before it ran, not with zero. The log
    // collects entries from every artifact, so `since(0)` would count the
    // engine's own resources as this subject's.
    let persist_registration_sequence = engine.world().persist_registration_sequence();
    let component_registration_sequence = engine.world().component_registration_sequence();
    let resource_registration_sequence = engine.world().resource_registration_sequence();
    // What the world held before this generation ran. Only the policy that must
    // tell this generation's registrations from a name another subject already
    // registered needs it.
    let component_names_registered_before = (first_load_failure
        == FirstLoadFailure::ClearGenerationData)
        .then(|| engine.world().registered_component_names_since(0));

    // Step 2: Call the entry point, inside the subject's registration scope
    // when it has one.
    let status = {
        let init_started = Instant::now();
        // This is the same scoped call a statically linked build makes. Only
        // the entry point differs: a DLL export here, a linked function there.
        let status = pill_player::registration::register_module(engine, scope, |_| {
            library.call_init(engine_api)
        });
        analytics::record_init(subject, init_started.elapsed().as_secs_f64() * 1000.0);
        status
    };

    if status != 0 {
        match first_load_failure {
            FirstLoadFailure::ClearWorld => {
                clear_failed_generation(engine, subject, clearing_owner)
            }
            FirstLoadFailure::ClearSystemsOnly => {
                let removed_system_count = engine.clear_systems_owned_by(clearing_owner);
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    module = subject,
                    removed_systems = removed_system_count,
                    "cleared the failed generation's systems and kept the world"
                );
            }
            FirstLoadFailure::ClearGenerationData => {
                // Systems first, then the registrations this generation made.
                // All of it runs while the image is still loaded, because a
                // resource's drop function and a column's drop glue belong to
                // the image that wrote them.
                let removed_system_count = engine.clear_systems_owned_by(clearing_owner);
                let registered_component_names = engine
                    .world()
                    .registered_component_names_since(component_registration_sequence);
                let owned_components: Vec<String> = match &component_names_registered_before {
                    // A name another subject already registered is not this
                    // generation's to retire. Dropping it would erase the peer's
                    // data and its registration.
                    Some(before) => registered_component_names
                        .into_iter()
                        .filter(|name| !before.contains(name))
                        .collect(),
                    None => registered_component_names,
                };
                let owned_resource_ids = engine
                    .world()
                    .resource_ids_registered_since(resource_registration_sequence);
                let dropped_resource_count = engine.world_mut().drop_resources(&owned_resource_ids);
                let dropped_row_count = engine
                    .world_mut()
                    .drop_forgotten_components(&owned_components);
                info!(
                    target: telemetry_target::HOT_RELOAD,
                    module = subject,
                    removed_systems = removed_system_count,
                    dropped_resources = dropped_resource_count,
                    dropped_components = owned_components.len(),
                    dropped_rows = dropped_row_count,
                    "cleared the failed generation's own data and kept the world"
                );
            }
        }
        return GenerationInit {
            status,
            registered_type_names: Vec::new(),
            registered_schemas: Vec::new(),
            registered_resource_ids: Vec::new(),
            component_names: Vec::new(),
        };
    }

    // Step 3: What this generation owns, also claimed in the world so one
    // subject's retirement cannot take a value another subject shares.
    let registered_resource_ids = engine
        .world()
        .resource_ids_registered_since(resource_registration_sequence);
    engine
        .world_mut()
        .retain_resource_claims(&registered_resource_ids);
    let registered_type_names = engine
        .world()
        .persist_type_names_registered_since(persist_registration_sequence);
    GenerationInit {
        status,
        registered_schemas: registered_schemas(engine.world(), &registered_type_names),
        registered_type_names,
        registered_resource_ids,
        component_names: engine
            .world()
            .registered_component_names_since(component_registration_sequence),
    }
}

/// Returns the schema hash the world records for each name in `type_names`.
///
/// Collected in the same transaction as the `init` that registered them, so a
/// hash that differs later means another subject's reload changed that type's
/// layout in between.
fn registered_schemas(world: &World, type_names: &[String]) -> Vec<(String, u64)> {
    // A generation can register one type twice. The component inventory and an
    // explicit `register` call both do, and the log records both. The first
    // entry wins.
    let mut schemas: Vec<(String, u64)> = Vec::new();
    for name in type_names {
        if schemas.iter().any(|(known, _)| known == name) {
            continue;
        }
        if let Some(hash) = world.persist_schema_hash(name) {
            schemas.push((name.clone(), hash));
        }
    }
    schemas
}

/// Returns the persistable components a subject registered whose layout has
/// since been replaced by another subject's reload.
///
/// Every binary that links a shared component registers it, and a reload of one
/// of them may change its layout. The registry accepts the new layout from a
/// superseding registration, and the reload migrates the rows. Every other
/// binary that registered the type is still built against the old layout. Its
/// queries are refused by the declared-schema check, and anything else its
/// systems do with the type, such as spawning a value, would write rows in the
/// old layout. An empty result means the subject has the current layout.
pub(crate) fn stale_components(world: &World, registered_schemas: &[(String, u64)]) -> Vec<String> {
    registered_schemas
        .iter()
        .filter(|(name, declared)| {
            world
                .persist_schema_hash(name)
                .is_some_and(|current| current != *declared)
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Releases everything one failed generation owns, while its image is loaded.
///
/// The failed image's systems are `Box<dyn System>` trait objects, and its data
/// carries drop functions from the same image: the resources it inserted and
/// the columns its entities live in. Systems go first, then the world is
/// replaced. Dropping the old world runs all of those functions, and both
/// steps must finish before the image can be unloaded.
fn clear_failed_generation(engine: &mut Engine, subject: &str, system_owner: SystemOwner) {
    engine.clear_systems_owned_by(system_owner);
    let abandoned_world = std::mem::replace(engine.world_mut(), World::new());
    let resource_count = abandoned_world.resource_count();
    drop(abandoned_world);
    info!(
        target: telemetry_target::HOT_RELOAD,
        module = subject,
        resources = resource_count,
        "cleared the world before unmapping the generation that failed to initialize"
    );
}

/// Builds, loads and commits one native generation, for either subject.
///
/// Both subjects run the same three steps in the same order, and a failure
/// anywhere leaves the current generation running: compile (`build`), load a
/// private copy, then swap through [`ReloadTransaction`]. Their differences are
/// parameters, not a second lifecycle: which build function to call, whether
/// the ABI check applies, and the refusal text the suites wait for.
///
/// `before_commit` runs once the replacement has built, loaded and passed
/// validation, just before the swap. It does not run when an earlier step
/// refused the replacement. A subject uses it to release state that must be
/// dropped while the retiring image is still loaded: the renderer module
/// detaches its GPU backend there.
///
/// Returns the commit when the swap happened, and `None` when anything was
/// refused.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_load_and_commit(
    engine: &mut Engine,
    engine_api: &EngineApi,
    workspace_root: &Path,
    build: impl FnOnce(Option<(&AtomicU64, u64)>) -> Result<PathBuf, BuildError>,
    cancel_flag: Option<(&AtomicU64, u64)>,
    validation: LoadValidation,
    before_commit: &mut dyn FnMut(),
    transaction: ReloadTransaction<'_>,
) -> Option<ReloadCommit> {
    let subject = transaction.subject;
    let kind = &transaction.kind;

    // Step 1: Compile before touching engine state, so a compiler error can
    // never remove the systems of the working generation. A newer save during
    // the build cancels it, and the next frame retries.
    let output_path = match build(cancel_flag) {
        Ok(path) => path,
        Err(error) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = subject,
                error = %error,
                "{}", kind.build_failed_message()
            );
            // The editor shows the compiler's own text: whatever the build
            // runner recorded for this failure.
            notices::push(HostNotice::ModuleFailed {
                module: subject.to_string(),
                output: crate::build_runner::recent_build_output(),
            });
            return None;
        }
    };

    // Step 2: Load and check the replacement without changing anything, so the
    // active generation stays untouched until it is ready to initialize.
    let new_library = match NativeLibrary::load_copy(&output_path, workspace_root, subject) {
        Ok(library) => library,
        Err(error) => {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = subject,
                error = %error,
                "{}", kind.load_failed_message()
            );
            // No compiler text belongs here. The build before this failure
            // succeeded, so the recorded output holds only its successful tail.
            notices::push(HostNotice::ModuleFailed {
                module: subject.to_string(),
                output: format!("{}: {error}", kind.load_failed_message()),
            });
            return None;
        }
    };
    if let LoadValidation::ModuleAbi = validation {
        if let Err(error) = new_library.check_module_abi(subject) {
            warn!(
                target: telemetry_target::HOT_RELOAD,
                module = subject,
                error = %error,
                "{}", kind.rejected_message()
            );
            notices::push(HostNotice::ModuleFailed {
                module: subject.to_string(),
                output: format!("{}: {error}", kind.rejected_message()),
            });
            return None;
        }
    }

    // Step 3: Hand off to the shared transaction. Its own steps pick up at
    // Step 3, and its order is load-bearing.
    before_commit();
    transaction.commit(engine, engine_api, new_library)
}

/// Returns the names in `forgotten_type_names` that nothing declares any more.
///
/// `forgotten_type_names` holds persistable names the previous generation
/// registered and the new one did not register again. A name in
/// `registered_component_names` was registered again as a plain component, so
/// its data is live and its columns must survive. A name in
/// `peer_component_names` belongs to another artifact that still declares it,
/// usually a shared identity the project registers as well as a module.
/// Dropping it here would erase that peer's rows.
fn unclaimed_type_names(
    forgotten_type_names: &[String],
    registered_component_names: &[String],
    peer_component_names: &[String],
) -> Vec<String> {
    forgotten_type_names
        .iter()
        .filter(|name| {
            !registered_component_names
                .iter()
                .any(|current| current == *name)
        })
        .filter(|name| !peer_component_names.iter().any(|peer| peer == *name))
        .cloned()
        .collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    // External crates
    use pill_engine::World;

    // Current crate
    use super::{registered_schemas, stale_components, unclaimed_type_names};

    /// A subject is stale exactly while the hash it registered differs from
    /// the one recorded now. A name the world no longer records is not
    /// reported.
    #[test]
    fn a_subject_is_stale_only_while_its_registered_layout_is_replaced() {
        let mut world = World::new();
        pill_engine::register_transform_component(&mut world);
        let name = pill_engine::common_components::TRANSFORM_SHARED_NAME.to_owned();

        let registered_schema_hashes = registered_schemas(&world, std::slice::from_ref(&name));
        assert_eq!(
            registered_schema_hashes.len(),
            1,
            "the registered hash is captured"
        );
        assert!(stale_components(&world, &registered_schema_hashes).is_empty());

        let built_against_previous = vec![(name.clone(), registered_schema_hashes[0].1 ^ 1)];
        assert_eq!(
            stale_components(&world, &built_against_previous),
            vec![name]
        );

        let unrecorded_schema_hashes = vec![("no::such::Component".to_owned(), 7)];
        assert!(stale_components(&world, &unrecorded_schema_hashes).is_empty());
    }

    /// A forgotten name counts as claimed when the new generation registered
    /// it as anything, and when any other subject still declares it. Only a
    /// name that nothing claims is returned.
    #[test]
    fn a_forgotten_type_is_unclaimed_only_when_nothing_declares_it() {
        let forgotten_type_names = vec![
            "dropped::Nowhere".to_owned(),
            "downgraded::Plain".to_owned(),
            "shared::WithPeer".to_owned(),
        ];
        let registered_component_names = vec!["downgraded::Plain".to_owned()];
        let peer_component_names = vec!["shared::WithPeer".to_owned()];

        assert_eq!(
            unclaimed_type_names(
                &forgotten_type_names,
                &registered_component_names,
                &peer_component_names
            ),
            vec!["dropped::Nowhere".to_owned()]
        );
        assert!(unclaimed_type_names(&forgotten_type_names, &[], &[]).len() == 3);
        assert!(
            unclaimed_type_names(&[], &registered_component_names, &peer_component_names)
                .is_empty()
        );
    }
}
