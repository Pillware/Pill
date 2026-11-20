//! Managed resource registration, and the callback that hands resource bytes
//! to managed code.
//!
//! # Responsibilities
//!
//! - Register every resource a managed manifest declares as a foreign engine
//!   resource, and record the binding managed code reaches it through.
//! - Serve [`ResourceView`]s to `Res<T>` and `ResMut<T>` under the same scope
//!   and access-declaration rules the query callbacks follow.
//! - Provide the resource half of the shared reload pipeline as
//!   [`RESOURCE_SUBJECT`]. The stages it runs through live in
//!   [`manifest_apply`](super::manifest_apply).
//!
//! # Design
//!
//! A managed resource is blittable bytes with a declared name, which is exactly
//! what [`World::register_foreign_resource`] accepts. The manifest carries
//! resources in the same array as components and marks each entry with a `kind`
//! tag, so one transfer covers both kinds.
//!
//! The table of resource bindings is a process-wide `RwLock`. The component
//! bindings instead travel as a pointer installed for each managed invocation
//! ([`ComponentBindings`](super::components::ComponentBindings)). A resource
//! table is process-wide because a world holds one value of each resource, not
//! one per archetype. Keeping the table behind the lock also means the scope
//! struct, and every installer that fills it, stay unchanged.
//!
//! The reload protocol is the components' protocol, run from one place. A
//! resource owns no column, so its migration moves a stored value instead of
//! rewriting a column. That move cannot be reversed, so the subject records no
//! undo for it.

// Standard library
use std::collections::HashMap;

#[cfg(test)]
use std::cell::RefCell;
#[cfg(not(test))]
use std::sync::{OnceLock, RwLock};

// External crates
use pill_core::error::{CSharpError, EngineMessage};
use pill_core::telemetry::telemetry_target;
use pill_core::{error, info, warn};
use pill_engine::archetype::{FieldPlan, LayoutField};
use pill_engine::component_registry::{ComponentFieldDescriptor, FieldDecorator};
use pill_engine::{Engine, ResourceId, World};

// Current crate
use super::abi::ResourceView;
use super::components::{stable_component_id, StableComponentId};
use super::context::{resource_access_is_authorized, with_active_world, ACCESS_KIND_RESOURCE};
use super::manifest::intern;
use super::manifest_apply::{ManifestOutcome, ManifestSubject, RenamePredecessor, Settlement};

// =============================================================================
// Types
// =============================================================================

/// One managed resource, bound to its engine registration.
///
/// Every managed resource is foreign storage: the host has no Rust type for it.
/// That is why there is one kind of binding here where a component has two.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
#[derive(Debug, Clone)]
pub(super) struct ResourceBinding {
    /// Engine id the resource was registered under.
    pub(super) resource_id: ResourceId,
    /// The declaration the registration was made from.
    ///
    /// This is the only record of the resource's shape: name, size, alignment,
    /// schema hash and field layout. A reload compares, migrates and rolls back
    /// against it, so those steps never read a second copy that could disagree
    /// with what was registered.
    pub(super) declaration: ManagedResourceDeclaration,
}

/// One field of a managed resource, as the manifest describes it.
///
/// The names are owned `String`s, where a component field descriptor holds an
/// interned `&'static str`. A resource layout is kept for exactly as long as
/// its binding, so there is no longer-lived value to intern a name against.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
#[derive(Debug, Clone)]
pub(super) struct ResourceFieldLayout {
    /// Field name. A reload matches the two layouts on it.
    pub(super) name: String,
    /// Engine type tag, already reduced to the form a migration plan compares.
    pub(super) type_tag: String,
    /// Byte offset of the field within the resource.
    pub(super) offset: usize,
    /// Byte width of the field.
    pub(super) size: usize,
    /// Bytes of the declared default for this field, already parsed from the
    /// field's declared type. A reload writes them when it adds the field or
    /// resets it.
    pub(super) default: Option<Vec<u8>>,
    /// Presentation decorators the field declares, in declaration order.
    ///
    /// Owned for the same reason the names are: the layout lives exactly as
    /// long as its binding, so there is nothing to borrow the list from.
    pub(super) decorations: Vec<FieldDecorator>,
}

/// Every managed resource, keyed by the stable identity of its full name.
pub(super) type ResourceBindings = HashMap<StableComponentId, ResourceBinding>;

