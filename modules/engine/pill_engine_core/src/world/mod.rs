//! The world: the engine's central store of entities, archetypes, components
//! and resources.
//!
//! # Responsibilities
//!
//! - Declares the [`World`] struct: every map and counter the engine's state
//!   consists of, and the one place they are constructed.
//! - Creates and destroys entities, and hands their ids out again through a
//!   free list.
//! - Owns every archetype, including the migration that moves one entity's row
//!   from one archetype to another.
//!
//! # Design
//!
//! A component type has to be registered before it can be used, because the
//! world gives it a bit index that archetype masks are matched with. Destroying
//! an entity puts its id back on a free list with a new generation, so an old
//! handle cannot reach the slot's next occupant.
//!
//! `World` is the API most of the engine talks to, and it has several jobs:
//! script dispatch, resource claims and field reflection. Each job has a file
//! of its own below, and each of those files adds its own `impl World`. That
//! split exposes nothing new, because a child module's `impl World` is the same
//! implementation and can reach the struct's private fields.

// Standard library
use std::collections::HashMap;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_core::{debug, error, info, warn};

// Current crate
#[cfg(feature = "development")]
use crate::archetype::FieldPlan;
use crate::archetype::{
    validate_component_layout, Archetype, ArchetypeId, Blittability, ColumnLayout,
    ComponentColumns, StorageFactory,
};
use crate::commands::CommandQueue;
use crate::component::{
    Component, ComponentId, ComponentMask, ComponentRegistry, ComponentTicks, Tick,
};
use crate::entity::{Entity, EntityId};
use crate::query::change_detection::Mut;
#[cfg(feature = "development")]
use crate::resource::KeptLayoutDeclaration;
use crate::resource::{ErasedResource, ErasedResourceOps, Resource, ResourceId};
use crate::scripting::{ScriptComponent, ScriptContext};

// =============================================================================
// Module Tree
// =============================================================================
//
// Each job has its own file; the module header above explains the split.

mod components;
mod reflection;
mod resources;
mod scripting;
mod ticks;

pub use components::ColumnChunk;

// =============================================================================
// Re-exports
// =============================================================================

pub use crate::error::{AddComponentError, BuildError, RemoveComponentError, WorldError};

// =============================================================================
// Registration headroom
// =============================================================================

/// Free component types left below the 128-type ceiling at which registration
/// starts to warn.
///
/// The ceiling is shared by a project and all of its extensions, so a project
/// with few types of its own can still reach it. The warning fires while there
/// is still room to react, so the limit shows up before registration starts to
/// fail.
const REGISTRATION_HEADROOM_WARNING_THRESHOLD: usize = 16;

// =============================================================================
// EntityLocation
// =============================================================================

/// Where one entity is stored: its archetype and its row in that archetype.
///
/// Maps an [`Entity`] handle to the [`Archetype`] that owns its component
/// columns, plus the row in that archetype. The world updates it on every
/// creation, migration and destruction, so a lookup by handle stays O(1).
#[derive(Clone, Copy)]
pub(crate) struct EntityLocation {
    /// The archetype that currently stores the entity's components.
    pub(crate) archetype_id: ArchetypeId,
    /// Row of the entity in the archetype, shared by all of its columns.
    pub(crate) index_in_archetype: usize,
}

// =============================================================================
// IteratorTimings
// =============================================================================

/// Timing data for parallel iterators, grouped by label and shared between
/// them.
///
/// Every label is an owned `String`. A label is usually a string literal from
/// the module whose system ran the iterator, and the world outlives that
/// module's DLL. A borrowed `&'static str` stored here would point into memory
/// that hot reload unmaps, and the next comparison against it would read
/// memory that is no longer mapped.
pub(crate) struct IteratorTimings {
    /// Average time in nanoseconds each label's iterator run took, over about
    /// the last 32 frames. Parallel iterators use it as their splitting hint.
    pub per_iterator_label_average_duration: std::collections::HashMap<String, u64>,
    /// Labels visited in the current frame. Cleared each frame.
    pub visited_iterator_labels: Vec<String>,
    /// Labels that appeared more than once in the current frame.
    pub visited_duplicated_iterator_labels: Vec<String>,
}

impl IteratorTimings {
    /// Creates an empty [`IteratorTimings`] with no labels recorded.
    pub fn new() -> Self {
        Self {
            per_iterator_label_average_duration: std::collections::HashMap::new(),
            visited_iterator_labels: Vec::new(),
            visited_duplicated_iterator_labels: Vec::new(),
        }
    }
}

/// The declaration that claimed a shared resource name.
///
/// The world records one of these for each shared [`ResourceId`], so it can
/// compare a second claim with the first. The same type arriving twice is the
/// case shared resources exist for; a different type claiming the name is a
/// collision.
#[derive(Debug, Clone)]
pub struct SharedResourceClaim {
    /// The shared name the claiming type declared.
    pub shared_name: String,
    /// The name of the type that recorded the declaration.
    ///
    /// For a Rust type this is the last segment of its path; for another
    /// language it is the full managed name.
    pub declaring_type: String,
    /// Whether the recorded declaration came from another language.
    ///
    /// A declaration from another language has no Rust type name, so the world
    /// compares names only between two Rust declarations - see
    /// [`World::claim_shared_resource`].
    pub foreign: bool,
    /// Size in bytes the claiming type declared.
    pub size: usize,
    /// Alignment in bytes the claiming type declared.
    pub align: usize,
    /// A structural hash of the claiming type's layout, when it has one.
    ///
    /// A declaration from another language brings the hash in its manifest. A
    /// Rust type gets whatever [`Resource::shared_schema_hash`] returns, which
    /// defaults to `None`. The world compares the two hashes only when both
    /// sides have one, so the check adds evidence instead of a new
    /// requirement.
    pub schema_hash: Option<u64>,
}

// =============================================================================
// World
// =============================================================================

/// One live entity and the type names of the components attached to it.
///
/// [`World::entity_rows`] returns these for the editor's Hierarchy list. The
/// type names and the [`Entity`] handle stay the same across hot reloads, which
/// makes the handle a stable selection identity.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityRow {
    /// The live entity handle, generation-tagged.
    pub entity: Entity,
    /// Registered type names of the components attached to the entity.
    pub components: Vec<String>,
}

/// One component's list of fields, held in the two forms the world needs.
///
/// A native component hands over a `&'static` slice from the static data of the
/// artifact that declares it. A component defined by another language describes
/// its fields in a manifest at runtime, so the world owns that list. The two
/// differ only in where the list is stored, so both use a `Cow`.
///
/// The list is kept twice because two readers want different things. The
/// **declared** fields are what a generated mirror such as the C# struct has to
/// describe. The **editor** view flattens `struct:` fields into one dotted row
/// per leaf field (`position.x`), so a nested colour can be edited field by
/// field; that view describes no declaration a mirror could emit. When nothing
/// needed flattening, the two views are the same slice.
#[derive(Debug, Clone)]
pub(crate) struct ComponentFieldLayout {
    /// The fields exactly as the declaring artifact registered them.
    declared: std::borrow::Cow<'static, [crate::component_registry::ComponentFieldDescriptor]>,
    /// The editor's view: `struct:` fields flattened into dotted leaf rows.
    editor: std::borrow::Cow<'static, [crate::component_registry::ComponentFieldDescriptor]>,
}

impl ComponentFieldLayout {
    /// Wraps a layout that lives in the declaring artifact's static data and
    /// needs no flattening.
    pub(crate) fn from_static(
        fields: &'static [crate::component_registry::ComponentFieldDescriptor],
    ) -> Self {
        Self {
            declared: std::borrow::Cow::Borrowed(fields),
            editor: std::borrow::Cow::Borrowed(fields),
        }
    }

    /// Wraps a layout whose editor view was flattened into extra rows.
    pub(crate) fn from_expanded(
        declared: &'static [crate::component_registry::ComponentFieldDescriptor],
        editor: Vec<crate::component_registry::ComponentFieldDescriptor>,
    ) -> Self {
        Self {
            declared: std::borrow::Cow::Borrowed(declared),
            editor: std::borrow::Cow::Owned(editor),
        }
    }

    /// Wraps a layout that another language described at runtime and the world
    /// owns.
    ///
    /// Nothing is flattened here. A manifest lists its fields the way it wants
    /// them seen, so both views hold the same list.
    pub(crate) fn from_owned(
        fields: Vec<crate::component_registry::ComponentFieldDescriptor>,
    ) -> Self {
        Self {
            declared: std::borrow::Cow::Owned(fields.clone()),
            editor: std::borrow::Cow::Owned(fields),
        }
    }

