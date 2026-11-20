//! C# component identities, native bindings, and manifest registration.
//!
//! # Responsibilities
//!
//! - Define the stable identity a managed component is addressed by, and the
//!   binding that ties it to native or managed storage.
//! - Keep the live binding table ([`BindingStore`]) that a running managed
//!   project reads its columns through.
//! - Expose extension components to managed code by name
//!   ([`ModuleExposedComponent`]).
//! - Register a reflected component manifest; the schema model and its
//!   validation live in [`manifest`](super::manifest).
//! - Supply the component half of the shared reload pipeline, as
//!   [`COMPONENT_SUBJECT`]; the stages it runs through live in
//!   [`manifest_apply`](super::manifest_apply).
//!
//! # Design
//!
//! The shared components (`TransformComponent`, `MeshRendererComponent` and
//! the rest) resolve to the Rust types the host links in every posture, so a
//! headless host binds them exactly as a windowed one does. Every managed
//! component is addressed by a [`StableComponentId`], a hash of its canonical
//! full name. [`ComponentBinding`] records whether the storage behind that
//! identity is a managed layout of raw bytes or a native column that an
//! extension registered.
//!
//! A reload has no apply loop of its own. It hands the binding table and the
//! arriving declarations to the pipeline that every manifest kind runs
//! through, together with [`COMPONENT_SUBJECT`], which holds the parts a
//! component does differently from a resource.

// Standard library
use std::collections::HashMap;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

// External crates
use pill_core::error::{CSharpError, EngineMessage};
use pill_core::info;
use pill_core::telemetry::telemetry_target;
use pill_engine::archetype::{Blittability, FieldPlan, LayoutField};
use pill_engine::component_registry::ComponentFieldDescriptor;
use pill_engine::{ComponentId, Engine, World};

// Current crate
#[cfg(feature = "hot_reload")]
use super::manifest::build_field_plan;
use super::manifest::{
    format_field_layout_line, handle_asset_path, managed_field_layout, parse_and_validate_manifest,
    plan_tag, split_manifest_kinds, ManagedManifestEntry,
};
use super::manifest_apply::{BindingTable, ManifestSubject, RenamePredecessor, Settlement};

// =============================================================================
// Types + Impls
// =============================================================================

/// Stable 128-bit identity of a managed component, derived from its name.
///
/// The name is the C# full name with dots (`pill_spline.Spline`).
/// [`stable_component_id`] hashes it with the engine's own mixing function,
/// and the managed runtime computes the same value in `Engine.StableIdOf`, so
/// two separately compiled sides agree without exchanging anything. The C ABI
/// has no 128-bit integer, so `component_key` and `component_key_high` carry
/// the two `u64` halves, and [`Self::from_halves`] puts them back together.
///
/// A shared component that Rust registered is named with `::` separators, so
/// it hashes to a different value. [`ModuleExposedComponent`] keeps the C#
/// name beside the engine component ID the Rust name produced, and neither
/// name is translated into the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct StableComponentId(
    /// The 128-bit canonical identity value.
    pub(super) u128,
);

impl StableComponentId {
    /// Reconstruct the canonical 128-bit ID from the two halves carried by the
    /// C ABI, with the high half restored to its original bit position.
    pub(super) const fn from_halves(low: u64, high: u64) -> Self {
        Self(((high as u128) << 64) | low as u128)
    }
}

/// Maps every stable managed identity to its native or managed binding.
pub(super) type ComponentBindings = HashMap<StableComponentId, ComponentBinding>;

/// The live binding table, replaceable while a managed project runs.
///
/// Every managed system closure holds this handle instead of a copy of the
/// map. A reload can change a component's layout or add a component, and the
/// invocation scope a system installs when it runs has to see the layout its
/// columns actually use, not the one that was current when the system was
/// registered. The lock is held for one system run and taken for writing only
/// between frames, so the two never overlap.
pub(super) struct BindingStore {
    /// The table itself.
    bindings: RwLock<ComponentBindings>,
}

impl BindingStore {
    /// Wrap one binding table.
    pub(super) fn new(bindings: ComponentBindings) -> Self {
        Self {
            bindings: RwLock::new(bindings),
        }
    }