/// One resource entry taken from the managed manifest.
///
/// A resource's manifest entry is a component entry without the parts that only
/// mean something for a column. It is carried as its own small struct rather
/// than reusing the component entry and leaving fields unread.
#[derive(Debug, Clone)]
pub(super) struct ManagedResourceDeclaration {
    /// Full managed type name, which is also the resource's engine name.
    pub(super) full_name: String,
    /// Width of the declared layout in bytes.
    pub(super) size: usize,
    /// Alignment of the declared layout in bytes.
    pub(super) align: usize,
    /// Hash of the managed field schema.
    pub(super) schema_hash: u64,
    /// The names of live registrations this resource renames onto itself. A
    /// rename migrates the stored value instead of refusing the manifest.
    pub(super) aliases: Vec<String>,
    /// The declared field layout, which plans a reload's byte migration.
    pub(super) fields: Vec<ResourceFieldLayout>,
}

/// The live table of resource bindings, shared by registration and the
/// callback.
///
/// A host keeps one table for the whole process. A world holds one value per
/// resource, so registration, reload and the `Res<T>` callback must all see
/// the same bindings.
///
/// Under `cargo test` the table is thread-local. Every manifest apply replaces
/// the whole table, and cargo runs tests on parallel threads in one process, so
/// a component-only manifest in one test would otherwise erase a resource
/// test's bindings while it runs. A test registers, acts and asserts on one
/// thread, so a table per thread gives each test what a host gives the process.
#[cfg(not(test))]
static RESOURCE_BINDINGS: OnceLock<RwLock<ResourceBindings>> = OnceLock::new();

#[cfg(test)]
thread_local! {
    static RESOURCE_BINDINGS: RefCell<ResourceBindings> = RefCell::new(ResourceBindings::new());
}

/// Runs `body` with the live table.
///
/// A panic while the lock is held cannot leave the map wrong: it holds plain
/// data, and a writer finishes or abandons one whole entry. A poisoned lock is
/// therefore treated as healthy rather than becoming a new failure for every
/// later resource access. `BindingStore` follows the same rule.
#[cfg(not(test))]
fn with_bindings<R>(body: impl FnOnce(&ResourceBindings) -> R) -> R {
    let table = RESOURCE_BINDINGS.get_or_init(|| RwLock::new(HashMap::new()));
    let bindings_guard = table
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    body(&bindings_guard)
}

/// Runs `body` with this test's table; see `RESOURCE_BINDINGS`.
#[cfg(test)]
fn with_bindings<R>(body: impl FnOnce(&ResourceBindings) -> R) -> R {
    RESOURCE_BINDINGS.with(|table| body(&table.borrow()))
}

/// Replaces the live table with one generation's bindings.
///
/// It replaces rather than merges, because a reload's manifest is the complete
/// list of what the arriving generation declares, so a resource it dropped
/// must stop resolving. This function does not touch the engine's storage: a
/// stored value stays until `drop_resources` releases it, so replacing the
/// table only stops managed code from resolving the resource.
#[cfg(not(test))]
fn publish_bindings(bindings: ResourceBindings) {
    let table = RESOURCE_BINDINGS.get_or_init(|| RwLock::new(HashMap::new()));
    let mut bindings_guard = table
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *bindings_guard = bindings;
}

/// Replaces this test's table; see `RESOURCE_BINDINGS`.
#[cfg(test)]
fn publish_bindings(bindings: ResourceBindings) {
    RESOURCE_BINDINGS.with(|table| *table.borrow_mut() = bindings);
}

// =============================================================================
// Registration
// =============================================================================

/// Registers every resource the managed manifest declares.
///
/// It takes the entries a manifest parse separated out. The new table is
/// published once at the end, so one generation's whole set of bindings
/// becomes visible together.
///
/// Each resource is registered twice on purpose: once as a foreign resource, so
/// the scheduler and managed code can reach it, and once as persistable, so it
/// appears in a snapshot. A foreign resource has no Rust type to derive
/// persistence from, and its bytes are exactly what a snapshot can carry.
///
/// # Errors
///
/// Returns a [`CSharpError`] when a declared layout cannot describe an
/// allocation, when the name is already claimed by a Rust resource of a
/// different shape, or when a Rust value is already stored under the identity.
pub(super) fn register_resource_manifest(
    engine: &mut Engine,
    resources: &[ManagedResourceDeclaration],
) -> Result<(), CSharpError> {
    let mut bindings = ResourceBindings::new();
    for resource in resources {
        let resource_id = register_declared_resource(engine, resource)?;
        bindings.insert(
            stable_component_id(&resource.full_name),
            binding_for_declaration(resource_id, resource),
        );
    }
    publish_bindings(bindings);
    Ok(())
}