    /// The editor's view: nested `struct:` fields flattened into leaf rows.
    pub(crate) fn fields(&self) -> &[crate::component_registry::ComponentFieldDescriptor] {
        &self.editor
    }

    /// The fields as declared, which a generated mirror has to describe.
    pub(crate) fn declared(&self) -> &[crate::component_registry::ComponentFieldDescriptor] {
        &self.declared
    }
}

/// Holds all entities, archetypes and resources of the ECS.
///
/// This is the centre of the engine. It hands out entity ids together with
/// their generation, owns the archetype storage for every component
/// combination, tracks the archetype and row each entity lives in, stores
/// singleton resources, and keeps the component registry that archetype
/// storage is built from.
pub struct World {
    // --- Entity allocation and locations ---
    /// Next fresh entity ID handed out when the free list is empty.
    next_fresh_entity_id: u64,
    /// Free list of recycled entity IDs, each with the generation it will be
    /// handed out with. Stored as (id, generation) pairs.
    pub(crate) free_entity_ids: Vec<(u64, u32)>,
    /// The stable id of every live entity, keyed by its handle.
    ///
    /// This is world data, not a component. Keeping it here means the
    /// archetypes do not have to carry a row for it through every migration,
    /// and no query needs the id as component data.
    entity_ids: HashMap<Entity, EntityId>,
    /// Reverse of [`Self::entity_ids`], so a saved reference finds its entity
    /// in O(1).
    entity_handles: HashMap<EntityId, Entity>,
    /// Seeds the counter-derived ids used when the platform has no random
    /// source; see [`Self::mint_entity_id`].
    entity_id_fallback_counter: u64,
    // --- Archetypes, columns and entity locations ---
    /// All archetypes in the world
    pub(crate) archetypes: HashMap<ArchetypeId, Archetype>,
    /// The archetype and row each entity lives in.
    pub(crate) entity_locations: HashMap<Entity, EntityLocation>,
    /// Storage factories for creating component storage by component id
    pub(crate) storage_factories: HashMap<ComponentId, StorageFactory>,
    /// The native function table each component id had before its latest
    /// registration replaced it.
    ///
    /// A migration that runs in place still holds rows written with the *old*
    /// layout, so it must release them through the code that wrote them. By
    /// the time it runs, `rehome_native_columns` has already written the
    /// arriving generation's table onto every column. `register_component_inner`
    /// saves the outgoing table here, so the migration can restore it for the
    /// removal that drops those rows.
    pub(crate) retired_native_storage_ops: HashMap<ComponentId, crate::archetype::ColumnOps>,
    /// Updaters that call `update()` on each script component.
    script_updaters: HashMap<ComponentId, scripting::ScriptUpdater>,
    // --- Component registry ---
    /// Component registry for bit indices and names
    pub(crate) component_registry: ComponentRegistry,
    // --- Resources ---
    //
    // `remove_resource_records` clears the five per-id maps below in one place;
    // the stored value itself is removed by its caller. `resource_claim_counts`
    // belongs to `release_resource_claims`, and `resource_registration_sequence`
    // is one counter for the whole world.
    /// The singleton resources, stored by id.
    ///
    /// The values are [`ErasedResource`], not `Box<dyn Any>`. A trait object
    /// carries its destructor in a vtable that belongs to the artifact which
    /// created the value. Resources are never cleared on reload, so retiring
    /// the module that owned one would leave that destructor pointing into
    /// unmapped memory. The erased box holds a replaceable function table
    /// instead, the same approach component columns already use.
    pub(crate) resources: HashMap<ResourceId, ErasedResource>,
    /// Change-detection ticks for each resource, keyed the same as `resources`.
    pub(crate) resource_ticks: HashMap<ResourceId, ComponentTicks>,
    /// The most recent function table registered for each resource id.
    ///
    /// This is the resource equivalent of `storage_factories` for components.
    /// [`Self::rehome_resources`] uses it to update live resources. Both
    /// [`Self::insert_resource`] and [`Self::register_resource`] write it, so a
    /// generation that registers a type again without inserting its value
    /// still leaves a table whose code is mapped.
    pub(crate) resource_factories: HashMap<ResourceId, ErasedResourceOps>,
    /// The declared field layout of each foreign resource that sent one.
    ///
    /// This is the resource counterpart of `component_field_layouts`. A foreign
    /// resource's bytes mean nothing without its layout, so the editor and any
    /// other reader that wants named fields need it. It is separate from
    /// `resource_factories` because a layout is optional: a resource registered
    /// without one still works, it just cannot be inspected.
    pub(crate) resource_field_layouts: HashMap<ResourceId, ComponentFieldLayout>,
    /// For each shared resource id, the declaration that claimed it and the
    /// layout it declared.
    ///
    /// Only shared ids are listed. An ordinary resource is identified by its
    /// `TypeId`, so a second claim on the same id is always the same type. This
    /// is what `ComponentRegistry` records for shared components.
    pub(crate) shared_resource_claims: HashMap<ResourceId, SharedResourceClaim>,
    /// Counter that goes up by one with every resource registration.
    pub(crate) resource_registration_sequence: u64,
    /// The most recent registration sequence recorded for each resource id.
    ///
    /// The resource counterpart of the component registration log, and needed
    /// for the same reason: `resource_factories` only ever grows, so it cannot
    /// tell a table registered by the current generation from one registered
    /// by a generation that has since retired. There is one entry per id rather
    /// than one per registration, because `insert_resource` replaces a value: a
    /// record per replacement would grow for the life of the process, and every
    /// reload diff would scan and sort it.
    pub(crate) resource_registration_stamps: HashMap<ResourceId, u64>,
    /// How many claimants each resource id currently has.
    ///
    /// More than one claimant can register a shared id by name - the project
    /// and a module, for example. `drop_resources` is how one of them retires
    /// what its generation stopped registering. Without this count, the first
    /// claimant to retire the id would destroy a value another one still uses.
    /// Ids with no entry are dropped unconditionally.
    pub(crate) resource_claim_counts: HashMap<ResourceId, u32>,
    // --- Per-frame bookkeeping ---
    /// Counter used for change detection; it only ever goes up.
    ///
    /// The [`Engine`](crate::engine::Engine) adds one to it once per frame. A
    /// parallel batch adds one for each of its members when it reserves their
    /// ticks, and each query that tracks changes adds one when it starts
    /// iterating. The value is a plain `u32` compared with wrapping arithmetic
    /// ([`Tick::is_newer_than`](crate::component::Tick::is_newer_than)), so
    /// wrapping around is harmless.
    pub(crate) change_tick: u32,
    /// The tick at which the system fetching parameters from this world last
    /// ran.
    ///
    /// The [`Engine`](crate::engine::Engine) sets it right before it calls a
    /// system, so change filters built inside the system, such as `Changed<T>`
    /// and `Added<T>`, compare against the right point in time. It starts at
    /// `0`, which means "since the beginning", so an ad-hoc query that no
    /// system drives still sees everything.
    pub(crate) system_last_run: u32,

    /// Counter that goes up by one whenever an archetype is added or removed.
    ///
    /// Queries cache the list of archetypes that match them and compare this
    /// counter on each run: if it has not changed, the cached list is still
    /// correct. It exists only to save that work.
    pub(crate) archetype_generation: u64,

    /// Debug builds only: resources that currently have a mutable borrow.
    ///
    /// A scheduler bug that hands `&mut` for one resource to two systems is
    /// caught here. The [`Engine`](crate::engine::Engine) clears the set at the
    /// start of every frame.
    ///
    /// It sits behind a mutex because the systems of one parallel batch each
    /// rebuild `&mut World` from the same pointer and run at the same time.
    /// Their resource ids never overlap, but a `HashSet` is not safe to modify
    /// from several threads whatever its keys are.
    ///
    /// The mutex must come from `std`, never from `parking_lot`. The host and
    /// every loaded DLL share this `World`, and each one links its own copy of
    /// `pill_engine` and with it its own `parking_lot`, whose list of parked
    /// threads belongs to that copy alone. An unlock running in another copy's
    /// code would look in its own list and never find the waiting thread. `std`
    /// is a single shared library, and on Windows its mutex waits on the lock's
    /// own address, so a wakeup reaches the waiting thread whichever DLL unlocks
    /// it.
    #[cfg(debug_assertions)]
    pub(crate) debug_resource_write_locks: std::sync::Mutex<std::collections::HashSet<ResourceId>>,

