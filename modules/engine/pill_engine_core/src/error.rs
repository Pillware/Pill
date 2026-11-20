//! Typed errors for the ECS engine: one enum per subsystem, each variant's
//! message written down once, and the composed [`EngineError`].
//!
//! # Responsibilities
//!
//! - Declare the engine's subsystem errors ([`WorldError`],
//!   [`CommandError`], [`AddComponentError`], [`RemoveComponentError`],
//!   [`BuildError`], [`PersistenceError`], [`SystemError`] and
//!   [`SystemFailure`]).
//! - Compose [`WorldError`], [`CommandError`] and [`PersistenceError`] into
//!   [`EngineError`], together with the shared host errors from `pill_core`.
//!   [`AddComponentError`], [`RemoveComponentError`] and [`BuildError`] are
//!   returned directly by the call that raises them, and [`SystemError`]
//!   travels inside [`SystemFailure`] because a failing system never aborts the
//!   frame.
//!
//! # Design
//!
//! A variant that carries its own message states the wording once, in a
//! `#[message(...)]` attribute. The `#[engine_error]` macro turns that
//! definition into plain text, a diagnostic code and styled output through the
//! `pill_core` diagnostics runtime. These enums never import a styling crate
//! and never turn a source error into text; the enums that compose do it with
//! `?` through transparent `#[from]` wrappers.
//!
//! The frame loop does not hand a frontend an [`EngineError`]:
//! [`Engine::process_frame`](crate::Engine::process_frame) reports
//! `Result<(), Vec<CommandError>>`, and
//! [`Engine::drain_system_failures`](crate::Engine::drain_system_failures)
//! returns the collected [`SystemFailure`]s. [`EngineError`] is there for host
//! code that wants one error type.

// Standard library
use std::fmt;

// External crates
use pill_core::error::{EngineMessage, MessageRenderer, SemanticRole};
use pill_core_macros::engine_error;

// Current crate
use crate::{archetype::ArchetypeId, ComponentId, Entity, ResourceId};

// =============================================================================
// World Errors
// =============================================================================

/// Failures the archetype world reports about its own state.
///
/// The world raises these when a call would break one of its rules: entity
/// storage and migration, the component and resource registries, and the
/// checks that keep two binaries sharing one identity in agreement. A call
/// fails this way for an unregistered ID, a byte layout that does not match, a
/// full component table, or a name that another type already claimed.
#[engine_error(namespace = engine::world, runtime = ::pill_core::error)]
#[derive(PartialEq)]
pub enum WorldError {
    /// The stable ID of a descriptor component cannot be zero.
    #[message("descriptor component stable ID cannot be zero")]
    DescriptorStableIdZero,

    /// The size of a descriptor component cannot be zero.
    #[message("descriptor component size cannot be zero")]
    DescriptorSizeZero,

    /// The alignment of a descriptor component must be a non-zero power of two.
    #[message("descriptor component alignment must be a non-zero power of two")]
    DescriptorAlignmentInvalid,

    /// The size and alignment pair does not form a valid memory layout.
    #[message("descriptor component size and alignment do not form a valid layout")]
    DescriptorLayoutInvalid,

    /// The stable ID is already registered with a different layout or name.
    #[message("descriptor component stable ID is already registered with another name or schema")]
    DescriptorAlreadyRegistered,

    /// A descriptor component cannot be remapped onto itself.
    #[message("descriptor component cannot be remapped onto itself")]
    DescriptorRemapSelf,