/// Empties this thread's table, so one test's registrations cannot reach
/// another's.
///
/// The table is thread-local under `cargo test` by design, but cargo reuses
/// threads across tests, so each resource test starts by resetting the table it
/// is about to write. See `RESOURCE_BINDINGS`.
#[cfg(test)]
pub(super) fn reset_resource_bindings_for_test() {
    publish_bindings(ResourceBindings::new());
}

/// Returns the engine id and byte width of one bound resource.
///
/// The hot path needs only these two values. Cloning the binding to read them
/// would copy the field layout on every `Res<T>` access.
pub(super) fn bound_resource_id_and_width(key: StableComponentId) -> Option<(ResourceId, usize)> {
    with_bindings(|table| {
        table
            .get(&key)
            .map(|binding| (binding.resource_id, binding.declaration.size))
    })
}

/// Resolves this manifest's resource entries without touching the engine.
///
/// A caller that has both kinds at hand resolves resources *and* components
/// through this, before either one mutates anything: a refusal about one kind's
/// shape must not leave the other kind's relayout already applied.
///
/// # Errors
///
/// Returns a [`CSharpError`] when one entry has more than one alias that
/// resolves to a live registration, and when an entry the table already holds
/// also declares an alias. Both checks run during resolution, so both fail
/// before anything is mutated.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) fn prepare_resource_manifest<'declaration>(
    engine: &Engine,
    resources: &'declaration [ManagedResourceDeclaration],
) -> Result<
    super::manifest_apply::PreparedManifest<
        'declaration,
        ManagedResourceDeclaration,
        ResourceBinding,
    >,
    CSharpError,
> {
    let previous = with_bindings(|table| table.clone());
    super::manifest_apply::prepare_manifest(&RESOURCE_SUBJECT, engine, previous, resources)
}

/// Applies what [`prepare_resource_manifest`] resolved, then publishes the
/// table.
///
/// For each resource the pipeline picks one of five outcomes, comparing the
/// arriving declaration with the previous binding:
///
/// - unchanged: the schema hash matches, so the stored bytes stay;
/// - added;
/// - reshaped: the bytes migrate field by field, matched by name;
/// - renamed through an alias;
/// - retired.
///
/// Aliases are handled first and retirement last.
///
/// The table is published on every exit, including a failing one. A reshape
/// that already ran cannot be undone, so keeping its new binding in the table
/// is what makes the table describe the layout the engine really holds. A
/// failed reshape is logged as a failure that needs a host restart.
///
/// # Errors
///
/// Returns the [`CSharpError`] the engine reports when it refuses a
/// registration or a byte migration. Entries applied before the refusal are
/// rolled back where an undo exists; a reshape stays applied, as described
/// above.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) fn execute_resource_manifest(
    engine: &mut Engine,
    prepared: super::manifest_apply::PreparedManifest<
        '_,
        ManagedResourceDeclaration,
        ResourceBinding,
    >,
) -> Result<ManifestOutcome, CSharpError> {
    let (result, table) =
        super::manifest_apply::execute_manifest(&RESOURCE_SUBJECT, engine, prepared);
    publish_bindings(table);
    result
}

// =============================================================================
// The resource manifest subject
// =============================================================================

/// One applied resource entry, recorded so a later refusal can undo it.
///
/// Only a rename appears here. An addition needs no undo: the engine keeps the
/// registration and the table describes it. A reshape cannot be undone: it
/// moves bytes into a possibly narrower shape, and a reverse move would have to
/// invent the bytes it dropped. Neither has a variant for that reason.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) enum ResourceUndo {
    /// The entry renamed a resource from an earlier declaration.
    Renamed {
        /// The registration the rename retired, to be registered again.
        predecessor: ResourceBinding,
        /// The successor's id, whose value moves back to the predecessor.
        successor_id: ResourceId,
        /// The shape the successor was registered with. It is the source side
        /// of the plan that moves the value back.
        successor_fields: Vec<ResourceFieldLayout>,
    },
}