    /// How many deferred commands the most recent flush took off the queue.
    ///
    /// `CommandQueue::execute_queued_commands` sets it, and the Engine reads it
    /// for its Tracy plots.
    pub(crate) commands_executed_this_frame: usize,

    /// How long each labelled parallel iterator took, keyed by the string its
    /// `label()` returns.
    ///
    /// It is shared through an `Arc` so iterators can read and write it without
    /// a raw `World` pointer. The `Mutex` handles concurrent access from the
    /// systems of one engine batch.
    pub(crate) iterator_timings: std::sync::Arc<std::sync::Mutex<IteratorTimings>>,

    // --- Persistence registries ---
    /// Serializer for each component type, used when taking a snapshot.
    pub(crate) persist_serializers: HashMap<ComponentId, crate::persistence::SerializeComponentFn>,
    /// Deserializer for each type name, used when restoring a snapshot.
    pub(crate) persist_deserializers: HashMap<String, crate::persistence::DeserializeComponentFn>,
    /// Inserter for each component type: pushes a `Box<dyn Component>` into
    /// storage.
    pub(crate) persist_inserters: HashMap<ComponentId, crate::persistence::InsertComponentFn>,
    /// Schema hash for each persistable component, keyed by type name.
    pub(crate) persist_schema_hashes: HashMap<String, u64>,
    /// Counter that goes up by one on every persistable registration, so the
    /// host can list exactly which types one module's `init` registered.
    pub(crate) persist_registration_sequence: u64,
    /// Log of persistable registrations in order, as `(type_name, sequence)`
    /// pairs.
    pub(crate) persist_registration_log: Vec<(String, u64)>,
    /// Type names the host marked as superseded for the init pass about to
    /// run.
    ///
    /// A reloaded generation may then replace the persist entries of the
    /// generation before it, instead of being refused as a second claimant.
    pub(crate) superseded_persist_names: std::collections::HashSet<String>,
    /// The subset of those names whose earlier generation was already stale,
    /// because it was built against a layout something else has since
    /// replaced.
    ///
    /// A registration of one of these in any layout but the registered one is
    /// skipped, so the component is not laid out a second time.
    pub(crate) stale_superseded_persist_names: std::collections::HashSet<String>,

    /// Serializer for each resource, used when taking a snapshot and keyed by
    /// the live id.
    ///
    /// It is keyed by id rather than name because the serializer has to find
    /// the value, and values are stored under their id. The restorer below is
    /// keyed by name instead, because a restore starts from a snapshot entry,
    /// which carries a name and never an id.
    pub(crate) persist_resource_serializers:
        HashMap<ResourceId, crate::persistence::SerializeResourceFn>,
    /// Restorer for each name: rebuilds a resource from snapshot bytes.
    pub(crate) persist_resource_restorers: HashMap<String, crate::persistence::RestoreResourceFn>,
    /// The persistence name each registered resource id was recorded under.
    pub(crate) persist_resource_names: HashMap<ResourceId, String>,
    /// Schema hash for each persistable resource, so a reload can tell a
    /// reshaped resource from an unchanged one.
    pub(crate) persist_resource_schema_hashes: HashMap<String, u64>,
    /// Shared-component readers already reported as stale, as `(component,
    /// declared hash)` pairs, so a query that is rebuilt every frame logs its
    /// refusal only once.
    pub(crate) reported_schema_mismatches: Vec<(ComponentId, u64)>,
    // --- Component registration log and field layouts ---
    /// Counter that goes up by one on every component registration, plain or
    /// persistable.
    ///
    /// The host uses it to list every type that one module's `init` registered
    /// at all, which shows whether a type was dropped or only downgraded to a
    /// plain component.
    pub(crate) component_registration_sequence: u64,
    /// Log of every component registration, plain and persistable alike, in
    /// order, as `(type_name, component_id, sequence)` triples.
    pub(crate) component_registration_log: Vec<(String, ComponentId, u64)>,
    /// The field layout of each component, as its declaring artifact submitted
    /// it or as another language described it.
    ///
    /// A `#[derive(PillComponent)]` layout is static and lives in the declaring
    /// artifact. A layout from a manifest is created at runtime and owned by the
    /// world. The C# mirror codegen and the editor's generic inspector read
    /// these layouts. Each reloaded generation records them again, and a
    /// descriptor manifest replaces its entry instead of adding to it.
    ///
    /// A native layout is stored with its nested `struct:` fields expanded into
    /// dotted leaf rows (`World::record_component_field_layout`), so a nested
    /// colour or vector can be edited one channel at a time.
    pub(crate) component_field_layouts: HashMap<ComponentId, ComponentFieldLayout>,
    /// The asset type each `Handle<T>` field of a component refers to, keyed by
    /// field name and holding the registered name of `T`.
    ///
    /// The names are owned copies because they come from module code. Each
    /// generation's registration records them again.
    pub(crate) asset_field_types: HashMap<ComponentId, HashMap<String, String>>,
    /// Looks up the layout of a `#[derive(PillMirror)]` value type while another
    /// layout is being recorded, so its nested fields can be expanded.
    ///
    /// The facade's `register_all_components` installs it for the length of its
    /// loop and leaves it `None` at all other times. The value types belong to
    /// the DLL that registers them, and a resolver left in place would point
    /// into that DLL after it unloads.
    pub(crate) value_type_layout_resolver:
        Option<crate::component_registry::ValueTypeLayoutResolver>,
    /// Looks up the declared shape of a `#[derive(PillMirror)]` enum while a
    /// layout is being recorded, so a field of that type reads and writes as
    /// an enum. Installed and removed together with the value-type resolver,
    /// for the same reason.
    pub(crate) enum_layout_resolver: Option<crate::component_registry::EnumLayoutResolver>,
    /// The first registration failure of the current init pass, if any.
    ///
    /// It is set when the 128-type ceiling is reached, when another registry
    /// error comes out of `register_component`, and when a shared resource claim
    /// or a persistable registration is refused. The registration loop of one
    /// artifact takes it out at the end, so the generated `init` can fail the
    /// whole reload instead of running with only half its components registered.
    registration_error: Option<WorldError>,
}

impl World {
    /// Creates an empty world.
    pub fn new() -> Self {
        Self {
            next_fresh_entity_id: 0,
            free_entity_ids: Vec::new(),
            entity_ids: HashMap::new(),
            entity_handles: HashMap::new(),
            entity_id_fallback_counter: 0,
            archetypes: HashMap::new(),
            entity_locations: HashMap::new(),
            storage_factories: HashMap::new(),
            retired_native_storage_ops: HashMap::new(),
            script_updaters: HashMap::new(),
            component_registry: ComponentRegistry::new(),
            resources: HashMap::new(),
            resource_ticks: HashMap::new(),
            resource_factories: HashMap::new(),
            resource_field_layouts: HashMap::new(),
            shared_resource_claims: HashMap::new(),
            resource_registration_sequence: 0,
            resource_registration_stamps: HashMap::new(),
            resource_claim_counts: HashMap::new(),
            change_tick: 0,
            system_last_run: 0,
            archetype_generation: 0,
            #[cfg(debug_assertions)]
            debug_resource_write_locks: std::sync::Mutex::new(std::collections::HashSet::new()),
            commands_executed_this_frame: 0,
            iterator_timings: std::sync::Arc::new(std::sync::Mutex::new(IteratorTimings::new())),
            persist_serializers: HashMap::new(),
            persist_deserializers: HashMap::new(),
            persist_inserters: HashMap::new(),
            persist_schema_hashes: HashMap::new(),
            persist_registration_sequence: 0,
            persist_registration_log: Vec::new(),
            superseded_persist_names: std::collections::HashSet::new(),
            stale_superseded_persist_names: std::collections::HashSet::new(),
            persist_resource_serializers: HashMap::new(),
            persist_resource_restorers: HashMap::new(),
            persist_resource_names: HashMap::new(),
            persist_resource_schema_hashes: HashMap::new(),
            reported_schema_mismatches: Vec::new(),
            component_registration_sequence: 0,
            component_registration_log: Vec::new(),
            component_field_layouts: HashMap::new(),
            asset_field_types: HashMap::new(),
            value_type_layout_resolver: None,
            enum_layout_resolver: None,
            registration_error: None,
        }
    }