    /// The world's component type limit has been reached.
    ///
    /// A world holds at most 128 component types, and the engine's own types
    /// take only a few of those slots. The rest are declared by the project and
    /// by the descriptor manifests the managed runtime sends, so running out is
    /// a configuration outcome, not a programming error. The error carries the
    /// offending type name and the number registered so far, so the host can
    /// report it like any other engine error instead of panicking.
    #[message(
        "component type limit exceeded: cannot register ",
        name_style(type_name),
        " (max 128 component types, ",
        debug_value(count),
        " already registered)"
    )]
    ComponentTypeLimitExceeded {
        /// The type that could not be registered.
        type_name: String,
        /// How many component types were already registered.
        count: u8,
    },

    /// A descriptor entity must carry at least one component.
    #[message("a descriptor entity must contain at least one component")]
    DescriptorEntityEmpty,

    /// A descriptor entity cannot contain the same component twice.
    #[message("a descriptor entity cannot contain duplicate components")]
    DescriptorDuplicateComponent,

    /// The component ID was never registered with this world.
    #[message("descriptor component ", debug_value(id), " is not registered")]
    DescriptorComponentNotRegistered { id: ComponentId },

    /// The supplied bytes are not the size the registered layout declares.
    #[message(
        "descriptor component ",
        debug_value(id),
        " byte length does not match its manifest"
    )]
    DescriptorByteLengthMismatch { id: ComponentId },

    /// The entity does not exist in the world.
    #[message("entity not found")]
    EntityNotFound,

    /// The entity already carries the component being added.
    #[message("entity already contains the descriptor component")]
    DescriptorComponentAlreadyPresent,

    /// The entity does not carry the component being removed or set.
    #[message("entity does not contain the descriptor component")]
    DescriptorComponentMissing,

    /// A row index or byte range does not fit the component's column.
    #[message("descriptor component row or byte length is invalid")]
    DescriptorRowInvalid,

    /// A byte copy length does not match the registered element size.
    #[message("descriptor component byte length does not match its registered size")]
    DescriptorSizeMismatch,

    /// A raw-byte write was attempted on a column whose rows own resources.
    ///
    /// The byte mutators copy and overwrite rows without running the element's
    /// destructor, which is safe only for plain data. On a column whose type
    /// has a real destructor, the same calls would leak the value they
    /// overwrite, or leave two columns owning one allocation.
    #[message("raw-byte writes are not valid on a column whose rows own resources")]
    ColumnRowsAreNotPlainData,

    /// An archetype lists a registered descriptor component but has no storage
    /// column for it.
    ///
    /// The manifest and the archetype's columns disagree here, which a partly
    /// applied relayout or reload can leave behind. The component is listed,
    /// but its storage was never created.
    #[message(
        "descriptor component ",
        debug_value(component_id),
        " has no storage column in archetype ",
        debug_value(archetype_id)
    )]
    DescriptorStorageMissing {
        /// The descriptor component whose column is absent.
        component_id: ComponentId,
        /// The archetype that was expected to own the column.
        archetype_id: ArchetypeId,
    },

    /// A descriptor column's element size disagrees with the registered layout
    /// the migration plan was validated against.
    ///
    /// A column and the storage factory that describe it have drifted apart,
    /// which is what a partly applied relayout leaves behind.
    #[message(
        "descriptor component ",
        debug_value(component_id),
        " in archetype ",
        debug_value(archetype_id),
        " has ",
        debug_value(actual),
        " byte elements; the registered layout declares ",
        debug_value(expected)
    )]
    ComponentColumnLayoutMismatch {
        /// The component whose column disagrees.
        component_id: ComponentId,
        /// The archetype owning the column.
        archetype_id: ArchetypeId,
        /// The size the registered layout declares.
        expected: usize,
        /// The size the column's elements are stored at.
        actual: usize,
    },

    /// A foreign resource declaration carries no name to be identified by.
    #[message("a foreign resource must declare a name")]
    ForeignResourceNameEmpty,

    /// The declared layout of a foreign resource cannot describe an allocation.
    #[message(
        "foreign resource layout of ",
        debug_value(size),
        " bytes at alignment ",
        debug_value(align),
        " is not a valid allocation"
    )]
    ForeignResourceLayoutInvalid {
        /// Declared size in bytes.
        size: usize,
        /// Declared alignment in bytes.
        align: usize,
    },

    /// The id does not name a registered foreign resource.
    #[message("resource ", debug_value(id), " is not a registered foreign resource")]
    ForeignResourceNotRegistered {
        /// The resource that was asked for.
        id: ResourceId,
    },

    /// A foreign resource cannot be remapped onto itself.
    #[message("foreign resource cannot be remapped onto itself")]
    ForeignResourceRemapSelf,

    /// The id does not name a registered shared resource.
    #[message("resource ", debug_value(id), " is not a registered shared resource")]
    SharedResourceNotRegistered {
        /// The resource that was asked for.
        id: ResourceId,
    },

    /// The id was registered by a Rust type, so foreign bytes cannot be stored
    /// under it.
    #[message(
        "resource ",
        debug_value(id),
        " is registered by a Rust type; foreign bytes cannot replace its value"
    )]
    ForeignResourceFactoryIsNative {
        /// The resource the payload was offered to.
        id: ResourceId,
    },

    /// A Rust value is stored under the id, and its type fixes its shape.
    #[message(
        "resource ",
        debug_value(id),
        " holds a Rust value; a foreign declaration cannot reshape it"
    )]
    ForeignResourceHoldsRustValue {
        /// The resource whose value refused the migration.
        id: ResourceId,
    },

    /// The payload length does not match the foreign resource's declared size.
    #[message(
        "foreign resource ",
        debug_value(id),
        " takes ",
        debug_value(expected),
        " bytes; the payload has ",
        debug_value(actual)
    )]
    ForeignResourceBytesMismatch {
        /// The resource the payload was offered to.
        id: ResourceId,
        /// Declared size in bytes.
        expected: usize,
        /// Payload length in bytes.
        actual: usize,
    },

    /// A migration plan for a foreign resource falls outside its rows.
    #[message(
        "the migration plan for foreign resource ",
        debug_value(id),
        " reads or writes past the edge of a row"
    )]
    ForeignResourcePlanOutOfBounds {
        /// The resource the plan was built for.
        id: ResourceId,
    },

    /// A shared id holds a value of another shape, so removing it as `T` is
    /// refused instead of destroying what is there.
    ///
    /// A shared id is derived from a written-down name, which lets a Rust type
    /// that never registered reach another language's slot. The stored box
    /// refuses to hand out a value of a different type. The value, its ticks,
    /// its factory and its claim stay in place, so the caller learns why
    /// instead of losing the resource on the way to `None`.
    #[message(
        "shared resource ",
        debug_value(id),
        " does not hold a ",
        value(requested_type),
        "; a shared id is name-derived, so the removal is refused rather than destroying another type's value"
    )]
    SharedResourceHoldsAnotherType {
        /// The shared id the removal targeted.
        id: ResourceId,
        /// The Rust type the removal asked for.
        requested_type: &'static str,
    },

    /// Two different shared resource names hash to one identity.
    ///
    /// The id of a shared resource is a 128-bit hash of its name, and a hash
    /// collision would make one slot answer to two names - each declaration
    /// silently joining the other's resource. The id cannot carry the evidence,
    /// so each claim also stores its name and compares it with the recorded
    /// one.
    #[message(
        "shared resource names ",
        name_style(shared_name),
        " and ",
        name_style(existing_name),
        " hash to one identity; rename one of them"
    )]
    SharedResourceIdentityCollision {
        /// The name claiming the id now.
        shared_name: String,
        /// The name that already claimed it.
        existing_name: String,
    },

    /// One shared resource name, two different field shapes.
    ///
    /// Size and alignment cannot tell `{u32, u32}` from `{f32, f32}`; when both
    /// declarations carry a schema hash, the shapes can be compared and a
    /// reinterpretation becomes a refusal instead of a silent misread.
    #[message(
        "shared resource ",
        name_style(shared_name),
        " is declared with two different field shapes (existing hash ",
        value(existing_hash),
        ", incoming hash ",
        value(incoming_hash),
        "); rebuild every artifact that links it against one definition"
    )]
    SharedResourceSchemaMismatch {
        /// The shared name both registrations declared.
        shared_name: String,
        /// Schema hash recorded by the registration that got there first.
        existing_hash: u64,
        /// Schema hash of the type being registered now.
        incoming_hash: u64,
    },

    /// Two live registrations claim the same component type name under
    /// different [`ComponentId`]s.
    ///
    /// The persistable-registration path replaces a same-name entry in its
    /// persist maps when the host has announced that generation as superseded.
    /// Without that announcement, an older column that still holds entities is
    /// a concurrent peer - a second binary that linked the type and got its own
    /// `TypeId` - and replacing it would drop its rows at the next reload.
    ///
    /// Give the type a shared identity with `#[pill(shared)]` so both binaries
    /// resolve to one column instead of two.
    #[message(
        "component ",
        name_style(type_name),
        " is registered twice with different type identities (",
        debug_value(existing_id),
        " still holds ",
        value(live_rows),
        " live rows, and ",
        debug_value(incoming_id),
        " is registering now); declare it `#[pill(shared)]` so both registrations bind to one column"
    )]
    ComponentNameCollision {
        /// The type name both registrations claim.
        type_name: String,
        /// The already-registered id whose column still holds rows.
        existing_id: ComponentId,
        /// The id being registered now.
        incoming_id: ComponentId,
        /// How many rows the existing column still holds.
        live_rows: usize,
    },

    /// A persistable component was re-registered by a superseding generation
    /// with a layout the previous generation's column cannot host.
    ///
    /// The migration reads the rows the incoming generation spawned into the
    /// old column through the incoming type. Those rows sit in slots spaced for
    /// the old layout, so a wider alignment - or an old row size that is not a
    /// multiple of the new alignment - makes the reads misaligned. That is
    /// undefined behaviour, so the registration is refused and the host rolls
    /// the reload back.
    ///
    /// A size change that keeps the alignment is accepted, because the
    /// add-a-field reloads need that shape to migrate.
    #[message(
        "component ",
        name_style(type_name),
        " was re-registered with a different size or alignment (existing: ",
        value(existing_size),
        " bytes / ",
        value(existing_align),
        " align; incoming: ",
        value(incoming_size),
        " bytes / ",
        value(incoming_align),
        " align); keeping the running generation"
    )]
    ComponentLayoutChanged {
        /// The type name both registrations claim.
        type_name: String,
        /// Size of the layout the existing column was built for.
        existing_size: usize,
        /// Alignment of that layout.
        existing_align: usize,
        /// Size of the type being registered now.
        incoming_size: usize,
        /// Alignment of the type being registered now.
        incoming_align: usize,
    },

    /// Two different Rust types claim the same shared resource name.
    ///
    /// A shared name is a process-wide identity, so two types holding it are
    /// one resource: one slot, with whichever artifact inserts last replacing
    /// the other's value. When their layouts also agree, reads through either
    /// type succeed and quietly return the other resource's data.
    ///
    /// The case this must not reject is one type compiled into two artifacts,
    /// which is what a shared name is for. Those copies agree on the type's own
    /// name; two different types do not.
    #[message(
        "shared resource name ",
        name_style(shared_name),
        " is claimed by two different types (",
        name_style(existing_type),
        " and ",
        name_style(incoming_type),
        "); a shared name is a process-wide identity, so give them distinct names"
    )]
    SharedResourceNameConflict {
        /// The shared name both types declared.
        shared_name: String,
        /// Rust type name that claimed the name first.
        existing_type: String,
        /// Rust type name claiming it now.
        incoming_type: String,
    },

    /// One shared resource name, two different memory layouts.
    ///
    /// Every artifact reaches the resource through one slot, and this check is
    /// the only thing that proves they agree about its shape. Accepting the
    /// second registration would leave the two artifacts reading one slot with
    /// different layouts, so it is refused instead.
    #[message(
        "shared resource ",
        name_style(shared_name),
        " is registered with two different layouts (existing: ",
        value(existing_size),
        " bytes / ",
        value(existing_align),
        " align; incoming: ",
        value(incoming_size),
        " bytes / ",
        value(incoming_align),
        " align); rebuild every artifact that links it against one definition"
    )]
    SharedResourceLayoutMismatch {
        /// The shared name both registrations declared.
        shared_name: String,
        /// Size recorded by the registration that got there first.
        existing_size: usize,
        /// Alignment recorded by that first registration.
        existing_align: usize,
        /// Size of the type being registered now.
        incoming_size: usize,
        /// Alignment of the type being registered now.
        incoming_align: usize,
    },

    /// Two different Rust types claim the same shared component name.
    ///
    /// A shared name is a process-wide identity, so two types holding it are
    /// one component: one bit, one column, and every write through either type
    /// reaches the other's rows. When their layouts also match, the reads
    /// succeed and silently return another component's data.
    ///
    /// One type compiled into two binaries must not be rejected, because that
    /// is what shared identity is for. A name tells the two cases apart: the
    /// same type compiled twice reports the same short name even when its
    /// module paths differ. Only `#[pill(shared = "...")]` can produce the
    /// refused case, because a plain `#[pill(shared)]` derives the name from
    /// the module path and the type name.
    #[message(
        "shared component name ",
        name_style(shared_name),
        " is claimed by two different types (",
        name_style(existing_type),
        " and ",
        name_style(incoming_type),
        "); a shared name is a process-wide identity, so give them distinct names"
    )]
    SharedComponentNameConflict {
        /// The shared name both types declared.
        shared_name: String,
        /// Short Rust name of the type that registered the name first.
        existing_type: String,
        /// Short Rust name of the type claiming it now.
        incoming_type: String,
    },

    /// Two binaries registered the same shared component with different
    /// memory layouts.
    ///
    /// Every binary that links the component reaches it through one column, and
    /// this check is the only thing that proves they agree about what a row
    /// contains. Accepting the second registration would leave the two binaries
    /// reading and writing one column at different field offsets, so it is
    /// refused instead.
    ///
    /// Identity is resolved when the DLL loads, not by the compiler, so a field
    /// reorder or type change compiles cleanly in both binaries and shows up
    /// here. Rebuild both against the same definition.
    #[message(
        "shared component ",
        name_style(shared_name),
        " is registered with two different layouts (existing: ",
        value(existing_size),
        " bytes / ",
        value(existing_align),
        " align; incoming ",
        name_style(type_name),
        ": ",
        value(incoming_size),
        " bytes / ",
        value(incoming_align),
        " align); rebuild every binary that links it against one definition"
    )]
    SharedComponentLayoutMismatch {
        /// The shared name both registrations declared.
        shared_name: String,
        /// The Rust path of the type being registered now.
        type_name: String,
        /// Size recorded by the registration that got there first.
        existing_size: usize,
        /// Alignment recorded by that first registration.
        existing_align: usize,
        /// Size of the type being registered now.
        incoming_size: usize,
        /// Alignment of the type being registered now.
        incoming_align: usize,
    },

    /// A component type name resolved to more than one registered
    /// [`ComponentId`].
    ///
    /// A name is only useful when it identifies one column. More than one
    /// surviving candidate means [`WorldError::ComponentNameCollision`] was not
    /// enforced somewhere upstream, so this is reported instead of resolved by
    /// an arbitrary tiebreak that could pick the wrong column.
    #[message(
        "component name ",
        name_style(type_name),
        " resolves to ",
        value(count),
        " different registered components; it must resolve to exactly one"
    )]
    ComponentNameAmbiguous {
        /// The ambiguous type name.
        type_name: String,
        /// How many registered components claim it.
        count: usize,
    },

    /// Two imported asset types claim the same source file extension.
    ///
    /// A file's asset type is chosen by its extension, so a second claim would
    /// make the choice depend on which module registered first. Both
    /// [`World::register_imported_asset`](crate::World::register_imported_asset)
    /// and [`World::register_standalone_asset`](crate::World::register_standalone_asset)
    /// raise it.
    #[message(
        "imported asset type ",
        name_style(incoming_type),
        " claims source extension ",
        value(extension),
        ", which ",
        name_style(registered_type),
        " already imports"
    )]
    ImportExtensionClaimedTwice {
        /// The contested extension, lowercased, without the dot.
        extension: String,
        /// The type that already imports it.
        registered_type: String,
        /// The type that asked for it.
        incoming_type: String,
    },

    /// The archetype recorded for an entity is absent from the world.
    ///
    /// An entity location outlived the archetype it points at, which a partly
    /// applied hot-reload rehome can produce.
    #[message(
        "entity ",
        debug_value(entity),
        " references archetype ",
        debug_value(archetype_id),
        " which is missing from the world"
    )]
    ArchetypeMissing {
        /// The entity whose recorded location is stale.
        entity: Entity,
        /// The archetype ID that could not be resolved.
        archetype_id: ArchetypeId,
    },
}