/// The actions the shared apply pipeline needs for managed resources.
///
/// A resource owns no column and no archetype, so its "reshape" migrates one
/// stored value where a component's rewrites a column. The subject exists to
/// carry exactly this kind of difference.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) const RESOURCE_SUBJECT: ManifestSubject<
    ManagedResourceDeclaration,
    ResourceBinding,
    ResourceUndo,
> = ManifestSubject {
    kind: "resource",
    identity: |resource| stable_component_id(&resource.full_name),
    name: |resource| resource.full_name.as_str(),
    aliases: |resource| &resource.aliases,
    binding_name: |_engine, binding| binding.declaration.full_name.clone(),
    // Every entry in the resource table is a managed declaration. Components
    // share their map with a native lane; resources have none.
    is_governed: |_binding| true,
    // A resource alias names a managed declaration, whose identity is the hash
    // of its name, so the table answers directly. An engine lookup is neither
    // needed nor meaningful.
    resolve_alias: |_engine, table, alias| {
        let predecessor = stable_component_id(alias);
        Ok(table.contains_key(&predecessor).then_some(predecessor))
    },
    // A declared resource can always be registered: it carries its own layout
    // and needs no counterpart on the Rust side.
    check_addable: |_resource| Ok(()),
    settle: |_engine, binding, resource| {
        Ok(if binding.declaration.schema_hash == resource.schema_hash {
            Settlement::Unchanged
        } else {
            Settlement::Reshaped
        })
    },
    register: |engine, _identity, resource| {
        let resource_id = register_declared_resource(engine, resource)?;
        Ok((binding_for_declaration(resource_id, resource), None))
    },
    reshape: |engine, _identity, existing, resource| {
        match relayout_declared_resource(engine, existing, resource) {
            Ok(()) => Ok((
                binding_for_declaration(existing.resource_id, resource),
                None,
            )),
            Err(error) => {
                // The arriving assembly is refused after this returns. The
                // generation still running then holds the old struct, while the
                // engine's layout for that resource may already have moved.
                // Nothing here can put that back, so the failure is logged
                // instead of being left to show up later as a wrong value.
                error!(
                    target: telemetry_target::HOT_RELOAD,
                    resource = %resource.full_name,
                    error = %error.to_plain_message(),
                    "managed resource migration failed part-way; restart the host before trusting resource values"
                );
                Err(error)
            }
        }
    },
    rename: rename_resource_entry,
    retire: retire_resources,
    undo: undo_resource_entry,
};

/// A shipping build never applies a manifest, so it refuses a rename rather
/// than moving the value onto a successor.
#[cfg(not(feature = "hot_reload"))]
fn rename_resource_entry(
    _engine: &mut Engine,
    _identity: StableComponentId,
    _predecessor: &RenamePredecessor<'_, ResourceBinding>,
    resource: &ManagedResourceDeclaration,
) -> Result<(ResourceBinding, Option<ResourceUndo>), CSharpError> {
    Err(CSharpError::ManifestInvalid {
        message: format!(
            "managed resource {} was renamed; a shipping build cannot move storage",
            resource.full_name
        ),
    })
}

/// Registers the successor of a renamed resource and moves the value across.
///
/// # Errors
///
/// Returns a [`CSharpError`] when the engine refuses the new registration, or
/// when it refuses to move the stored bytes onto it.
#[cfg(feature = "hot_reload")]
fn rename_resource_entry(
    engine: &mut Engine,
    _identity: StableComponentId,
    predecessor: &RenamePredecessor<'_, ResourceBinding>,
    resource: &ManagedResourceDeclaration,
) -> Result<(ResourceBinding, Option<ResourceUndo>), CSharpError> {
    let resource_id = register_declared_resource(engine, resource)?;
    let plan = resource_field_plan(&predecessor.binding.declaration.fields, &resource.fields);
    let retyped = plan.retyped_fields().to_vec();
    engine
        .world_mut()
        .remap_foreign_resource(predecessor.binding.resource_id, resource_id, &plan)
        .map_err(|error| CSharpError::ManifestInvalid {
            message: error.to_plain_message(),
        })?;
    if !retyped.is_empty() {
        warn!(
            target: telemetry_target::HOT_RELOAD,
            resource = %resource.full_name,
            fields = %retyped.join(", "),
            "managed resource fields changed type and were reset to their zero bytes"
        );
    }
    info!(
        target: telemetry_target::HOT_RELOAD,
        resource = %resource.full_name,
        predecessor = %predecessor.alias,
        "managed resource renamed and its value migrated"
    );
    Ok((
        binding_for_declaration(resource_id, resource),
        Some(ResourceUndo::Renamed {
            predecessor: predecessor.binding.clone(),
            successor_id: resource_id,
            successor_fields: resource.fields.clone(),
        }),
    ))
}