    /// Reserves room for at least `additional` more entities.
    ///
    /// The internal maps are grown now, so creating many entities in a row does
    /// not have to grow them again. Call it before a loop that calls
    /// `create_entity()`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use pill_engine::*;
    /// # let mut world = World::new();
    /// world.reserve_entities(10_000);
    /// for _ in 0..10_000 {
    ///     world.create_entity(); // No reallocation overhead
    /// }
    /// ```
    pub fn reserve_entities(&mut self, additional: usize) {
        let _zone = crate::profile_scope!(
            "reserve entities",
            [("Additional entities to reserve: {}", additional)]
        );
        self.free_entity_ids.reserve(additional);
        self.entity_locations.reserve(additional);
    }

    /// Returns the entities of the archetype at `chunk_index`, for language
    /// bindings.
    ///
    /// Entity chunks let a binding walk every archetype in turn. They drive
    /// `EntityTerm` and queries whose component terms are all optional.
    pub fn entity_chunk(&self, chunk_index: usize) -> Option<(ArchetypeId, &[Entity])> {
        let archetype = self.archetypes.values().nth(chunk_index)?;
        Some((archetype.id, archetype.entities.as_slice()))
    }

    /// Returns the entity column of an archetype the caller already knows.
    ///
    /// This is [`Self::entity_chunk`] for a known archetype, used by language
    /// bindings that found the archetype through a driver chunk.
    pub fn entity_chunk_in_archetype(
        &self,
        archetype_id: ArchetypeId,
    ) -> Option<(ArchetypeId, &[Entity])> {
        let archetype = self.archetypes.get(&archetype_id)?;
        Some((archetype.id, archetype.entities.as_slice()))
    }

    /// Entity slots waiting to be handed out again.
    ///
    /// The engine keeps these slots so a later creation reuses an id instead of
    /// growing the id space. A slot whose entity was destroyed comes back with
    /// the next generation; a slot released before its entity was inserted
    /// keeps its own.
    pub fn recycled_entity_id_count(&self) -> usize {
        self.free_entity_ids.len()
    }

    /// Creates an entity from components that are defined at runtime.
    ///
    /// Each entry pairs a component id with the bytes of one value.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorEntityEmpty`] if `components` is empty,
    /// [`WorldError::DescriptorDuplicateComponent`] if an id appears more than
    /// once, [`WorldError::DescriptorComponentNotRegistered`] if a component was
    /// never registered, and [`WorldError::DescriptorByteLengthMismatch`] if a
    /// payload does not match its registered layout size.
    ///
    /// Writing the payloads afterwards can only fail on a world invariant that
    /// no longer holds: [`WorldError::EntityNotFound`],
    /// [`WorldError::DescriptorComponentMissing`],
    /// [`WorldError::ColumnRowsAreNotPlainData`] and
    /// [`WorldError::DescriptorRowInvalid`]. The checks above rule all four out
    /// before the entity is created.
    pub fn create_descriptor_entity(
        &mut self,
        components: &[(ComponentId, Vec<u8>)],
    ) -> Result<Entity, WorldError> {
        // Step 1: Validate the component set - non-empty, unique, registered,
        // and every payload matches its registered layout.
        if components.is_empty() {
            return Err(WorldError::DescriptorEntityEmpty);
        }
        let mut component_ids: Vec<_> = components.iter().map(|(id, _)| *id).collect();
        component_ids.sort();
        component_ids.dedup();
        if component_ids.len() != components.len() {
            return Err(WorldError::DescriptorDuplicateComponent);
        }
        for (id, bytes) in components {
            let Some(StorageFactory::Descriptor(layout)) = self.storage_factories.get(id) else {
                return Err(WorldError::DescriptorComponentNotRegistered { id: *id });
            };
            if bytes.len() != layout.size {
                return Err(WorldError::DescriptorByteLengthMismatch { id: *id });
            }
        }

        // Step 2: Insert the entity with the shared code that appends a row. It
        // grows one zeroed row per descriptor column and stamps the arrival
        // tick. Then write each payload into its row; the checks above rule out
        // a missing column and a wrong payload size, so a write can only fail
        // on a world invariant that no longer holds.
        let entity = self.allocate_entity();
        self.insert_entity_with_components(entity, component_ids, None, |_| {});
        for (id, bytes) in components {
            self.set_descriptor_component_bytes(entity, *id, bytes)?;
        }
        Ok(entity)
    }

    /// Returns whether an entity is alive.
    ///
    /// True when the world holds the entity with that exact generation. False
    /// when it was destroyed, or when its slot was reused with a new
    /// generation.
    #[must_use]
    pub fn is_entity_valid(&self, entity: Entity) -> bool {
        self.entity_locations.contains_key(&entity)
    }

    /// Takes the next entity handle.
    ///
    /// It reuses a slot from the free list when one is there, with the
    /// generation stored for it, so old handles to the slot stay invalid.
    /// Otherwise it takes an id that was never used.
    pub(crate) fn allocate_entity(&mut self) -> Entity {
        let _zone = crate::profile_scope!(
            "allocate entity",
            [
                (
                    "Free entity IDs available for reuse: {}",
                    self.free_entity_ids.len()
                ),
                (
                    "Next fresh entity ID to allocate: {}",
                    self.next_fresh_entity_id
                )
            ]
        );
        // Try to reuse an ID from the free list
        if let Some((id, generation)) = self.free_entity_ids.pop() {
            Entity { id, generation }
        } else {
            // Allocate a fresh ID
            let entity = Entity {
                id: self.next_fresh_entity_id,
                generation: 0,
            };
            self.next_fresh_entity_id += 1;
            entity
        }
    }

    /// Reserves a handle for an entity a command will create later.
    ///
    /// The handle carries a generation like any other, but no query can see the
    /// entity until something inserts it. This is the type-erased counterpart
    /// of [`Commands::create_entity`]. Language bindings, the editor and the
    /// renderer use it to hold a handle before the entity exists.
    pub fn reserve_entity(&mut self) -> Entity {
        self.allocate_entity()
    }

    /// The stable id of a live entity, or `None` once it is destroyed.
    ///
    /// A handle names a slot in the world. An [`EntityId`] names the entity
    /// itself, which is what data saved for a later run stores. Destroying an
    /// entity removes both entries, so the id stops working together with its
    /// entity and never points at the next occupant of the slot.
    #[must_use]
    pub fn entity_id(&self, entity: Entity) -> Option<EntityId> {
        self.entity_ids.get(&entity).copied()
    }

    /// The live entity an [`EntityId`] names, or `None`.
    #[must_use]
    pub fn entity_by_id(&self, id: EntityId) -> Option<Entity> {
        self.entity_handles.get(&id).copied()
    }

    /// Whether an [`EntityId`] already belongs to a live entity.
    #[must_use]
    pub fn entity_id_is_taken(&self, id: EntityId) -> bool {
        self.entity_handles.contains_key(&id)
    }

    /// Makes a fresh id for an entity that is about to be inserted.
    ///
    /// The id is random, so it says nothing about the order entities were
    /// created in, and a stale reference points at nothing instead of at a
    /// later entity. Four attempts are enough to avoid a collision. After that,
    /// or when the platform has no random source at all, the id is derived from
    /// a counter and a warning is logged: a world that cannot create entities
    /// is worse than one with predictable ids.
    fn mint_entity_id(&mut self) -> EntityId {
        for _ in 0..4 {
            match EntityId::random() {
                Ok(id) if !self.entity_id_is_taken(id) => return id,
                Ok(_) => continue,
                Err(error) => {
                    warn!(
                        target: telemetry_target::ECS,
                        error = %error,
                        "the platform has no random source; entity ids are derived from a counter"
                    );
                    break;
                }
            }
        }
        let id = EntityId::from_name(&format!("pill:entity:{}", self.entity_id_fallback_counter));
        self.entity_id_fallback_counter = self.entity_id_fallback_counter.wrapping_add(1);
        id
    }

    /// Gives back a reserved handle that never became an entity.
    ///
    /// The handle must come from [`Self::reserve_entity`] and must not have
    /// been inserted into an archetype. Releasing an entity that queries can
    /// see would let its id be handed out again under a different generation,
    /// while the old entity still exists.
    pub fn release_entity(&mut self, entity: Entity) {
        self.free_entity_ids.push((entity.id, entity.generation));
    }