// =============================================================================
// Refusal Messages
// =============================================================================

/// The refusal logged when two live registrations claim one component type
/// name.
///
/// This is the one place the sentence lives. The log line that reports a
/// [`WorldError::ComponentNameCollision`] uses this constant, so a second guard
/// site cannot invent its own wording. It must not contain a double space or a
/// line break, because the message is read verbatim in host logs.
pub(crate) const COMPONENT_NAME_COLLISION_REFUSAL: &str =
    "two live registrations claim one component type name; refusing to evict the peer's persist entries";

/// The refusal logged when a reloading generation re-registers a persistable
/// component with a layout the previous generation's column cannot host.
///
/// Written the same way as [`COMPONENT_NAME_COLLISION_REFUSAL`]: the log line
/// that reports a [`WorldError::ComponentLayoutChanged`] uses this constant,
/// and the reload migration suite matches its text.
pub(crate) const COMPONENT_LAYOUT_CHANGED_REFUSAL: &str =
    "was re-registered with a different size or alignment; keeping the running generation";

// =============================================================================
// Command Errors
// =============================================================================

/// Error returned when a deferred command cannot be executed.
///
/// Command errors are non-fatal by default: the engine logs them and keeps
/// going. Call `Engine::set_should_exit_on_error(true)` and
/// `Engine::process_frame` returns every failure to its caller instead.
#[engine_error(namespace = engine::commands, runtime = ::pill_core::error)]
pub enum CommandError {
    /// The target entity no longer exists in the world.
    #[message("entity ", debug_value(entity), " not found for ", value(operation))]
    EntityNotFound {
        entity: Entity,
        operation: &'static str,
    },