/// Drops the resources the manifest stopped naming.
///
/// `drop_resources` skips an id another artifact still claims, so a shared
/// resource that another module also declares keeps its value, and only this
/// table's binding disappears. The drop runs before the assembly that declared
/// the value is unmapped, which is the timing `drop_resources` requires.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
fn retire_resources(engine: &mut Engine, bindings: &[ResourceBinding]) -> usize {
    let resource_ids: Vec<ResourceId> =
        bindings.iter().map(|binding| binding.resource_id).collect();
    engine.world_mut().drop_resources(&resource_ids)
}

/// A shipping build cannot reach an undo: no manifest was applied to reverse.
#[cfg(not(feature = "hot_reload"))]
fn undo_resource_entry(_engine: &mut Engine, _undo: ResourceUndo) {}

/// Moves a renamed resource's value back onto its predecessor, best effort.
///
/// A failed rollback leaves the process mixed. The failure is therefore logged
/// with the resource it concerns, rather than returned in place of the original
/// refusal.
#[cfg(feature = "hot_reload")]
fn undo_resource_entry(engine: &mut Engine, undo: ResourceUndo) {
    let ResourceUndo::Renamed {
        predecessor,
        successor_id,
        successor_fields,
    } = undo;
    let predecessor_name = predecessor.declaration.full_name.clone();
    // Step 1: Rebuild the predecessor's declaration. Its own declaration
    // restores the registration; the alias list the arriving generation
    // carried is not part of the shape.
    let mut declaration = predecessor.declaration;
    declaration.aliases = Vec::new();
    // Step 2: Register the predecessor again under its old name.
    let predecessor_resource_id = match register_declared_resource(engine, &declaration) {
        Ok(resource_id) => resource_id,
        Err(error) => {
            error!(
                target: telemetry_target::HOT_RELOAD,
                resource = %predecessor_name,
                error = %error.to_plain_message(),
                "could not re-register a renamed resource's predecessor during rollback"
            );
            return;
        }
    };
    // Step 3: Move the value back. The plan runs backwards: the successor's
    // declared shape is the source, and the predecessor's remembered shape is
    // the destination.
    let plan = resource_field_plan(&successor_fields, &declaration.fields);
    if let Err(error) =
        engine
            .world_mut()
            .remap_foreign_resource(successor_id, predecessor_resource_id, &plan)
    {
        error!(
            target: telemetry_target::HOT_RELOAD,
            resource = %predecessor_name,
            error = %error.to_plain_message(),
            "could not move a renamed resource's value back during rollback"
        );
        return;
    }
    info!(
        target: telemetry_target::HOT_RELOAD,
        resource = %predecessor_name,
        "rolled back a managed resource rename"
    );
}

/// Registers one declared resource and opts it into snapshots.
///
/// A field layout the engine refuses is only logged, because the resource is
/// registered and usable by then.
///
/// # Errors
///
/// Returns a [`CSharpError`] when the engine refuses the registration, or when
/// it refuses to store the zero seed.
fn register_declared_resource(
    engine: &mut Engine,
    resource: &ManagedResourceDeclaration,
) -> Result<ResourceId, CSharpError> {
    // Step 1: Register the foreign resource under its full managed name.
    let resource_id = engine
        .world_mut()
        .register_foreign_resource(
            &resource.full_name,
            &resource.full_name,
            resource.size,
            resource.align,
            resource.schema_hash,
        )
        .map_err(|error| CSharpError::ManifestInvalid {
            message: error.to_plain_message(),
        })?;
    // Step 2: Opt the resource into snapshots. Doing it here, rather than at
    // the manifest site, keeps the two records of "this resource exists"
    // together. Only a registration the engine accepted is worth persisting.
    engine
        .world_mut()
        .register_persistable_foreign_resource(&resource.full_name);
    publish_field_layout(engine, resource_id, resource);

    // Step 3: Seed the value, because a managed declaration is all there is.
    // Nothing on the managed side inserts a resource the way a Rust `init`
    // calls `insert_resource`, so without this the first `Res<T>` access would
    // find the resource registered but holding nothing. Zero is the right seed
    // for the same reason a new field of a descriptor component starts zeroed:
    // every type in this vocabulary is blittable, so each declared shape has a
    // defined all-zero value, and C# gives a fresh struct exactly that.
    //
    // Only when nothing is stored: a reload re-registers a resource it already
    // owns, and seeding it again would destroy the value the reload is meant to
    // preserve.
    if engine.world().foreign_resource_bytes(resource_id).is_none() {
        engine
            .world_mut()
            .insert_foreign_resource_bytes(resource_id, &vec![0_u8; resource.size])
            .map_err(|error| CSharpError::ManifestInvalid {
                message: error.to_plain_message(),
            })?;
    }
    info!(
        target: telemetry_target::HOT_RELOAD,
        resource = %resource.full_name,
        size = resource.size,
        align = resource.align,
        "managed resource registered"
    );
    Ok(resource_id)
}