    /// Recycles the slot of a create that the command flush refused.
    ///
    /// Unlike [`Self::release_entity`], this handle had already been given out,
    /// so the slot goes back with the next generation, the same rule
    /// [`Self::destroy_entity`] follows. A handle the caller kept then cannot
    /// point at whatever uses the slot next. A slot at `u32::MAX` is retired
    /// instead of wrapping; see there for why.
    pub(crate) fn recycle_refused_entity(&mut self, entity: Entity) {
        if entity.generation != u32::MAX {
            self.free_entity_ids
                .push((entity.id, entity.generation + 1));
        }
    }

    /// Finds the archetype for a component set, creating it if it is new.
    ///
    /// Archetypes are reused by every entity with the same component set. The
    /// lookup goes through the `ComponentMask`, so it is a single hash lookup
    /// and does not sort the ids again.
    pub(crate) fn get_or_create_archetype(
        &mut self,
        component_ids: Vec<ComponentId>,
    ) -> ArchetypeId {
        let _zone = crate::profile_scope!(
            "get or create archetype",
            [("Component types in archetype: {}", component_ids.len())]
        );
        // Build the component mask first; it is what the fast lookup uses. The
        // mask identifies the component set whatever order the ids arrive in.
        let mut component_mask = ComponentMask::empty();
        for component_id in &component_ids {
            if let Some(bit) = self.component_registry.get_bit(component_id) {
                component_mask.set(bit);
            }
        }

        // Derive the archetype ID directly from the mask - the mask uniquely
        // identifies the component set, so no separate lookup table is needed.
        let archetype_id = ArchetypeId(component_mask.bits());

        // Hot path: the archetype already exists, which is the common case.
        //
        // The hit is trusted because of how bits are released: a bit goes back
        // to the pool only after every archetype that carries it has been
        // dropped, so the cached component set cannot belong to a different
        // set. The assertion below checks that rule here in case the release
        // path is ever bypassed. It matters because the mask is the id, so an
        // alias would serve the wrong columns and nothing else would show it.
        if let Some(existing) = self.archetypes.get(&archetype_id) {
            debug_assert!(
                existing.component_types.len() == component_ids.len()
                    && component_ids
                        .iter()
                        .all(|component_id| existing.component_types.contains(component_id)),
                "a recycled component bit aliased two component sets onto one archetype id"
            );
            return archetype_id;
        }

        // Cold path: create a new archetype, sorting the ids only now that one
        // is needed.
        let mut sorted_component_ids = component_ids;
        sorted_component_ids.sort();

        crate::profile_message!(
            "new archetype created: {:?} with {} component types (total archetypes now: {})",
            ArchetypeId(component_mask.bits()),
            sorted_component_ids.len(),
            self.archetypes.len() + 1,
        );

        // Create archetype with storage for all component types
        let new_archetype = Archetype::new(
            archetype_id,
            sorted_component_ids,
            component_mask,
            &self.storage_factories,
        );
        self.archetypes.insert(archetype_id, new_archetype);
        self.archetype_generation = self.archetype_generation.wrapping_add(1);

        archetype_id
    }