    /// A queued command could not write one of its components.
    ///
    /// The component was validated when the command was queued, but the world
    /// can change before the queue is flushed. A hot reload that retires a
    /// component type between a managed `CreateEntity` call and the end of the
    /// frame leaves the queued command naming storage that no longer exists.
    /// The failure is collected as an error instead of panicking, because a
    /// panic here would unwind through the C ABI of a managed project.
    #[message(
        "entity ",
        debug_value(entity),
        " was created but component ",
        debug_value(component_id),
        " could not be written: ",
        value(reason)
    )]
    ComponentWriteFailed {
        entity: Entity,
        component_id: ComponentId,
        /// The failure the write caught, so the report says why. Built only on
        /// a failed command, never on the frame path.
        reason: String,
    },

    /// The queued creation listed one component id more than once.
    ///
    /// The id set defines the archetype's columns. A duplicate would push two
    /// rows for one entity row, or make the archetype insert panic when it
    /// finds the id already present. The command is refused before the entity
    /// row exists, so a rejected command leaves no partial entity.
    #[message(
        "entity ",
        debug_value(entity),
        " was asked to carry component ",
        debug_value(component_id),
        " twice"
    )]
    DuplicateComponent {
        entity: Entity,
        component_id: ComponentId,
    },

    /// The entity already possesses the component being added.
    #[message(
        "entity ",
        debug_value(entity),
        " already has component ",
        debug_value(component_id)
    )]
    ComponentAlreadyExists {
        entity: Entity,
        component_id: ComponentId,
    },

    /// The entity does not have the component being removed.
    #[message(
        "entity ",
        debug_value(entity),
        " does not have component ",
        debug_value(component_id)
    )]
    ComponentNotFound {
        entity: Entity,
        component_id: ComponentId,
    },

    /// A queued structural edit failed inside the world for a reason the flush
    /// has no more specific command error for.
    ///
    /// A storage column missing from an archetype, or a column whose layout no
    /// longer matches the manifest, is what a partly applied hot reload leaves
    /// behind. These failures are collected like any other command failure
    /// instead of panicking inside the flush, which for a managed project would
    /// unwind across the C ABI.
    #[message("entity ", debug_value(entity), " migration failed: ", value(reason))]
    MigrationFailed { entity: Entity, reason: String },
}