/// Builds the byte plan from one resource shape to another, and applies the
/// declared defaults to the fields the diff leaves empty.
///
/// A reshape and a rename share it, because they differ only in the id the plan
/// runs against. `set_default` does nothing on a copied field, which stops a
/// changed default from overwriting a carried value.
///
/// # Panics
///
/// Panics if the engine refuses a declared default for its planned field, which
/// the manifest validation makes impossible.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
fn resource_field_plan(
    previous: &[ResourceFieldLayout],
    arriving: &[ResourceFieldLayout],
) -> FieldPlan {
    let previous_fields: Vec<LayoutField<'_>> = previous.iter().map(as_layout_field).collect();
    let next_fields: Vec<LayoutField<'_>> = arriving.iter().map(as_layout_field).collect();
    let mut plan = FieldPlan::between(&previous_fields, &next_fields);
    for field in arriving {
        let Some(bytes) = &field.default else {
            continue;
        };
        plan.set_default(field.offset, bytes)
            .expect("the field default was validated against the planned resource field");
    }
    plan
}

/// Moves one resource's stored bytes from the layout it has to the one the
/// arriving manifest declares.
///
/// Fields are matched by name. A field that moved is copied to its new offset.
/// A field the new shape adds starts at zero, or at its declared default,
/// instead of reading whatever followed in memory. A field the new shape
/// dropped is left out. A reshaped managed component's rows follow the same
/// rules, because both build the same plan.
///
/// A shipping build has no manifest apply path, so it refuses a changed
/// resource layout instead of reshaping the stored bytes.
#[cfg(not(feature = "hot_reload"))]
fn relayout_declared_resource(
    _engine: &mut Engine,
    _existing: &ResourceBinding,
    arriving: &ManagedResourceDeclaration,
) -> Result<(), CSharpError> {
    Err(CSharpError::ManifestInvalid {
        message: format!(
            "managed resource {} changed its layout; a shipping build cannot reshape storage",
            arriving.full_name
        ),
    })
}

/// # Errors
///
/// Returns a [`CSharpError`] when the engine refuses the new layout or the byte
/// migration.
#[cfg(feature = "hot_reload")]
fn relayout_declared_resource(
    engine: &mut Engine,
    existing: &ResourceBinding,
    arriving: &ManagedResourceDeclaration,
) -> Result<(), CSharpError> {
    // Step 1: Build the plan from the stored layout to the arriving one.
    let plan = resource_field_plan(&existing.declaration.fields, &arriving.fields);
    let retyped = plan.retyped_fields().to_vec();
    // Step 2: Ask the engine to move the stored bytes into the new shape.
    engine
        .world_mut()
        .relayout_foreign_resource(
            existing.resource_id,
            arriving.size,
            arriving.align,
            arriving.schema_hash,
            &plan,
        )
        .map_err(|error| CSharpError::ManifestInvalid {
            message: error.to_plain_message(),
        })?;
    // A retyped field keeps its name but no longer means the same thing, so its
    // old bytes are reset instead of being reinterpreted. That loses data, and
    // a silent reset is how a reload zeroes a value someone was watching, so it
    // is reported.
    if !retyped.is_empty() {
        warn!(
            target: telemetry_target::HOT_RELOAD,
            resource = %arriving.full_name,
            fields = %retyped.join(", "),
            "managed resource fields changed type; their values were reset"
        );
    }
    // Step 3: Publish the arriving field layout. The engine dropped the stored
    // layout together with the shape it described, so the new one is published
    // here rather than left until the next startup.
    publish_field_layout(engine, existing.resource_id, arriving);
    info!(
        target: telemetry_target::HOT_RELOAD,
        resource = %arriving.full_name,
        size = arriving.size,
        "managed resource migrated to a new layout"
    );
    Ok(())
}