    /// Borrow the table for reading.
    ///
    /// If a thread panicked while it held the lock, the table is used anyway.
    /// A panic cannot leave a map of plain data in a wrong state, because a
    /// writer either finishes the one entry it was changing or leaves it out,
    /// so turning the poisoned lock into an error would add a failure mode no
    /// caller can act on.
    pub(super) fn read(&self) -> RwLockReadGuard<'_, ComponentBindings> {
        self.bindings
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Borrow the table for writing, with the same rule as [`Self::read`].
    #[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
    pub(super) fn write(&self) -> RwLockWriteGuard<'_, ComponentBindings> {
        self.bindings
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Tells the bridge where one managed identity's storage lives.
///
/// Every binding is type-erased: managed storage keeps the layout facts that
/// raw bytes need, and a Rust component - an extension's or the renderer
/// data's - is served through the byte view of its native column. The bridge
/// names no Rust component type.
#[derive(Clone, Copy)]
pub(super) enum ComponentBinding {
    /// A managed layout stored as raw bytes.
    Managed {
        /// Engine ID of the managed component type.
        component_id: ComponentId,
        /// Size of the managed layout in bytes.
        size: usize,
        /// Alignment of the managed layout in bytes.
        align: usize,
        /// Hash of the managed field schema the storage was registered with.
        ///
        /// Stored here instead of being read back from the engine, because a
        /// reload has to tell a component whose layout changed from one whose
        /// bytes only moved. The two can agree on size and alignment while
        /// their fields differ.
        schema_hash: u64,
    },
    /// A native component registered by an extension, exposed to managed code
    /// through the raw byte view of its column.
    ///
    /// The host never names the concrete Rust type. Reads and writes go
    /// through the type-erased native column accessors, exactly as they do for
    /// managed storage; the difference is that the column is the module's own
    /// native storage, so managed code and Rust code read and write the same
    /// rows.
    ModuleNative {
        /// Engine ID of the module-registered native component type.
        component_id: ComponentId,
        /// Size of the native layout in bytes.
        size: usize,
        /// Alignment of the native layout in bytes.
        align: usize,
        /// Hash of the public fields the generated mirror declares, when the
        /// component's field layout is known. A managed mirror has to declare
        /// exactly those fields. `None` means only size and alignment are
        /// checked.
        field_signature: Option<u64>,
    },
}

impl ComponentBinding {
    /// Return the engine component ID regardless of whether storage is backed
    /// by a concrete Rust type or a managed layout.
    pub(super) fn component_id(self) -> ComponentId {
        match self {
            Self::Managed { component_id, .. } | Self::ModuleNative { component_id, .. } => {
                component_id
            }
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Produce one half of the stable component identity or a native schema hash.
///
/// Re-exported from the engine instead of being written again here: the engine
/// derives a shared component's [`ComponentId`] from its declared name with
/// this same function, and a second copy could change differently later and
/// silently stop agreeing on an identity.
pub(super) use pill_engine::component::component_name_hash as component_hash;

/// Hash a canonical managed full name into its stable 128-bit identity.
///
/// Managed code names a component with dots (`pill_spline.Spline`), while Rust
/// names it with colons, so this identity is not interchangeable with the one
/// the engine derives for a shared component. [`ModuleExposedComponent`] keeps
/// the C# name beside the engine component ID the Rust name produced, so both
/// identities are kept as they are. Only the mixing function is shared.
pub(super) const fn stable_component_id(name: &str) -> StableComponentId {
    StableComponentId::from_halves(
        component_hash(name, 0xcbf29ce484222325),
        component_hash(name, 0x84222325cbf29ce4),
    )
}

/// One native component an extension registered, exposed to managed code under
/// a derived C#-facing name.
///
/// The host collects these after the extensions load and hands them to the C#
/// backend, which creates a byte-level [`ComponentBinding::ModuleNative`] for
/// each one so `project_cs` can query and write the module's real storage.
#[derive(Debug, Clone)]
pub struct ModuleExposedComponent {
    /// C#-facing name derived from the registered Rust type name
    /// (`pill_spline::Spline` -> `pill_spline.Spline`). Managed code declares
    /// its mirror struct under exactly this full name so the stable 128-bit
    /// identity matches.
    pub csharp_name: String,
    /// Engine ID of the module-registered native component.
    pub component_id: ComponentId,
    /// Size in bytes of the native layout.
    pub size: usize,
    /// Alignment in bytes of the native layout.
    pub align: usize,
    /// Compile-time field layout registered with the component, when the
    /// component was declared with `#[derive(PillComponent)]`. Empty for a
    /// component registered without the derive, and for managed components;
    /// both keep the mirror that treats the component as one opaque block of
    /// bytes.
    ///
    /// These are the fields *as declared*: a mirror is a struct declaration,
    /// so a nested `struct:` field stays one field and the code generator
    /// resolves its C# type. The editor's flattened leaf rows (`points[0].y`)
    /// describe an inspector, not a type, and would name fields that C# cannot
    /// declare.
    #[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
    pub fields: Vec<ComponentFieldDescriptor>,
}

/// Turn registered component names into the layouts that C# binds against.
///
/// The C#-facing name is the Rust path with `::` replaced by `.`, so a mirror
/// struct generated under that full name reproduces the same stable identity.
/// A name that does not resolve is skipped: the managed side binds by name, so
/// a name with no component simply has no binding. The field layout comes from
/// the derive's registration; a component without one keeps the opaque byte
/// blob.
pub fn exposed_components_from_names(
    world: &World,
    names: &[String],
) -> Vec<ModuleExposedComponent> {
    // A module can register one component more than once: registration is
    // idempotent, and the renderer data crate registers its components both
    // explicitly and through its module entry point, so a name can repeat in
    // the registration log. Expose each name once, in first-registration
    // order. A repeated name would emit its C# mirror struct twice.
    let mut exposed_type_names = std::collections::HashSet::new();
    names
        .iter()
        .filter(|type_name| exposed_type_names.insert(type_name.as_str()))
        .filter_map(|type_name| {
            let component_id = resolve_exposed_component_id(world, type_name)?;
            let (size, align) = world.component_layout(component_id)?;
            let fields = world
                .component_declared_field_layout(component_id)
                .unwrap_or(&[]);
            Some(ModuleExposedComponent {
                csharp_name: type_name.replace("::", "."),
                component_id,
                size,
                align,
                fields: fields.to_vec(),
            })
        })
        .collect()
}

/// Resolve one exposed component name to its engine [`ComponentId`], reporting
/// an ambiguous name instead of binding managed code to a guessed column.
///
/// A name that resolves to nothing is an ordinary outcome, because the module
/// that registered it may be unloaded, so it yields `None` quietly. A name
/// claimed by two registrations is not ordinary: managed code would be bound to
/// one of the two columns, chosen arbitrarily, and every read and write through
/// that binding would silently address the wrong rows. That case is logged and
/// left unbound.
pub(super) fn resolve_exposed_component_id(world: &World, type_name: &str) -> Option<ComponentId> {
    match world.resolve_component_id_by_name_any(type_name) {
        Ok(component_id) => component_id,
        Err(error) => {
            pill_core::error!(
                target: telemetry_target::ECS,
                type_name = %type_name,
                error = %error,
                "exposed component name is ambiguous; leaving it unbound for managed code"
            );
            None
        }
    }
}

/// Build byte-level bindings for every extension component exposed to
/// managed code, keyed by the stable identity of its derived C# name.
///
/// The bindings are merged into the shared table before the managed manifest
/// is registered, so a `project_cs` mirror whose full name matches a module
/// component resolves to the module's native storage instead of being
/// registered as an unrelated managed component.
pub(super) fn module_native_bindings(
    engine: &mut Engine,
    exposed_components: &[ModuleExposedComponent],
) -> ComponentBindings {
    let mut component_bindings = HashMap::new();
    for component in exposed_components {
        component_bindings.insert(
            stable_component_id(&component.csharp_name),
            ComponentBinding::ModuleNative {
                component_id: component.component_id,
                size: component.size,
                align: component.align,
                field_signature: super::mirror_naming::generated_field_signature(&component.fields)
                    .map(|signature| component_hash(&signature, 0xcbf29ce484222325)),
            },
        );
    }
    // Check every binding against the live column before it is handed out. An
    // id that is gone yields no binding. A layout that disagrees with what the
    // module forwarded is logged and dropped here, because otherwise managed
    // code would later be served a chunk of the wrong size.
    component_bindings.retain(|_, binding| match binding {
        ComponentBinding::ModuleNative {
            component_id,
            size,
            align,
            ..
        } => match engine.world().component_layout(*component_id) {
            Some((live_size, live_align)) if live_size == *size && live_align == *align => true,
            Some((live_size, live_align)) => {
                pill_core::error!(
                    target: telemetry_target::ECS,
                    component_id = ?component_id,
                    forwarded_size = *size,
                    forwarded_align = *align,
                    live_size,
                    live_align,
                    "module component binding disagrees with the live column; dropping it"
                );
                false
            }
            None => false,
        },
        _ => true,
    });
    component_bindings
}

/// Validate and register all components discovered in the managed assembly.
///
/// Shared components must already have a native engine binding; every other
/// component is registered as managed storage before the bindings are
/// returned to the caller.
///
/// # Errors
///
/// Returns a [`CSharpError`] when the manifest is malformed, an identity does
/// not match its canonical name, a layout is invalid or duplicated, a shared
/// component has no native binding, or a managed mirror disagrees with the
/// native component's layout or field schema.
pub(super) fn register_component_manifest(
    engine: &mut Engine,
    bytes: &[u8],
    mut bindings: ComponentBindings,
) -> Result<ComponentBindings, CSharpError> {
    let (components, resources) = split_manifest_kinds(parse_and_validate_manifest(bytes)?);
    // Step 1: Register the resources. A startup method may already want to
    // read one, and a resource registration touches no column, so nothing here
    // can be undone by a component entry refused afterwards.
    super::resources::register_resource_manifest(engine, &resources)?;
    for component in components {
        let stable_id =
            StableComponentId::from_halves(component.stable_id_low, component.stable_id_high);

        if let Some(binding) = bindings.get(&stable_id).copied() {
            check_binding_against_manifest(binding, &component)?;
            continue;
        }

        // Step 2: Register each remaining component as managed storage.
        if component.shared {
            return Err(format!(
                "managed shared component {} has no native engine binding",
                component.full_name
            )
            .into());
        }
        // Registration goes through the one helper that every reload path
        // uses, so startup and an assembly swap install managed storage the
        // same way. The helper builds the editor layout after the checks above,
        // because that layout publishes every field name and struct tag: an
        // entry that already had a binding, or a manifest just refused as
        // shared, must not publish anything.
        let (component_id, field_layout) = register_managed_storage(engine, stable_id, &component)?;
        bindings.insert(stable_id, managed_binding(component_id, &component));
        // The binding and the registered column are two records of one layout,
        // and the two can disagree. The check runs where both are written, so a
        // path that later changes the column without updating the store fails
        // in a debug build at its source. The chunk path reads the live column,
        // so its byte stride is right whatever the store says.
        debug_assert_eq!(
            engine.world().component_layout(component_id),
            Some((component.size, component.alignment)),
            "the column just registered must match the binding just stored"
        );

        // Give the editor the same field vocabulary that
        // `#[derive(PillComponent)]` produces, so a C# component shows named,
        // editable fields instead of none. Registering the component again on
        // an assembly swap replaces the layout.
        info!(
            target: telemetry_target::HOT_RELOAD,
            component = %component.full_name,
            fields = %format_field_layout_line(&field_layout),
            "managed component field layout registered"
        );
    }
    Ok(bindings)
}

/// Hash of the public fields a managed mirror declares, in the form
/// [`generated_field_signature`](super::mirror_naming::generated_field_signature)
/// produces for the native side.
///
/// The generated alignment pad is private, and the generator does not list it
/// as a data field, so it is left out.
fn managed_field_signature(component: &ManagedManifestEntry) -> u64 {
    let declared_fields = component
        .fields
        .iter()
        .filter(|field| field.name != "_alignmentPad")
        .map(|field| (field.offset, field.name.clone(), field.size))
        .collect();
    component_hash(
        &super::mirror_naming::field_signature_text(declared_fields),
        0xcbf29ce484222325,
    )
}

/// Check one manifest entry against the binding the host already holds.
///
/// Both paths use it, so they cannot disagree about what "the same component"
/// means. If a reload accepted a difference that startup refused, the
/// registered id would mean something else after the reload.
///
/// # Errors
///
/// Returns a [`CSharpError`] when the entry's size or alignment differs from
/// the binding's. It also returns one when the schemas differ: a managed
/// binding must agree on its schema hash, and a module-native binding, when it
/// carries a field signature, must agree on its list of public fields.
pub(super) fn check_binding_against_manifest(
    binding: ComponentBinding,
    component: &ManagedManifestEntry,
) -> Result<(), CSharpError> {
    let (size, align, expected_schema) = match binding {
        ComponentBinding::Managed {
            size,
            align,
            schema_hash,
            ..
        } => (size, align, Some(schema_hash)),
        ComponentBinding::ModuleNative {
            size,
            align,
            field_signature,
            ..
        } => {
            // The manifest's own schema hash is an FNV-1a hash of a C#-only
            // text (managed type names, nested-struct recursion), which the
            // engine's flat descriptors cannot reproduce. Both sides can state
            // the list of public fields instead (name, offset, size), because
            // the mirror generator derives it from the native descriptors.
            // Comparing that list refuses a mirror whose size is right but
            // whose fields differ.
            if let Some(expected) = field_signature {
                if managed_field_signature(component) != expected {
                    return Err(format!(
                        "managed mirror {} does not match the native component field schema",
                        component.full_name
                    )
                    .into());
                }
            }
            (size, align, None)
        }
    };
    if size != component.size || align != component.alignment {
        return Err(format!(
            "managed mirror {} has layout size/alignment {}/{} but native component uses {}/{}",
            component.full_name, component.size, component.alignment, size, align
        )
        .into());
    }
    if expected_schema.is_some_and(|hash| hash != component.schema_hash) {
        return Err(format!(
            "managed mirror {} does not match the native component field schema",
            component.full_name
        )
        .into());
    }
    Ok(())
}

/// What one applied manifest changed, for the caller's log line.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
#[derive(Debug, Default)]
pub(super) struct ManifestApplyReport {
    /// Managed components whose layout changed and whose rows were migrated.
    pub(super) migrated: Vec<String>,
    /// Managed components the manifest added.
    pub(super) added: Vec<String>,
    /// Managed components the manifest renamed through an alias, as
    /// `old name -> new name`.
    pub(super) renamed: Vec<String>,
    /// Managed components whose storage was retired because the manifest
    /// stopped naming them.
    pub(super) retired: Vec<String>,
    /// Managed resources the manifest added.
    pub(super) resources_added: Vec<String>,
    /// Managed resources whose layout changed and whose bytes were migrated.
    pub(super) resources_migrated: Vec<String>,
    /// Managed resources the manifest renamed through an alias, as
    /// `old name -> new name`.
    pub(super) resources_renamed: Vec<String>,
    /// Managed resources whose declaration was retired because the manifest
    /// stopped naming it.
    pub(super) resources_retired: Vec<String>,
}

/// Apply a swapped assembly's component manifest to the live world.
///
/// The reload counterpart of [`register_component_manifest`]: where startup may
/// register what it likes, this one has to *migrate*, so a managed component
/// whose layout changed keeps its entities and its rows and only the bytes
/// move. This kind's contribution to that pipeline is [`COMPONENT_SUBJECT`].
///
/// These cases are refused rather than migrated:
///
/// - a `Native` or `ModuleNative` binding whose layout or schema changed - the
///   Rust side did not change, so the mirror is simply wrong;
/// - a shared component with no native binding, exactly as at startup;
/// - a managed entry whose aliases name more than one previous registration,
///   because picking one would silently drop the other's rows;
/// - an entry that is already registered and also declares an alias, because
///   treating it as unchanged would leave the predecessor's rows in a binding
///   that nothing tracks.
///
/// A managed component the manifest stopped naming is not refused: its storage
/// is retired after everything else has applied, so a removed type does not
/// force a restart while developing.
///
/// # Errors
///
/// Returns a [`CSharpError`] for any of the refusals above, for a malformed
/// manifest, or when the engine refuses a registration or a relayout.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) fn apply_component_manifest_on_reload(
    engine: &mut Engine,
    bytes: &[u8],
    store: &BindingStore,
) -> Result<ManifestApplyReport, CSharpError> {
    let (manifest, resources) = split_manifest_kinds(parse_and_validate_manifest(bytes)?);
    // Step 1: Resolve both kinds before either one changes the engine. One
    // manifest carries resources and components together, and every refusal
    // about the manifest's own shape - an ambiguous alias, an entry that cannot
    // be registered as it stands, a table that already disagrees - is raised
    // here, while the world is still untouched. Applying one kind first and
    // letting the other refuse later would leave a rejected reload's resources
    // migrated under the generation that keeps running.
    let prepared_resources = super::resources::prepare_resource_manifest(engine, &resources)?;
    let previous_components = store.read().clone();
    let prepared_components = super::manifest_apply::prepare_manifest(
        &COMPONENT_SUBJECT,
        engine,
        previous_components,
        &manifest,
    )?;

    // Step 2: Apply the component entries. The pipeline records each entry it
    // applies so it can undo it, so a refusal here still leaves nothing applied
    // and the resources below untouched.
    let (component_apply_result, binding_table) =
        super::manifest_apply::execute_manifest(&COMPONENT_SUBJECT, engine, prepared_components);
    *store.write() = binding_table;
    let component_outcome = component_apply_result?;

    // Step 3: Apply the resources last. A relayout is the one step that cannot
    // be reversed, so nothing that can fail may run after it.
    let resource_outcome = super::resources::execute_resource_manifest(engine, prepared_resources)?;

    Ok(ManifestApplyReport {
        migrated: component_outcome.migrated,
        added: component_outcome.added,
        renamed: component_outcome.renamed,
        retired: component_outcome.retired,
        resources_added: resource_outcome.added,
        resources_migrated: resource_outcome.migrated,
        resources_renamed: resource_outcome.renamed,
        resources_retired: resource_outcome.retired,
    })
}

// =============================================================================
// The component manifest subject
// =============================================================================

/// One applied component entry, recorded so a later refusal can be undone.
///
/// Engine state only: the binding table's half of an undo belongs to the
/// pipeline, which knows exactly what it wrote where.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) enum ComponentUndo {
    /// The entry registered a managed component.
    Added { component_id: ComponentId },
    /// The entry migrated a managed component to a new shape.
    Migrated {
        component_id: ComponentId,
        previous_binding: ComponentBinding,
        previous_fields: Vec<ComponentFieldDescriptor>,
    },
    /// The entry renamed a managed component from an earlier registration.
    Renamed {
        successor_id: ComponentId,
        predecessor_stable_id: StableComponentId,
        predecessor_name: String,
        predecessor_binding: ComponentBinding,
        predecessor_fields: Vec<ComponentFieldDescriptor>,
    },
}

/// The actions the shared apply pipeline needs for managed components.
///
/// Everything a component does differently from a resource, and nothing else:
/// the stages themselves live in [`super::manifest_apply`].
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) const COMPONENT_SUBJECT: ManifestSubject<
    ManagedManifestEntry,
    ComponentBinding,
    ComponentUndo,
> = ManifestSubject {
    kind: "component",
    identity: |component| {
        StableComponentId::from_halves(component.stable_id_low, component.stable_id_high)
    },
    name: |component| component.full_name.as_str(),
    aliases: |component| &component.aliases,
    binding_name: component_binding_name,
    // Only managed storage belongs to this pipeline, so only managed storage
    // can be retired here. A native or module-native binding describes a column
    // that Rust owns, and a manifest that stops naming it says nothing about
    // that column's lifetime.
    is_governed: |binding| matches!(binding, ComponentBinding::Managed { .. }),
    resolve_alias: resolve_component_alias,
    check_addable: |component| {
        if component.shared {
            return Err(format!(
                "managed shared component {} has no native engine binding",
                component.full_name
            )
            .into());
        }
        Ok(())
    },
    settle: settle_component,
    register: register_component_entry,
    reshape: reshape_component_entry,
    rename: rename_component_entry,
    retire: |engine, bindings| {
        let retired_component_ids: Vec<ComponentId> = bindings
            .iter()
            .map(|binding| binding.component_id())
            .collect();
        engine
            .world_mut()
            .retire_component_storage(&retired_component_ids)
    },
    undo: undo_component_entry,
};

/// The name a live binding's component is registered under.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
fn component_binding_name(engine: &Engine, binding: &ComponentBinding) -> String {
    let component_id = binding.component_id();
    engine
        .world()
        .registered_components()
        .iter()
        .find(|(_, id)| *id == component_id)
        .map(|(name, _)| name.clone())
        .unwrap_or_default()
}

/// Resolve one alias to the managed binding it names, if any.
///
/// The alias is a name the engine knows, so it is resolved through the registry
/// first, and the table is then searched for the binding that holds that id. A
/// resolution that reaches a native binding is ignored: it names a column Rust
/// owns, and a managed entry cannot inherit its rows.
///
/// # Errors
///
/// Returns a [`CSharpError`] when the engine knows the name but cannot map it to
/// one component, which is the ambiguity [`resolve_exposed_component_id`]
/// reports.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
fn resolve_component_alias(
    engine: &Engine,
    table: &BindingTable<ComponentBinding>,
    alias: &str,
) -> Result<Option<StableComponentId>, CSharpError> {
    let resolved_component_id = engine
        .world()
        .resolve_component_id_by_name_any(alias)
        .map_err(|error| CSharpError::ManifestInvalid {
            message: error.to_plain_message(),
        })?;
    let Some(component_id) = resolved_component_id else {
        return Ok(None);
    };
    Ok(table.iter().find_map(|(stable_id, binding)| {
        (binding.component_id() == component_id
            && matches!(binding, ComponentBinding::Managed { .. }))
        .then_some(*stable_id)
    }))
}

/// Decide what an arriving entry means for the binding already registered.
///
/// A native or module-native binding is never migrated: the Rust side did not
/// change, so a mirror that disagrees is simply wrong and is refused by the
/// same check startup applies.
///
/// # Errors
///
/// Returns the error of [`check_binding_against_manifest`] when the arriving
/// entry claims a native or module-native binding and does not match it.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
fn settle_component(
    _engine: &Engine,
    binding: &ComponentBinding,
    component: &ManagedManifestEntry,
) -> Result<Settlement, CSharpError> {
    let ComponentBinding::Managed {
        size,
        align,
        schema_hash,
        ..
    } = binding
    else {
        check_binding_against_manifest(*binding, component)?;
        return Ok(Settlement::Unchanged);
    };
    // The same layout means nothing to do. Anything else is a migration.
    if *size == component.size
        && *align == component.alignment
        && *schema_hash == component.schema_hash
    {
        return Ok(Settlement::Unchanged);
    }
    Ok(Settlement::Reshaped)
}

/// Register one manifest entry the binding table has no entry for.
///
/// This is the startup path in one function: anything that reaches here is
/// managed storage with its editor-facing field layout, because a shared entry
/// with no native binding was already refused.
///
/// # Errors
///
/// Returns a [`CSharpError`] when the engine refuses to register the column,
/// which [`register_managed_storage`] reports.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
fn register_component_entry(
    engine: &mut Engine,
    stable_id: StableComponentId,
    component: &ManagedManifestEntry,
) -> Result<(ComponentBinding, Option<ComponentUndo>), CSharpError> {
    let (component_id, _) = register_managed_storage(engine, stable_id, component)?;
    info!(
        target: telemetry_target::HOT_RELOAD,
        component = %component.full_name,
        "managed component registered on reload"
    );
    Ok((
        managed_binding(component_id, component),
        Some(ComponentUndo::Added { component_id }),
    ))
}

/// A shipping build has no manifest apply path: the real arm is compiled only
/// with the `hot_reload` feature. The stub keeps the call graph whole while
/// refusing a changed layout explicitly if it is ever reached.
#[cfg(not(feature = "hot_reload"))]
fn reshape_component_entry(
    _engine: &mut Engine,
    _stable_id: StableComponentId,
    _existing: &ComponentBinding,
    component: &ManagedManifestEntry,
) -> Result<(ComponentBinding, Option<ComponentUndo>), CSharpError> {
    Err(CSharpError::ManifestInvalid {
        message: format!(
            "managed component {} changed its layout; a shipping build cannot reshape storage",
            component.full_name
        ),
    })
}

/// Migrate one managed component's rows into the arriving layout.
///
/// # Errors
///
/// Returns a [`CSharpError`] when the engine refuses the row migration or the
/// new field layout.
#[cfg(feature = "hot_reload")]
fn reshape_component_entry(
    engine: &mut Engine,
    _stable_id: StableComponentId,
    existing: &ComponentBinding,
    component: &ManagedManifestEntry,
) -> Result<(ComponentBinding, Option<ComponentUndo>), CSharpError> {
    let component_id = existing.component_id();
    let field_plan = build_field_plan(engine, component_id, &component.fields);
    // The fields are copied before the migration, so the inverse plan needs no
    // engine borrow later, when the world is mutated again.
    let previous_fields = engine
        .world()
        .component_field_layout(component_id)
        .unwrap_or(&[])
        .to_vec();
    let migrated_row_count = engine
        .world_mut()
        .relayout_descriptor_component(
            component_id,
            component.size,
            component.alignment,
            component.schema_hash,
            &field_plan,
        )
        .map_err(|error| CSharpError::ManifestInvalid {
            message: error.to_plain_message(),
        })?;
    engine
        .world_mut()
        .register_component_descriptor_with_layout(
            component_id,
            managed_field_layout(&component.full_name, &component.fields),
        )
        .map_err(|error| CSharpError::ManifestInvalid {
            message: error.to_string(),
        })?;
    record_managed_asset_fields(engine, component_id, component);
    info!(
        target: telemetry_target::HOT_RELOAD,
        component = %component.full_name,
        rows = migrated_row_count,
        "managed component layout migrated"
    );
    // A field that changed type keeps its name but loses its value, and saying
    // nothing would make a reset look like a migration that worked. The line is
    // reported once per component, not once per field, so it stays readable
    // when one rewrite touches several fields.
    report_retyped_fields(&component.full_name, &field_plan);
    Ok((
        managed_binding(component_id, component),
        Some(ComponentUndo::Migrated {
            component_id,
            previous_binding: *existing,
            previous_fields,
        }),
    ))
}

/// A shipping build never applies a manifest, so it refuses a rename rather
/// than moving rows onto a successor.
#[cfg(not(feature = "hot_reload"))]
fn rename_component_entry(
    _engine: &mut Engine,
    _stable_id: StableComponentId,
    _predecessor: &RenamePredecessor<'_, ComponentBinding>,
    component: &ManagedManifestEntry,
) -> Result<(ComponentBinding, Option<ComponentUndo>), CSharpError> {
    Err(CSharpError::ManifestInvalid {
        message: format!(
            "managed component {} was renamed; a shipping build cannot move storage",
            component.full_name
        ),
    })
}

/// Register the successor of a renamed component and move the rows across.
///
/// # Errors
///
/// Returns a [`CSharpError`] when the successor cannot be registered, or when
/// the engine refuses to move the predecessor's rows onto it.
#[cfg(feature = "hot_reload")]
fn rename_component_entry(
    engine: &mut Engine,
    stable_id: StableComponentId,
    predecessor: &RenamePredecessor<'_, ComponentBinding>,
    component: &ManagedManifestEntry,
) -> Result<(ComponentBinding, Option<ComponentUndo>), CSharpError> {
    let predecessor_id = predecessor.binding.component_id();
    let predecessor_name = predecessor.alias.to_string();
    let predecessor_fields = engine
        .world()
        .component_field_layout(predecessor_id)
        .unwrap_or(&[])
        .to_vec();
    // The plan is built from the predecessor's live field layout, before the
    // successor is registered. Registering the successor touches no row, and
    // the remap follows it, so a failure in either step leaves the undo record
    // describing what was there.
    let field_plan = build_field_plan(engine, predecessor_id, &component.fields);
    let (successor_id, _) = register_managed_storage(engine, stable_id, component)?;
    let migrated_row_count = engine
        .world_mut()
        .remap_descriptor_component(predecessor_id, successor_id, &field_plan)
        .map_err(|error| CSharpError::ManifestInvalid {
            message: error.to_plain_message(),
        })?;
    info!(
        target: telemetry_target::HOT_RELOAD,
        component = %component.full_name,
        predecessor = %predecessor_name,
        rows = migrated_row_count,
        "managed component renamed and rows migrated"
    );
    report_retyped_fields(&component.full_name, &field_plan);
    Ok((
        managed_binding(successor_id, component),
        Some(ComponentUndo::Renamed {
            successor_id,
            predecessor_stable_id: predecessor.identity,
            predecessor_name,
            predecessor_binding: *predecessor.binding,
            predecessor_fields,
        }),
    ))
}

/// A shipping build cannot reach an undo: no manifest was applied to reverse.
#[cfg(not(feature = "hot_reload"))]
fn undo_component_entry(_engine: &mut Engine, _undo: ComponentUndo) {}

/// Reverse one applied component entry, best effort.
///
/// A rollback that fails leaves the process half-applied, so the failure is
/// logged with the component it concerns instead of replacing the original
/// refusal, which is the error the caller needs.
#[cfg(feature = "hot_reload")]
fn undo_component_entry(engine: &mut Engine, undo: ComponentUndo) {
    match undo {
        ComponentUndo::Added { component_id } => {
            // `retire_component_storage` removes the rows and every
            // registration artifact of the id, which is exactly what an
            // added-then-refused component has to give back.
            let retired_component_count =
                engine.world_mut().retire_component_storage(&[component_id]);
            info!(
                target: telemetry_target::HOT_RELOAD,
                dropped = retired_component_count,
                "rolled back a managed component registered by a refused manifest"
            );
        }
        ComponentUndo::Migrated {
            component_id,
            previous_binding,
            previous_fields,
        } => {
            let ComponentBinding::Managed {
                size,
                align,
                schema_hash,
                ..
            } = previous_binding
            else {
                // Only a managed binding is ever recorded as migrated.
                return;
            };
            let field_plan = rollback_field_plan(engine, component_id, &previous_fields);
            match engine.world_mut().relayout_descriptor_component(
                component_id,
                size,
                align,
                schema_hash,
                &field_plan,
            ) {
                Ok(restored_row_count) => {
                    if let Err(error) = engine
                        .world_mut()
                        .register_component_descriptor_with_layout(component_id, previous_fields)
                    {
                        // The rows are back, but the editor's field layout is
                        // not. Log the component and keep unwinding.
                        pill_core::error!(
                            target: telemetry_target::HOT_RELOAD,
                            component_id = ?component_id,
                            error = %error,
                            "could not restore the component's field layout during rollback"
                        );
                    }
                    info!(
                        target: telemetry_target::HOT_RELOAD,
                        rows = restored_row_count,
                        "rolled back a managed component layout migration"
                    );
                }
                Err(error) => {
                    // The rows could not be put back. Name the component so the
                    // half-applied state can be diagnosed.
                    pill_core::error!(
                        target: telemetry_target::HOT_RELOAD,
                        component_id = ?component_id,
                        error = %error.to_plain_message(),
                        "could not undo a manifest layout migration"
                    );
                }
            }
        }
        ComponentUndo::Renamed {
            successor_id,
            predecessor_stable_id,
            predecessor_name,
            predecessor_binding,
            predecessor_fields,
        } => {
            let ComponentBinding::Managed {
                size,
                align,
                schema_hash,
                ..
            } = predecessor_binding
            else {
                // Only a managed binding is ever recorded as renamed.
                return;
            };
            // The predecessor's id is derived from its stable id, so registering
            // the old name again restores the same id that the remap retired.
            // The rows then move back through the inverse plan, and that remap
            // retires the successor's registration in turn.
            let predecessor_registration = engine.world_mut().register_component_descriptor(
                predecessor_stable_id.0,
                predecessor_name.clone(),
                size,
                align,
                schema_hash,
                Blittability::from_manifest_fields(),
            );
            let restored_id = match predecessor_registration {
                Ok(id) => id,
                Err(error) => {
                    pill_core::error!(
                        target: telemetry_target::HOT_RELOAD,
                        component = %predecessor_name,
                        error = %error.to_plain_message(),
                        "could not re-register a renamed component's predecessor during rollback"
                    );
                    return;
                }
            };
            let field_plan = rollback_field_plan(engine, successor_id, &predecessor_fields);
            match engine.world_mut().remap_descriptor_component(
                successor_id,
                restored_id,
                &field_plan,
            ) {
                Ok(restored_row_count) => {
                    if let Err(error) = engine
                        .world_mut()
                        .register_component_descriptor_with_layout(restored_id, predecessor_fields)
                    {
                        pill_core::error!(
                            target: telemetry_target::HOT_RELOAD,
                            component = %predecessor_name,
                            error = %error,
                            "could not restore the renamed component's field layout during rollback"
                        );
                    }
                    info!(
                        target: telemetry_target::HOT_RELOAD,
                        component = %predecessor_name,
                        rows = restored_row_count,
                        "rolled back a managed component rename"
                    );
                }
                Err(error) => {
                    pill_core::error!(
                        target: telemetry_target::HOT_RELOAD,
                        component = %predecessor_name,
                        error = %error.to_plain_message(),
                        "could not move a renamed component's rows back during rollback"
                    );
                }
            }
        }
    }
}

/// Record which asset type each `Handle<T>` field of a managed component
/// references, as the derive does for a Rust component.
///
/// The manifest names `T` by its Rust type path, which is what its C# mirror
/// carries. The world records the name that type is registered under, looked up
/// through the asset manager, so a Rust field and a C# field that reference the
/// same asset type agree. A path that no Rust type registered is recorded as
/// written.
fn record_managed_asset_fields(
    engine: &mut Engine,
    component_id: ComponentId,
    component: &ManagedManifestEntry,
) {
    let world = engine.world_mut();
    for field in &component.fields {
        let Some(asset_path) = handle_asset_path(&field.primitive_type) else {
            continue;
        };
        let registered_asset_type_name = world
            .get_resource::<pill_engine::AssetManager>()
            .and_then(|assets| assets.registered_asset_type(asset_path))
            .unwrap_or(asset_path)
            .to_owned();
        world.record_asset_field_type(component_id, &field.name, &registered_asset_type_name);
    }
}

/// The one registration sequence: startup and every reload action (add,
/// rename) go through it, so the component descriptor and the editor-facing
/// field layout cannot be installed by one path and skipped by another. The
/// installed layout is returned for the startup log line that reports it.
///
/// # Errors
///
/// Returns a [`CSharpError`] when the engine refuses the component descriptor
/// or its field layout.
fn register_managed_storage(
    engine: &mut Engine,
    stable_id: StableComponentId,
    component: &ManagedManifestEntry,
) -> Result<(ComponentId, Vec<ComponentFieldDescriptor>), CSharpError> {
    let field_layout = managed_field_layout(&component.full_name, &component.fields);
    let component_id = engine
        .world_mut()
        .register_component_descriptor(
            stable_id.0,
            component.full_name.clone(),
            component.size,
            component.alignment,
            component.schema_hash,
            // `parse_and_validate_manifest` already refused every field type
            // that is not blittable, which is what this proof assumes.
            Blittability::from_manifest_fields(),
        )
        .map_err(|error| CSharpError::ManifestInvalid {
            message: error.to_plain_message(),
        })?;
    record_managed_asset_fields(engine, component_id, component);
    engine
        .world_mut()
        .register_component_descriptor_with_layout(component_id, field_layout.clone())
        .map_err(|error| CSharpError::ManifestInvalid {
            message: error.to_string(),
        })?;
    Ok((component_id, field_layout))
}

/// The binding one arriving manifest entry describes.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
fn managed_binding(
    component_id: ComponentId,
    component: &ManagedManifestEntry,
) -> ComponentBinding {
    ComponentBinding::Managed {
        component_id,
        size: component.size,
        align: component.alignment,
        schema_hash: component.schema_hash,
    }
}

/// Report the fields a migration reset, once per component.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
fn report_retyped_fields(component_name: &str, field_plan: &FieldPlan) {
    if field_plan.retyped_fields().is_empty() {
        return;
    }
    info!(
        target: telemetry_target::HOT_RELOAD,
        component = %component_name,
        fields = %field_plan.retyped_fields().join(", "),
        "managed component fields changed type and were reset to their default bytes"
    );
}

/// Build the plan that puts a component back to a recorded field layout.
///
/// The inverse of [`super::manifest::build_field_plan`] for the rollback path:
/// the engine's current layout is the source, and the descriptors captured
/// before the migration are the destination.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
fn rollback_field_plan(
    engine: &Engine,
    component_id: ComponentId,
    previous_fields: &[ComponentFieldDescriptor],
) -> FieldPlan {
    let live_layout_fields: Vec<LayoutField<'_>> = engine
        .world()
        .component_field_layout(component_id)
        .unwrap_or(&[])
        .iter()
        .map(|field| LayoutField {
            name: field.name,
            type_tag: plan_tag(field.type_tag),
            offset: field.offset,
            size: field.size,
        })
        .collect();
    let recorded_layout_fields: Vec<LayoutField<'_>> = previous_fields
        .iter()
        .map(|field| LayoutField {
            name: field.name,
            type_tag: plan_tag(field.type_tag),
            offset: field.offset,
            size: field.size,
        })
        .collect();
    FieldPlan::between(&live_layout_fields, &recorded_layout_fields)
}