// =============================================================================
// Component Add/Remove/Builder Errors
// =============================================================================

/// Error type for `add_component` operations.
///
/// Raised when the target entity is missing, already carries the component
/// being added, or when the component type was never registered with this
/// world.
#[engine_error(namespace = engine::world, runtime = ::pill_core::error)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AddComponentError {
    /// The entity does not exist (was destroyed or never created).
    #[message("entity not found")]
    EntityNotFound,

    /// The entity already has a component of this type.
    #[message("component already exists on entity")]
    ComponentAlreadyExists,

    /// The component type was never registered with this world.
    ///
    /// Registration is what gives a component its archetype bit, so an
    /// unregistered type would resolve its destination archetype back to the
    /// entity's current one - storing nothing while reporting success.
    #[message(
        "component ",
        debug_value(id),
        " not registered — call world.register_component::<T>() first"
    )]
    ComponentNotRegistered { id: ComponentId },
}

/// Error type for `remove_component` operations.
///
/// Returned by the typed component removal paths when the target entity
/// is missing or does not carry the component being removed.
#[engine_error(namespace = engine::world, runtime = ::pill_core::error)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RemoveComponentError {
    /// The entity does not exist (was destroyed or never created).
    #[message("entity not found")]
    EntityNotFound,

    /// The entity does not have a component of this type.
    #[message("component not found on entity")]
    ComponentNotFound,
}