/// Gives the engine the field layout this declaration carries.
///
/// Without it, a foreign resource's bytes are opaque, so this is what makes the
/// resource inspectable. It is the resource counterpart of the field layout a
/// managed component's registration publishes. When the engine refuses the
/// layout, the failure is logged instead of returned: being inspectable is not
/// worth failing a reload over, and the resource is already registered and
/// usable at that point.
///
/// It is called again after a relayout, because the engine drops the stored
/// layout when the shape moves instead of serving the old offsets over the new
/// bytes.
fn publish_field_layout(
    engine: &mut Engine,
    resource_id: ResourceId,
    resource: &ManagedResourceDeclaration,
) {
    if resource.fields.is_empty() {
        return;
    }
    let descriptors: Vec<ComponentFieldDescriptor> = resource
        .fields
        .iter()
        .map(|field| ComponentFieldDescriptor {
            name: intern(&field.name),
            type_tag: intern(&field.type_tag),
            offset: field.offset,
            size: field.size,
            // Every managed resource field is a flat blittable value, so its
            // alignment is its own width and it has no array length to carry.
            align: field.size.max(1),
            element_count: 0,
            decorators: std::borrow::Cow::Owned(field.decorations.clone()),
        })
        .collect();
    if let Err(error) = engine
        .world_mut()
        .register_resource_field_layout(resource_id, descriptors)
    {
        warn!(
            target: telemetry_target::HOT_RELOAD,
            resource = %resource.full_name,
            error = %error,
            "managed resource field layout refused; the resource works but is not inspectable"
        );
    }
}

/// Borrows one recorded field as the plan's layout view.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
fn as_layout_field(field: &ResourceFieldLayout) -> LayoutField<'_> {
    LayoutField {
        name: field.name.as_str(),
        type_tag: field.type_tag.as_str(),
        offset: field.offset,
        size: field.size,
    }
}

/// Builds the table entry for one accepted declaration.
fn binding_for_declaration(
    resource_id: ResourceId,
    resource: &ManagedResourceDeclaration,
) -> ResourceBinding {
    ResourceBinding {
        resource_id,
        declaration: resource.clone(),
    }
}

// =============================================================================
// FFI
// =============================================================================

/// Serves one resource's bytes to a managed `Res<T>` or `ResMut<T>`.
///
/// The managed side turns the status code into a message:
///
/// - `0`: the view was filled;
/// - `1`: the identity names no registered resource;
/// - `2`: the running system did not declare this access;
/// - `3`: no managed system is running on this thread;
/// - `4`: the world holds no value for the resource yet;
/// - `5`: the caller passed no output buffer.
///
/// The order matters. The scope is checked before the table, so a call from
/// outside a system reports that, rather than naming a resource it could not
/// have reached anyway.
fn ffi_get_resource_view_guarded(
    stable_id_low: u64,
    stable_id_high: u64,
    access_mode: u8,
    view_output: *mut ResourceView,
) -> u8 {
    // Step 1: Refuse a null output buffer, reporting the caller's bug.
    if view_output.is_null() {
        return 5;
    }
    let key = StableComponentId::from_halves(stable_id_low, stable_id_high);
    // Step 2: Check that the running system declared this access. A write
    // declaration also permits reads; the authorization check applies that
    // rule and refuses an unknown mode, so neither is handled here.
    match resource_access_is_authorized(key, access_mode) {
        None => return 3,
        Some(false) => return 2,
        Some(true) => {}
    }
    // Step 3: Find the binding and fill the caller's view from the live value.
    let Some((resource_id, declared_size)) = bound_resource_id_and_width(key) else {
        return 1;
    };
    let status = with_active_world(|world| {
        fill_resource_view(world, resource_id, declared_size, access_mode, view_output)
    });
    status.unwrap_or(3)
}