    /// Starts building a new entity.
    ///
    /// Returns an [`EntityBuilder`] to add components to and then insert the
    /// entity.
    pub fn create_entity(&'_ mut self) -> EntityBuilder<'_> {
        let entity = self.allocate_entity();
        EntityBuilder {
            world: self,
            entity,
            stable_id: None,
            // Most entities have 3-8 components; pre-allocate to avoid
            // reallocation during .with() chains.
            component_inserters: Vec::with_capacity(
                crate::config::EntityBuilderConfig::DEFAULT_COMPONENTS_CAPACITY,
            ),
        }
    }

    /// Adds an entity and its components to the archetype for that component
    /// set.
    ///
    /// `stable_id` is the id the caller chose, or `None` to make a fresh one. A
    /// caller that passes an id has already checked that it is free; the command
    /// path always passes `None`.
    ///
    /// The archetype's columns are type-erased, so pushing a component needs its
    /// concrete type. That is why components arrive through `insert_fn`, which
    /// [`EntityBuilder`] fills in with the types it captured.
    ///
    /// # Panics
    ///
    /// Panics in a debug build when `component_ids` repeats an id. Every caller
    /// refuses that first - the builder and the deferred command executor both
    /// return a duplicate-component error, and `create_descriptor_entity`
    /// removes duplicates - so this names the one state that the archetype's
    /// one-column-per-id addressing cannot survive.
    pub(crate) fn insert_entity_with_components<F>(
        &mut self,
        entity: Entity,
        component_ids: Vec<ComponentId>,
        stable_id: Option<EntityId>,
        insert_fn: F,
    ) where
        F: FnOnce(&mut ComponentColumns),
    {
        let _zone = crate::profile_scope!(
            "insert entity",
            [("Target entity being inserted: {:?}", entity)]
        );
        // Step 0: the component set has to be a set. A repeat would push two
        // rows into one column for the one entity row appended below.
        debug_assert!(
            component_ids
                .iter()
                .enumerate()
                .all(|(index, id)| !component_ids[index + 1..].contains(id)),
            "component set repeated an id while inserting entity {entity:?}: {component_ids:?}"
        );
        // Step 1: Get or create the archetype for this component set.
        let archetype_id = self.get_or_create_archetype(component_ids);
        let current_tick = Tick::new(self.change_tick);

        let archetype = self.archetypes.get_mut(&archetype_id).unwrap_or_else(|| {
            // `get_or_create_archetype` either returns an existing entry
            // or inserts a fresh one, so a miss here is an
            // internal-invariant break. Naming the archetype and entity
            // makes the report useful.
            panic!(
                "archetype {archetype_id:?} vanished after get_or_create_archetype \
                     while inserting entity {entity:?}"
            )
        });
        let row_index: usize = archetype.entities.len();

        // Step 2: Append the entity row, then let the closure push each
        // component's concrete value into its column.
        archetype.entities.push(entity);

        insert_fn(&mut archetype.component_storages);

        // Step 3: Type-erased components have no concrete Rust value for
        // `insert_fn` to push. Allocate their rows here; the command executor
        // overwrites the zero bytes before the new entity becomes observable.
        for &component_id in &archetype.component_types {
            // `insert_fn` above pushed a row for every component that has a
            // Rust type. Only descriptor columns are still empty.
            if component_id.is_native_storage() {
                continue;
            }
            if let Some(column) = archetype.component_storages.get_mut(component_id) {
                // The layout was checked before it reached storage, so this can
                // only fail after roughly 2^60 rows of one component have been
                // pushed: it would run out of address space first. The fallible
                // paths - `move_entity_to_archetype` and
                // `relayout_descriptor_component` - return that error to their
                // caller instead; this function returns nothing, so a panic is
                // the only report it can give.
                column
                    .push_zeroed()
                    .expect("descriptor column growth cannot exhaust the address space");
            }
        }

        // Step 4: Stamp the arrival tick on the row every column just grew by.
        // The column stores its own ticks, so the row exists; the one thing it
        // cannot know is which tick the world is on.
        for &component_id in &archetype.component_types {
            if let Some(column) = archetype.component_storages.get_mut(component_id) {
                column.set_row_ticks(row_index, ComponentTicks::new(current_tick));
            }
        }

        // Step 5: Record where the entity lives so random access stays O(1).
        self.entity_locations.insert(
            entity,
            EntityLocation {
                archetype_id,
                index_in_archetype: row_index,
            },
        );

        // Step 6: Give the entity its stable id and index it in both directions.
        // The id is minted here rather than when the handle was reserved, so a
        // handle that is reserved and never inserted costs nothing.
        let stable_id = stable_id.unwrap_or_else(|| self.mint_entity_id());
        self.entity_ids.insert(entity, stable_id);
        self.entity_handles.insert(stable_id, entity);
    }

    /// Moves an entity to another archetype, keeping every component it keeps.
    ///
    /// Each column the two archetypes share moves bitwise: the row's bytes go
    /// to the destination, and the source gives up the row without running drop
    /// glue, because ownership moved with the bytes. There is one path for every
    /// component, native or descriptor, so a component holding a `Vec` costs a
    /// pointer copy instead of a deep clone followed by a deep drop. No `Clone`
    /// impl runs during what the engine calls a relocation.
    ///
    /// `attach` fills the columns that the destination has and the source does
    /// not, which is exactly the component being added, or nothing for a
    /// removal. It runs before any row moves, because it is the only part of the
    /// migration that can still fail. A refusal there costs only the entity's
    /// place in the destination's list; a failure after the rows moved would
    /// leave rows in two archetypes at once.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::EntityNotFound`] when the entity has no location
    /// record, [`WorldError::ArchetypeMissing`] when the source or the
    /// destination archetype is missing from the world,
    /// [`WorldError::DescriptorStorageMissing`] when a component named by an
    /// archetype has no storage column - the desync a partially applied hot
    /// reload can leave behind - [`WorldError::DescriptorSizeMismatch`] when two
    /// columns for one component id disagree on their row width, and whatever
    /// `attach` returns. Every one of these is raised before the first row
    /// moves, so a refused migration leaves the entity where it was.
    pub(crate) fn move_entity_to_archetype<F>(
        &mut self,
        entity: Entity,
        new_component_ids: Vec<ComponentId>,
        attach: F,
    ) -> Result<(), WorldError>
    where
        F: FnOnce(&mut ComponentColumns) -> Result<(), WorldError>,
    {
        let _zone = crate::profile_scope!(
            "move entity to archetype",
            [(
                "Entity being migrated: {:?}, New component type count: {}",
                entity,
                new_component_ids.len()
            )]
        );
        // Step 1: Find where the entity currently lives.
        let old_location = match self.entity_locations.get(&entity) {
            Some(location) => *location,
            None => return Err(WorldError::EntityNotFound),
        };

        let old_archetype_id = old_location.archetype_id;
        let old_index = old_location.index_in_archetype;

        // Step 2: Get or create the destination archetype, and stop here if the
        // entity already lives in it.
        let new_archetype_id = self.get_or_create_archetype(new_component_ids);

        // The same archetype means there is nothing to do. Adding a component
        // should never end up here.
        if old_archetype_id == new_archetype_id {
            debug!(
                target: telemetry_target::ECS,
                entity = ?entity,
                archetype_id = ?new_archetype_id,
                "entity already lives in the destination archetype; nothing to migrate"
            );
            return Ok(());
        }

        // Step 3: Take the source archetype out of the map while the move runs.
        // Moving a row needs `&mut` on both archetypes at once, and the borrow
        // checker cannot express that through one `HashMap`. Owning one of them
        // outright makes the aliasing rule explicit here.
        let Some(mut old_archetype) = self.archetypes.remove(&old_archetype_id) else {
            return Err(WorldError::ArchetypeMissing {
                entity,
                archetype_id: old_archetype_id,
            });
        };

        // Step 4: Move the row. Every check that can refuse runs in the helper
        // below, which has the source archetype in hand, so this single exit
        // point puts the source back whatever the helper returns.
        let outcome = self.move_row_into_destination(
            &mut old_archetype,
            entity,
            old_index,
            new_archetype_id,
            attach,
        );

        // Step 5: Put the source archetype back, or drop it. A move that
        // succeeded and left it empty drops it, so its columns do not keep memory
        // for a component set nothing uses. Every other exit - a refusal, or
        // rows still in it - puts it back exactly as it was found.
        if outcome.is_ok() && old_archetype.entities.is_empty() {
            self.archetype_generation = self.archetype_generation.wrapping_add(1);
        } else {
            self.archetypes.insert(old_archetype_id, old_archetype);
        }

        outcome
    }

    /// Moves one entity's row into the archetype `new_archetype_id`.
    ///
    /// This is part of [`Self::move_entity_to_archetype`], which has already
    /// taken the source archetype out of the world. A refusal can return
    /// immediately because the caller puts that archetype back at its single
    /// exit, not here.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::ArchetypeMissing`] when the destination archetype
    /// is missing, [`WorldError::DescriptorStorageMissing`] when a component
    /// named by an archetype has no storage column - the desync a partially
    /// applied hot reload can leave behind -
    /// [`WorldError::DescriptorSizeMismatch`] when two columns for one component
    /// id disagree on their row width, and whatever `attach` returns.
    fn move_row_into_destination<F>(
        &mut self,
        old_archetype: &mut Archetype,
        entity: Entity,
        old_index: usize,
        new_archetype_id: ArchetypeId,
        attach: F,
    ) -> Result<(), WorldError>
    where
        F: FnOnce(&mut ComponentColumns) -> Result<(), WorldError>,
    {
        // Step 1: Check that the destination can take the row and reserve its
        // space before anything moves. A component the destination has no column
        // for is the manifest/storage desync that `DescriptorStorageMissing`
        // reports, and two columns for one id at different widths is its reload
        // equivalent. Finding both here keeps a failed migration atomic:
        // otherwise the destination would already hold part of the row while the
        // source still holds the entity. Reserving the space here has the same
        // reason, because afterwards no carried row can fail to land. Nothing
        // has moved when this refuses, so the archetype the caller puts back is
        // the world exactly as it was handed over.
        Self::prepare_destination_columns(
            &mut self.archetypes,
            old_archetype,
            entity,
            old_index,
            new_archetype_id,
        )?;

        // Step 2: Write the entity's row into the destination. The attached
        // component goes first, while a refusal is still free; the carried rows
        // follow, and Step 1 has ruled out every way those can fail.
        let Some(new_archetype) = self.archetypes.get_mut(&new_archetype_id) else {
            return Err(WorldError::ArchetypeMissing {
                entity,
                archetype_id: new_archetype_id,
            });
        };

        let new_index = new_archetype.entities.len();
        new_archetype.entities.push(entity);

        if let Err(error) = attach(&mut new_archetype.component_storages) {
            // The entity is not yet anywhere but this list, and no row has
            // moved, so dropping its place undoes the whole attempt.
            new_archetype.entities.truncate(new_index);
            return Err(error);
        }

        let mut carried_result = Ok(());
        for component_id in &new_archetype.component_types {
            let Some(source) = old_archetype.component_storages.get_mut(*component_id) else {
                // Not carried: the destination gained this component, and
                // `attach` above wrote it.
                continue;
            };
            let Some(destination) = new_archetype.component_storages.get_mut(*component_id) else {
                // Step 1 proved every destination column exists.
                carried_result = Err(WorldError::DescriptorStorageMissing {
                    component_id: *component_id,
                    archetype_id: new_archetype_id,
                });
                break;
            };
            if let Err(error) = destination.take_row_from(source, old_index) {
                carried_result = Err(error);
                break;
            }
        }
        // Unreachable after Step 1: the widths agree, the row exists, and every
        // destination column has room. If it happens anyway, the caller reports
        // the broken invariant instead of the move failing silently.
        carried_result?;

        // Stamp the arrival tick on the component this migration added. A
        // carried row brought its own ticks along, because they are part of the
        // row the destination column took, so a component the source already had
        // needs nothing here.
        let current_tick = Tick::new(self.change_tick);
        for &component_id in &new_archetype.component_types {
            if old_archetype.component_storages.contains(component_id) {
                continue;
            }
            if let Some(column) = new_archetype.component_storages.get_mut(component_id) {
                column.set_row_ticks(new_index, ComponentTicks::new(current_tick));
            }
        }

        // Every destination column grew by exactly one row. A miss means `attach`
        // left a column it was responsible for one row short, which is the desync
        // Step 1 rules out for every other cause.
        debug_assert_eq!(
            new_archetype.entities.len(),
            new_index + 1,
            "the migrated entity is the destination's last row"
        );
        for &component_id in &new_archetype.component_types {
            if let Some(column) = new_archetype.component_storages.get(component_id) {
                debug_assert_eq!(
                    column.len(),
                    new_index + 1,
                    "a destination column did not grow by exactly one row"
                );
            }
        }

        self.entity_locations.insert(
            entity,
            EntityLocation {
                archetype_id: new_archetype_id,
                index_in_archetype: new_index,
            },
        );

        // Step 3: Remove the entity from the old archetype with `swap_remove`,
        // which is O(1). A carried column already had its row removed by the
        // move that took it, so what is left is the entity list and the columns
        // the destination did not take. Those columns hold the components this
        // migration drops, and they are the only rows still to release.
        if old_index < old_archetype.entities.len() {
            // Remove the columns first, while the destination is still
            // reachable. Whether it has a column for a component is the same
            // condition the move above branched on, so it tells a carried
            // column apart from a dropped one.
            let destination_columns = self
                .archetypes
                .get(&new_archetype_id)
                .map(|archetype| &archetype.component_storages);
            for &component_id in &old_archetype.component_types {
                let carried =
                    destination_columns.is_some_and(|columns| columns.contains(component_id));
                if !carried {
                    if let Some(column) = old_archetype.component_storages.get_mut(component_id) {
                        column.swap_remove_discard(old_index);
                    }
                }
            }

            old_archetype.entities.swap_remove(old_index);

            // Update the location of the entity that was swapped (if any)
            if old_index < old_archetype.entities.len() {
                let swapped_entity = old_archetype.entities[old_index];
                if let Some(swapped_location) = self.entity_locations.get_mut(&swapped_entity) {
                    swapped_location.index_in_archetype = old_index;
                }
            }
        }

        Ok(())
    }

    /// Checks that the destination archetype can hold the entity's row, and
    /// reserves the space it needs, before the migration moves anything.
    ///
    /// This is part of [`Self::move_entity_to_archetype`]. It is separate so the
    /// destination can be borrowed mutably while the source archetype, already
    /// out of the map by then, is borrowed immutably beside it.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::ArchetypeMissing`] when the destination archetype
    /// is missing, [`WorldError::DescriptorStorageMissing`] when it names a
    /// component it has no column for, [`WorldError::DescriptorSizeMismatch`]
    /// when a carried column's row width disagrees with the destination's,
    /// [`WorldError::DescriptorRowInvalid`] when the entity's recorded row is
    /// not in a carried column, and [`WorldError::DescriptorLayoutInvalid`]
    /// when a column cannot grow. Nothing moves in any case; a reservation that
    /// already succeeded only grows a buffer, which no reader can observe.
    fn prepare_destination_columns(
        archetypes: &mut HashMap<ArchetypeId, Archetype>,
        old_archetype: &Archetype,
        entity: Entity,
        old_index: usize,
        new_archetype_id: ArchetypeId,
    ) -> Result<(), WorldError> {
        let Some(new_archetype) = archetypes.get_mut(&new_archetype_id) else {
            return Err(WorldError::ArchetypeMissing {
                entity,
                archetype_id: new_archetype_id,
            });
        };
        // `component_types` and `component_storages` are separate fields, so
        // the list can be read while the columns beside it are written.
        let component_type_ids: &[ComponentId] = &new_archetype.component_types;
        for &component_id in component_type_ids {
            let source = old_archetype.component_storages.get(component_id);
            // A component both archetypes name, but the source has no column
            // for, cannot be carried, and nothing else would write it, so the
            // destination would end the migration one row short. That is the
            // desync a partially applied reload leaves behind, and this
            // migration cannot work around this shape of it. A component being
            // dropped is different: its missing column simply has nothing left
            // to release.
            if source.is_none() && old_archetype.component_types.contains(&component_id) {
                return Err(WorldError::DescriptorStorageMissing {
                    component_id,
                    archetype_id: old_archetype.id,
                });
            }
            let Some(destination) = new_archetype.component_storages.get_mut(component_id) else {
                return Err(WorldError::DescriptorStorageMissing {
                    component_id,
                    archetype_id: new_archetype_id,
                });
            };
            if let Some(source) = source {
                if source.element_size() != destination.element_size() {
                    return Err(WorldError::DescriptorSizeMismatch);
                }
                if old_index >= source.len() {
                    return Err(WorldError::DescriptorRowInvalid);
                }
            }
            destination.reserve_row()?;
        }
        Ok(())
    }

    /// Removes an entity from the world completely.
    ///
    /// The entity leaves its archetype and every tracking structure is updated.
    /// Returns `true` if the entity was found and removed, `false` otherwise.
    #[must_use]
    pub fn destroy_entity(&mut self, entity: Entity) -> bool {
        let _zone = crate::profile_scope!(
            "destroy entity",
            [("Target entity being destroyed: {:?}", entity)]
        );
        // Step 1: Remove the entity's location record; a missing record
        // means the entity is already gone.
        let location = match self.entity_locations.remove(&entity) {
            Some(location) => location,
            None => return false, // Entity doesn't exist
        };

        // Step 1b: forget its stable id. Both indexes go together so the id
        // never names a later occupant of the recycled slot.
        if let Some(stable_id) = self.entity_ids.remove(&entity) {
            self.entity_handles.remove(&stable_id);
        }

        let archetype = match self.archetypes.get_mut(&location.archetype_id) {
            Some(archetype) => archetype,
            None => return false,
        };

        let old_index = location.index_in_archetype;

        // Step 2: swap_remove the entity and its component rows for O(1)
        // removal, updating the location of the entity swapped into its place.
        if old_index < archetype.entities.len() {
            archetype.entities.swap_remove(old_index);

            // Update the location of the entity that was swapped (if any)
            if old_index < archetype.entities.len() {
                let swapped_entity = archetype.entities[old_index];
                if let Some(swapped_location) = self.entity_locations.get_mut(&swapped_entity) {
                    swapped_location.index_in_archetype = old_index;
                }
            }

            // Also swap_remove the entity's row from every column. A column
            // releases its own change ticks together with the row, so one pass
            // is enough. Rust's split borrow lets the shared `component_types`
            // slice be read while the columns are borrowed mutably, so the ids
            // do not have to be cloned.
            let component_type_ids: &[ComponentId] = &archetype.component_types;
            for component_id in component_type_ids {
                if let Some(column) = archetype.component_storages.get_mut(*component_id) {
                    column.swap_remove_discard(old_index);
                } else {
                    // A component with no column has no data to remove, so
                    // skipping it is safe. The missing column is the
                    // manifest/storage desync that
                    // `WorldError::DescriptorStorageMissing` reports. It is
                    // logged rather than panicked on, because this runs inside
                    // `process_frame` for managed projects.
                    debug!(
                        target: telemetry_target::ECS,
                        component_id = ?component_id,
                        archetype_id = ?archetype.id,
                        "destroy_entity: component has no storage column; skipping"
                    );
                }
            }
        }

        // Step 3: If the archetype is now empty, remove it entirely to
        // prevent memory leaks.
        let archetype_id = location.archetype_id;
        if archetype.entities.is_empty() {
            self.archetypes.remove(&archetype_id);
            self.archetype_generation = self.archetype_generation.wrapping_add(1);
        }

        // Step 4: Put the entity id back with the next generation, so the slot
        // can be used again while old handles to it stop working.
        //
        // A slot whose generation has reached `u32::MAX` is retired instead of
        // wrapping back to zero. Wrapping would make every handle from 2^32
        // recycles ago valid again, which is the classic ABA problem. It is not
        // reachable in practice, but it is cheap to make the wrap impossible
        // instead of silent.
        if entity.generation != u32::MAX {
            self.free_entity_ids
                .push((entity.id, entity.generation + 1));
        }

        true
    }

    /// Removes every archetype that has no entities left.
    ///
    /// This is rarely needed, because an empty archetype is reused when an
    /// entity with the same component set appears again. It is useful when the
    /// memory its columns hold should be given back.
    pub fn cleanup_empty_archetypes(&mut self) {
        let empty_archetype_ids: Vec<ArchetypeId> = self
            .archetypes
            .iter()
            .filter(|(_, archetype)| archetype.entities.is_empty())
            .map(|(id, _)| *id)
            .collect();

        let _zone = crate::profile_scope!(
            "cleanup empty archetypes",
            [("removed: {}", empty_archetype_ids.len())]
        );
        for archetype_id in &empty_archetype_ids {
            self.archetypes.remove(archetype_id);
        }
        if !empty_archetype_ids.is_empty() {
            self.archetype_generation = self.archetype_generation.wrapping_add(1);
        }
    }

    /// Total number of entities alive in the world.
    #[inline]
    pub fn entity_count(&self) -> usize {
        self.entity_locations.len()
    }

    /// Iterates over every archetype in the world, for read-only column
    /// access.
    ///
    /// This is the narrow opening a renderer outside this crate needs. Finding
    /// the entities to draw means reading each archetype's columns, and
    /// `archetypes` is crate-private so nothing outside can change the ECS
    /// internals. It hands out `&Archetype` only; the map itself and every way
    /// of changing it stay sealed in this crate.
    ///
    /// It goes together with [`World::component_registry`], which finds the
    /// columns seen here by stable type name and size.
    #[inline]
    pub fn archetypes_iter(&self) -> impl Iterator<Item = &Archetype> {
        self.archetypes.values()
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// ComponentInserter
// =============================================================================

/// Type-erased way to push one component value into storage.
///
/// `EntityBuilder` boxes one `ComponentInserter` for each `.with(...)` call.
/// That way the concrete component type is captured, and the builder itself
/// does not have to be generic over every component type.
trait ComponentInserter {
    /// Push the captured component value into the given storage.
    fn insert(self: Box<Self>, storage: &mut ComponentColumns);
    /// Return the [`ComponentId`] of the captured component type.
    fn component_id(&self) -> ComponentId;
}

/// The [`ComponentInserter`] implementation that holds one concrete component
/// type.
struct TypedComponentInserter<T: Component> {
    /// The component value to insert when the entity is built.
    component: T,
}

impl<T: Component> ComponentInserter for TypedComponentInserter<T> {
    fn insert(self: Box<Self>, storage: &mut ComponentColumns) {
        storage.column_of_mut::<T>().push::<T>(self.component);
    }

    fn component_id(&self) -> ComponentId {
        ComponentId::of::<T>()
    }
}

// =============================================================================
// EntityBuilder
// =============================================================================

/// Builds an entity by adding one component at a time.
///
/// [`World::create_entity`] returns one. Add components with
/// [`with`](Self::with), then insert the entity into the world with
/// [`build`](Self::build).
///
/// # Examples
///
/// ```
/// # use pill_engine::*;
/// # #[derive(Debug, Clone)] struct Transform { x: f32, y: f32, z: f32 }
/// # impl Component for Transform {}
/// # #[derive(Debug, Clone)] struct Velocity { x: f32, y: f32 }
/// # impl Component for Velocity {}
/// # let mut world = World::new();
/// # world.register_component::<Transform>();
/// # world.register_component::<Velocity>();
/// world.create_entity()
///     .with(Transform { x: 0.0, y: 0.0, z: 0.0 })
///     .with(Velocity { x: 10.0, y: 0.0 })
///     .build().unwrap();
/// ```
pub struct EntityBuilder<'w> {
    /// World the built entity is inserted into on [`build`](Self::build).
    world: &'w mut World,
    /// Entity handle reserved by the free list for this build.
    entity: Entity,
    /// Stable id the creator supplied, or `None` to mint a fresh one.
    stable_id: Option<EntityId>,
    /// Type-erased component inserters accumulated via [`with`](Self::with).
    component_inserters: Vec<Box<dyn ComponentInserter>>,
}

impl<'w> EntityBuilder<'w> {
    /// Builds the entity with an id the caller already chose.
    ///
    /// A scene load uses this: the id written in the file has to be the id the
    /// loaded entities carry, or every reference inside the scene would point at
    /// nothing. [`build`](Self::build) refuses an id that a live entity already
    /// holds.
    pub fn with_id(mut self, id: EntityId) -> Self {
        self.stable_id = Some(id);
        self
    }

    /// Adds a component to the entity being built.
    pub fn with<T>(mut self, component: T) -> Self
    where
        T: Component,
    {
        self.component_inserters
            .push(Box::new(TypedComponentInserter { component }));
        self
    }

    /// Finishes the build and inserts the entity into the world.
    ///
    /// # Errors
    ///
    /// Returns [`BuildError::ComponentNotRegistered`] when a component type
    /// added with [`.with()`](Self::with) was not registered with the world
    /// first, [`BuildError::DuplicateComponent`] when one type was added more
    /// than once, and [`BuildError::DuplicateEntityId`] when the id given to
    /// [`with_id`](Self::with_id) already belongs to a live entity.
    pub fn build(self) -> Result<Entity, BuildError> {
        let mut component_ids: Vec<ComponentId> = self
            .component_inserters
            .iter()
            .map(|component_inserter| component_inserter.component_id())
            .collect();
        let _zone = crate::profile_scope!(
            "entity build",
            [("Component types on entity: {}", component_ids.len())]
        );
        let entity = self.entity;

        // Refuse a repeated id before the row exists. The set defines the
        // archetype's columns, so a second value of one type would add two rows to
        // one column for one entity row. A new archetype panics on the repeated
        // column, and a cached one accepts both values and leaves every later
        // entity reading the row before its own. The deferred path refuses the same
        // way in `commands::execute_create_entity`.
        component_ids.sort_unstable();
        if let Some(duplicate) = component_ids
            .windows(2)
            .find(|pair| pair[0] == pair[1])
            .map(|pair| pair[0])
        {
            self.world.release_entity(entity);
            return Err(BuildError::DuplicateComponent { id: duplicate });
        }

        // Validate that every component type is registered before we try to
        // create the archetype (which would panic on an unregistered type).
        for &id in &component_ids {
            if !self.world.storage_factories.contains_key(&id) {
                self.world.release_entity(entity);
                return Err(BuildError::ComponentNotRegistered { id });
            }
        }

        // A supplied id has to be free. Two live entities under one id would make a
        // saved reference ambiguous, and the reverse index can hold only one of
        // them.
        if let Some(id) = self.stable_id {
            if self.world.entity_id_is_taken(id) {
                self.world.release_entity(entity);
                return Err(BuildError::DuplicateEntityId { id });
            }
        }

        let stable_id = self.stable_id;
        let component_inserters = self.component_inserters;
        self.world
            .insert_entity_with_components(entity, component_ids, stable_id, |storage| {
                for component_inserter in component_inserters {
                    component_inserter.insert(storage);
                }
            });
        Ok(entity)
    }
}

// =============================================================================
// Free Functions
// =============================================================================
//
// Per-thread ticks. In parallel mode several systems run at the same time on
// different threads. Each one needs its own idea of which tick its last run
// started at, so change filters such as `Changed<T>` compare against the right
// baseline. A field on `World` would be shared and would race, so the value is
// thread-local. The engine's parallel dispatch stores the baseline before a
// system runs and puts the previous value back afterwards. In sequential mode
// both overrides are `None`, and queries read the shared fields on `World`
// instead.
//
// The same thread-local storage also holds the tick reserved for the current
// run. A parallel batch runs every system against one world, so increasing the
// shared counter there would be an unsynchronized read-modify-write. Instead
// the engine gives each member of the batch its own tick, taken on the dispatch
// thread, and installs it here.
//
// The values live in `pill_core::system_ticks`, not in this crate. `Query` is
// generic, so a system's queries read the thread-local from the DLL that
// declares the system. A copy in this rlib would exist once per DLL, and a
// module's systems would never see the values the engine wrote.

/// The tick this thread's system last started at, or `None` when no parallel
/// batch is running on this thread.
#[inline]
fn per_thread_last_run_tick() -> Option<Tick> {
    pill_core::system_ticks::last_run_tick().map(Tick::new)
}

/// The tick the engine reserved for the system running on this thread.
///
/// `None` means the caller should increase the shared counter on `World`.
#[inline]
fn per_thread_this_run_tick() -> Option<Tick> {
    pill_core::system_ticks::this_run_tick().map(Tick::new)
}

/// Stores this thread's last-run tick and returns the previous one, so the
/// caller can put it back when the system finishes.
#[inline]
pub(crate) fn set_per_thread_last_run_tick(value: Option<Tick>) -> Option<Tick> {
    pill_core::system_ticks::replace_last_run_tick(value.map(Tick::get)).map(Tick::new)
}

/// Stores the tick reserved for the system about to run on this thread and
/// returns the previous one, so the caller can put it back afterwards.
#[inline]
pub(crate) fn set_per_thread_this_run_tick(value: Option<Tick>) -> Option<Tick> {
    pill_core::system_ticks::replace_this_run_tick(value.map(Tick::get)).map(Tick::new)
}

/// Whether a size and alignment can describe a foreign resource's allocation.
///
/// A foreign resource is one that another language declares. Both
/// [`World::register_foreign_resource`] and
/// [`World::relayout_foreign_resource`] need this check, and keeping it in one
/// place means the two cannot disagree. If a declaration passed but its later
/// relayout did not, the world would hold a resource shape it can no longer
/// store.
fn is_valid_foreign_layout(size: usize, align: usize) -> bool {
    size != 0
        && align != 0
        && align.is_power_of_two()
        && std::alloc::Layout::from_size_align(size, align).is_ok()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod layout_tests {
    use super::*;

    /// Verifies that `EntityLocation` is 32 bytes with 16-byte alignment.
    #[test]
    fn entity_location_size() {
        assert_eq!(std::mem::size_of::<EntityLocation>(), 32);
        assert_eq!(std::mem::align_of::<EntityLocation>(), 16);
    }
}

#[cfg(test)]
mod tests;