/// Error type for `EntityBuilder::build`.
///
/// Returned when the builder writes a component type the world has never seen,
/// when the same component type is handed to it twice, or when the requested
/// entity id already belongs to a live entity. An unregistered type must be
/// registered with `world.register_component::<T>()` first.
#[engine_error(namespace = engine::world, runtime = ::pill_core::error)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BuildError {
    /// One or more component types were not registered with the world.
    /// Call `world.register_component::<T>()` for each type first.
    #[message(
        "component ",
        debug_value(id),
        " not registered — call world.register_component::<T>() first"
    )]
    ComponentNotRegistered { id: ComponentId },

    /// The stable id handed to `EntityBuilder::with_id` already belongs to a
    /// live entity.
    ///
    /// Two entities under one id would make a saved reference ambiguous, so
    /// the second build is refused rather than merged into the first.
    #[message("entity id ", debug_value(id), " already belongs to a live entity")]
    DuplicateEntityId { id: crate::entity::EntityId },

    /// One component type was handed to `EntityBuilder::with` more than once.
    ///
    /// The component set defines the archetype's columns, so a second value of
    /// the same type would grow one column by two rows for one entity row and
    /// leave every later entity reading the wrong one.
    #[message(
        "component ",
        debug_value(id),
        " was handed to the entity builder twice"
    )]
    DuplicateComponent { id: ComponentId },
}

// =============================================================================
// Persistence Errors
// =============================================================================