/// The panic-guarded entry point for [`ffi_get_resource_view_guarded`].
///
/// A panic that crosses an `extern "C"` boundary aborts the process. The
/// managed side already has a per-system error channel for a misbehaving
/// system, so a panic reached through this callback reports `5` instead of
/// killing the host mid-frame. That is the same status the callback returns
/// when the caller passes no output buffer, and the frame's error path names
/// the system that made the call.
pub(super) extern "C" fn ffi_get_resource_view(
    stable_id_low: u64,
    stable_id_high: u64,
    access_mode: u8,
    view_output: *mut ResourceView,
) -> u8 {
    super::context::guard_managed_callback("ffi_get_resource_view", 5, || {
        ffi_get_resource_view_guarded(stable_id_low, stable_id_high, access_mode, view_output)
    })
}

/// Writes one resource's live bytes into the caller's view.
///
/// A read takes the immutable byte view. A write takes the mutable one, which
/// marks the resource as changed as it is handed out. That matches the engine's
/// rule: managed code holds raw bytes with no wrapper that could notice a
/// mutation, so handing out the borrow is the last moment a change can be
/// recorded.
fn fill_resource_view(
    world: &mut World,
    resource_id: ResourceId,
    declared_size: usize,
    access_mode: u8,
    view_output: *mut ResourceView,
) -> u8 {
    // Step 1: Borrow the live bytes for the requested mode.
    let scope_token = super::context::active_scope_token();
    let view = if access_mode == 0 {
        let Some(bytes) = world.foreign_resource_bytes(resource_id) else {
            return 4;
        };
        // The binding and the engine's allocation are two records of one
        // layout, and they can drift. The check runs where both are at hand, so
        // a path that relayouts the resource without updating the table fails
        // in a debug build at its source. The view always carries the live
        // length, so managed code cannot step wrongly either way.
        debug_assert_eq!(
            bytes.len(),
            declared_size,
            "resource binding matches storage"
        );
        ResourceView {
            // The cast drops the immutability the engine granted. This is the
            // one place the file relies on the managed contract instead of the
            // type system: a `Res<T>` hands out `ref readonly`, so a write
            // through one does not compile. The component chunk path makes the
            // same trade-off for a read term.
            data: bytes.as_ptr().cast_mut(),
            length: bytes.len() as u32,
            scope_token,
        }
    } else {
        let Some((bytes, _ticks)) = world.foreign_resource_bytes_mut(resource_id) else {
            return 4;
        };
        debug_assert_eq!(
            bytes.len(),
            declared_size,
            "resource binding matches storage"
        );
        ResourceView {
            data: bytes.as_mut_ptr(),
            length: bytes.len() as u32,
            scope_token,
        }
    };
    // Step 2: Write the view into the caller's output buffer.
    // SAFETY: `view_output` was checked non-null by the FFI entry point and
    // points at a caller-owned `ResourceView` that lives for this call. The
    // data pointer written into it borrows engine storage that stays in place
    // for the rest of this managed invocation, which is exactly as long as the
    // scope token it carries remains current.
    unsafe { view_output.write(view) };
    0
}

// =============================================================================
// Access Reflection
// =============================================================================

/// Turns one reflected resource access into scheduler metadata.
///
/// Returns the engine id the access refers to, so the caller can add it to a
/// [`SystemAccess`](pill_engine::SystemAccess) as a read or a write.
///
/// # Errors
///
/// Returns [`CSharpError::UnregisteredResource`] when the key names no
/// registered resource. That means a system declared `Res<T>` for a type the
/// manifest never carried, which is almost always a missing `[EcsResource]`.
pub(super) fn resolve_resource_access(
    stable_id_low: u64,
    stable_id_high: u64,
) -> Result<ResourceId, CSharpError> {
    let key = StableComponentId::from_halves(stable_id_low, stable_id_high);
    bound_resource_id_and_width(key)
        .map(|(resource_id, _size)| resource_id)
        .ok_or_else(|| CSharpError::UnregisteredResource {
            key: format!("{stable_id_high:016X}{stable_id_low:016X}"),
        })
}

/// The discriminator a resource access carries in the reflected list.
///
/// It is re-exported here so the backend reads it from the module that owns the
/// meaning, not from the scope context where the constant is defined.
pub(super) const RESOURCE_ACCESS_KIND: u8 = ACCESS_KIND_RESOURCE;