/// Migration failures of persistable component columns.
///
/// Raised while moving persisted component data from one schema to another: a
/// type that is unregistered or ambiguous, a missing deserializer, serializer,
/// inserter or storage factory, a column that cannot hand its rows to the
/// destination archetype, or bytes that fail to decode into the new layout.
#[engine_error(namespace = engine::persistence, runtime = ::pill_core::error)]
pub enum PersistenceError {
    /// The component type is not registered in the current world.
    #[message(
        "component type ",
        name_style(type_name),
        " is not registered in the current world"
    )]
    ComponentTypeUnregistered { type_name: String },

    /// The component type name resolves to more than one registered
    /// component, so migration cannot tell which column owns the rows.
    ///
    /// The persistable-registration path removes superseded same-name entries
    /// so this cannot happen. Reaching it means two concurrent peers registered
    /// the name and the collision guard was bypassed.
    #[message(
        "component type ",
        name_style(type_name),
        " resolves to ",
        value(count),
        " registered components, so the rows to migrate are ambiguous"
    )]
    ComponentTypeAmbiguous {
        /// The ambiguous type name.
        type_name: String,
        /// How many registered components claim it.
        count: usize,
    },

    /// No deserializer is registered for the component type.
    #[message(
        "no deserializer is registered for component type ",
        name_style(type_name)
    )]
    DeserializerMissing { type_name: String },

    /// No serializer is registered for the component type.
    ///
    /// Migration needs one to carry the entities the incoming generation
    /// spawned during `init` across a rebuilt column, without reading their
    /// bytes through the retiring generation's layout.
    #[message(
        "no serializer is registered for component type ",
        name_style(type_name)
    )]
    SerializerMissing { type_name: String },

    /// No inserter is registered for the component type.
    #[message("no inserter is registered for component type ", name_style(type_name))]
    InserterMissing { type_name: String },

    /// The persisted bytes could not be decoded into the new schema.
    #[message("deserialization failed for component ", debug_value(component_id))]
    DeserializationFailed { component_id: ComponentId },

    /// A value could not be encoded, and the type's own `Serialize` says why.
    ///
    /// The inverse of [`Self::DeserializationFailed`], reported the same way:
    /// the pass records the type as skipped and leaves the world as it found
    /// it, instead of aborting a reload halfway through its transaction.
    #[message("serialization failed for component ", debug_value(component_id))]
    SerializationFailed { component_id: ComponentId },

    /// The old component column could not be removed from its archetype.
    #[message(
        "removing the old storage of component ",
        debug_value(component_id),
        " failed"
    )]
    StorageRemovalFailed { component_id: ComponentId },

    /// No storage factory is registered for the component.
    #[message(
        "no storage factory is registered for component ",
        debug_value(component_id)
    )]
    StorageFactoryMissing { component_id: ComponentId },

    /// The component has no native Rust storage to migrate.
    #[message(
        "component ",
        debug_value(component_id),
        " has no native Rust storage to migrate"
    )]
    NativeStorageExpected { component_id: ComponentId },

    /// An unchanged column could not hand its row to the destination.
    ///
    /// The cross-archetype migration moves every column it does not rewrite
    /// straight into the destination archetype. A destination that has no
    /// column for one of them, or one whose rows have a different width, is the
    /// registry/storage desync a partly applied reload leaves behind. The
    /// migration reports it instead of leaving the entity split between two
    /// archetypes.
    #[message(
        "component ",
        debug_value(component_id),
        " could not be moved into the destination archetype"
    )]
    ColumnMoveFailed { component_id: ComponentId },

    /// The destination archetype vanished immediately after creation.
    #[message("destination archetype missing after creation")]
    DestinationArchetypeMissing,
}

// =============================================================================
// System Errors
// =============================================================================

/// Failure reported by one scheduler system during a frame.
///
/// Systems return `Result<(), SystemError>`. The engine records every `Err`
/// with the failing system's name, and the reporting boundary reads the batch
/// through [`Engine::drain_system_failures`](crate::Engine::drain_system_failures).
/// A failed system never aborts the remaining batch or the frame.
#[engine_error(namespace = engine::systems, runtime = ::pill_core::error)]
pub enum SystemError {
    /// A resource the system requested is missing from the world.
    #[message("resource ", name_style(name), " is missing from the world")]
    MissingResource { name: String },

    /// A managed-language system reported failure through the interop bridge.
    #[message("managed failure: ", value(message))]
    Managed { message: String },

    /// A system failed with a message it chose itself.
    #[message(value(message))]
    Failure { message: String },
}

/// One failed system execution recorded during a frame.
///
/// Pairs the failing system's name with its [`SystemError`] and renders both
/// through the shared semantic protocol, so a reporter can log
/// `system <name> failed: <error>` in plain or styled form.
#[derive(Debug)]
pub struct SystemFailure {
    /// Registered name of the system that failed.
    pub system: String,
    /// The error the system returned.
    pub error: SystemError,
}

impl SystemFailure {
    /// Pairs one failing system's name with the error it returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use pill_engine::{SystemError, SystemFailure};
    ///
    /// let failure = SystemFailure::new(
    ///     String::from("ball_physics"),
    ///     SystemError::MissingResource {
    ///         name: String::from("SimulationTime"),
    ///     },
    /// );
    /// assert_eq!(
    ///     failure.to_string(),
    ///     "system ball_physics failed: resource SimulationTime is missing from the world"
    /// );
    /// ```
    pub fn new(system: String, error: SystemError) -> Self {
        Self { system, error }
    }
}

impl EngineMessage for SystemFailure {
    fn render_message(&self, renderer: &mut dyn MessageRenderer) -> fmt::Result {
        renderer.text("system ")?;
        renderer.styled(SemanticRole::Name, &self.system)?;
        renderer.text(" failed: ")?;
        self.error.render_message(renderer)
    }
}

impl fmt::Display for SystemFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.to_plain_message())
    }
}

// =============================================================================
// Engine Error Composition
// =============================================================================

/// Transparent composition of the engine errors that cross a subsystem
/// boundary, plus the shared host errors from `pill_core`.
///
/// Subsystem functions return their narrowest meaningful error, which is what
/// most callers want. This type is the one boundary where `?` can carry any of
/// them, for host code that prefers a single error type.
#[engine_error(namespace = engine, runtime = ::pill_core::error)]
pub enum EngineError {
    /// A world storage or migration operation failed.
    #[transparent]
    World(#[from] WorldError),

    /// A deferred command could not be applied.
    #[transparent]
    Commands(#[from] CommandError),

    /// A persisted component migration failed.
    #[transparent]
    Persistence(#[from] PersistenceError),

    /// A subsystem of the shared host crate failed.
    #[transparent]
    Host(#[from] ::pill_core::error::HostError),
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use miette::Diagnostic as _;

    /// The plain form of a world error keeps its message text.
    #[test]
    fn world_error_renders_plain_with_values() {
        let error = WorldError::DescriptorComponentNotRegistered {
            id: ComponentId::descriptor(7),
        };
        assert!(error.to_plain_message().contains("descriptor component"));
        assert!(error.to_plain_message().contains("is not registered"));
    }

    /// Command errors derive their diagnostic code from namespace and variant.
    #[test]
    fn command_error_code_derives_from_namespace_and_variant() {
        let error = CommandError::EntityNotFound {
            entity: Entity::new_for_test(3, 1),
            operation: "destroy",
        };
        assert_eq!(
            error.code().map(|code| code.to_string()).as_deref(),
            Some("engine::commands::entity_not_found")
        );
    }

    /// Transparent composition preserves the leaf message end to end.
    #[test]
    fn engine_error_composition_keeps_the_leaf_message() {
        let world_error = WorldError::EntityNotFound;
        let engine_error: EngineError = world_error.into();
        assert_eq!(engine_error.to_string(), "entity not found");
        assert_eq!(engine_error.to_plain_message(), "entity not found");
    }

    /// The entity builder error stays `Copy`, so passing it on does not consume
    /// it.
    #[test]
    fn build_error_remains_copy() {
        let error = BuildError::ComponentNotRegistered {
            id: ComponentId::descriptor(9),
        };
        let copied = error;
        assert_eq!(error, copied);
    }

    /// System failures render the failing system's name with its error.
    #[test]
    fn system_failure_renders_name_and_leaf_message() {
        let failure = SystemFailure::new(
            String::from("ball_physics"),
            SystemError::MissingResource {
                name: String::from("SimulationTime"),
            },
        );
        assert_eq!(
            failure.to_plain_message(),
            "system ball_physics failed: resource SimulationTime is missing from the world"
        );
        assert_eq!(failure.to_string(), failure.to_plain_message());
    }

    /// Managed failures keep the managed-reported message verbatim.
    #[test]
    fn managed_system_error_keeps_the_managed_message() {
        let error = SystemError::Managed {
            message: String::from("index out of range"),
        };
        assert_eq!(
            error.to_plain_message(),
            "managed failure: index out of range"
        );
        assert_eq!(
            error.code().map(|code| code.to_string()).as_deref(),
            Some("engine::systems::managed")
        );
    }
}
