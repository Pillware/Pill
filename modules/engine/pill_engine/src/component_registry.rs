//! The per-DLL registries for components, value types, mirrored methods, field
//! accessors and named exports.
//!
//! # Responsibilities
//!
//! - Re-export the engine core's field layouts
//!   (`pill_engine_core::reflection::component_registry`) under this path.
//! - Declare the descriptor types the derive and attribute macros submit into
//!   [`inventory`] collections, and the collections themselves.
//! - Provide the artifact-wide registration loop, the schema fingerprint of
//!   the persistable components, and the readers the module/project
//!   entry-point macros and the host call.
//!
//! # Design
//!
//! There is one registry per linked artifact. Every binary and every
//! hot-reload generation DLL carries its own collection, holding exactly the
//! components its own sources declared with `#[derive(PillComponent)]`. That
//! is what makes the registry safe across hot reload: a new generation's
//! `init` sees only the components that generation declares, so a type a
//! newer build stops declaring is never registered again from a stale copy,
//! and an evicted generation DLL takes its descriptors with it.
//!
//! It is per artifact because this crate is: `pill_engine` is compiled into
//! every DLL, while the engine core holds only what is the same for all of
//! them. When the core needs something only a collection knows - a value
//! type's layout - the calling DLL passes it in.
//!
//! Registration order is unspecified (linker/initializer order), and that
//! does no harm: component registration is keyed by `TypeId` and is idempotent.

// Standard library
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine_core::gizmos::{GizmoDeclaration, GizmoRegistry};
/// Field layouts, the layout trait and the value-type resolver, which are the
/// same for every DLL.
pub use pill_engine_core::reflection::component_registry::*;

// Current crate
use crate::error::WorldError;
use crate::{Engine, World};

/// One component type declared with `#[derive(PillComponent)]`.
///
/// The derive macro submits one into this artifact's registry. Every field can
/// be written in a `const`, so the descriptor can live in a `static`.
pub struct PillComponentDescriptor {
    /// Fully-qualified type path: the declaring module's path plus the type's
    /// name.
    pub type_name: &'static str,
    /// Whether the component is persistable (schema-migrated across reloads).
    pub persistable: bool,
    /// Compile-time field layout of the component; empty for components with
    /// no named fields (unit structs, tuple structs).
    pub fields: &'static [ComponentFieldDescriptor],
    /// Registers the component into a world.
    pub register: fn(&mut World),
    /// The gizmo the type declared with `#[pill(gizmo = ...)]`, if any.
    ///
    /// Registration publishes every declaration into the world's
    /// [`GizmoRegistry`], so the engine's gizmo pass never reads an artifact's
    /// inventory itself.
    pub gizmo: Option<GizmoDeclaration>,
}

/// A plain value type (not a component) declared with `#[derive(PillMirror)]`
/// or `pill_value_type!`.
///
/// The derive and the macro submit one into the same per-artifact inventory as
/// [`PillComponentDescriptor`], so the host can resolve `struct:<path>` field
/// tags to typed C# structs when it generates mirrors. It is `Copy` so the
/// module ABI can hand the host a copy of each descriptor. The `fields` slice
/// still points into the declaring artifact's static data, which stays mapped
/// for the artifact's lifetime. It implements `PartialEq` but not `Eq`: the
/// float payloads in a field's presentation decorators have no total equality.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PillValueTypeDescriptor {
    /// Fully-qualified type path: the module path plus the type's name, or the
    /// path a `pill_value_type!` declaration spells.
    pub type_name: &'static str,
    /// The package that declared the type, from `env!("CARGO_PKG_NAME")` at
    /// the macro's expansion site.
    ///
    /// The host uses it to attribute a declaration to the module that owns
    /// it: one artifact that links another crate (an extension depending on
    /// an extension) carries the dependency's submissions too, and only the
    /// owning module's mirror may emit them.
    pub crate_name: &'static str,
    /// Byte size of the value type (`size_of::<T>()`).
    pub size: usize,
    /// Byte alignment of the value type (`align_of::<T>()`).
    pub align: usize,
    /// Field layout of the value type.
    pub fields: &'static [ComponentFieldDescriptor],
}

/// One heap-owning field accessor declared by `#[derive(PillComponent)]`,
/// submitted into the same per-artifact inventory as the other descriptors.
///
/// [`ComponentFieldDescriptor`] says where a field sits in the row. An accessor
/// instead says how the field's buffer is reached: the derive emits one
/// `#[no_mangle]` trampoline per supported operation and records its symbol
/// here. The host resolves those symbols at load time and hands their
/// addresses to the C# runtime under the operation names `<field>_view`,
/// `<field>_resize`, `<field>_set`, `<field>_item`, `<field>_set_item` and
/// `<field>_push`. Each trampoline takes the address of the live component
/// value and works in place: it does not serialize, copy the buffer, or
/// allocate on the read path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PillFieldAccessorDescriptor {
    /// Fully-qualified component type name the field belongs to.
    pub type_name: &'static str,
    /// Rust field name (snake_case); the C# codegen derives member names from
    /// it.
    pub field_name: &'static str,
    /// Container kind: `"vec"`, `"dynbuf"`, `"string"`, or `"vecstring"`.
    ///
    /// `"vecstring"` describes a `Vec<String>`: its elements are separately
    /// allocated strings, so no span exists over them and managed code reaches
    /// each element through its own accessor instead.
    pub kind: &'static str,
    /// Element type tag of a `vec` or `dynbuf` field (`f32`,
    /// `struct:<path>`, ...); `string` for a `vecstring` field; empty for a
    /// `string` field.
    pub element_tag: &'static str,
    /// Exported symbol of the view trampoline
    /// (`pill_accessor_{Type}_{field}_view`). For a `vecstring` field the view
    /// reports a null data pointer and the element count, because its elements
    /// are separate allocations.
    pub view_symbol: &'static str,
    /// Exported symbol of the resize trampoline for a `vec`, `dynbuf` or
    /// `vecstring` field; empty for a `string` field.
    pub resize_symbol: &'static str,
    /// Exported symbol of the mutate-in-place trampoline for a `string` field;
    /// empty for every other kind.
    pub set_symbol: &'static str,
    /// Exported symbol of the per-element view trampoline for a `vecstring`
    /// field (`pill_accessor_{Type}_{field}_item`); empty for every other
    /// kind.
    pub item_symbol: &'static str,
    /// Exported symbol of the per-element replace trampoline for a `vecstring`
    /// field; empty for every other kind.
    pub set_item_symbol: &'static str,
    /// Exported symbol of the append trampoline for a `vecstring` field; empty
    /// for every other kind.
    pub push_symbol: &'static str,
}

/// One mirrored Rust function, or one mirrored type, submitted by the mirror
/// attributes: `#[pill_mirror_impl]`/`#[pill_mirror_fn]` for functions,
/// `#[pill_mirror_object]`/`#[pill_mirror_resource]`/`#[derive(PillMirror)]`
/// for types.
///
/// Every function gets an `unsafe extern "C"` trampoline with the one
/// signature `pill_engine::mirror` defines, `(args, ret) -> status`. Its
/// address is recorded here, so the host needs no symbol table: a statically
/// linked host reads the descriptor directly, and a host that loaded the
/// artifact as a module copies the same address out of the module. The mirror
/// codegen then emits a typed C# member that calls it.
///
/// A row named [`MIRROR_TYPE_ROW`] describes a type rather than a function.
/// Rows whose name starts with `__` are never emitted as C# members: the
/// runtime reaches them by name. The six shapes and the fields each one fills:
///
/// | Shape | `name` | `owner_kind` | `return_tag` | `arg_tags` | `arg_names` | `symbol` / `address` |
/// | --- | --- | --- | --- | --- | --- | --- |
/// | method (`#[pill_mirror_impl]`) | Rust method name | empty ([`OWNER_KIND_UNKNOWN`]; the codegen resolves the owner from the type rows) | return value | argument types | argument names | trampoline |
/// | free function (`#[pill_mirror_fn]`) | function name | `"module"` | return value | argument types | argument names | trampoline |
/// | object type row | [`MIRROR_TYPE_ROW`] | `"object"` | empty | empty | feature flags (`asset`, `import`, `standalone`) | drop trampoline |
/// | object asset-operation row | `__asset_<op>` | `"object"` | empty | empty | empty | the operation's trampoline |
/// | enum type row | [`MIRROR_TYPE_ROW`] | `"enum"` | the representation type | one discriminant per variant | variant names | empty / null |
/// | resource type row | [`MIRROR_TYPE_ROW`] | `"resource"` | empty | empty | `[0]` is the shared name | empty / null |
///
/// The C# codegen reads these rows back in `mirror_codegen.rs`: the object
/// flags are recovered from `arg_names`, the enum's discriminants from
/// `arg_tags`, and the `__asset_*` rows are reached by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PillMethodDescriptor {
    /// Fully-qualified path of the owning type, written the way the macro
    /// spells it: the module path plus the type's name (`pill_spline::OmoMO`).
    pub type_name: &'static str,
    /// Whether the declaration is a free function (`#[pill_mirror_fn]`) rather
    /// than a method (`#[pill_mirror_impl]`). A free-function entry's
    /// `type_name` is the path of the module declaring it (the crate root for
    /// a top-level function), and the C# codegen emits a static method on a
    /// static class named after that module instead of an instance method on a
    /// struct mirror.
    pub is_free_function: bool,
    /// The package that declared the method, from `env!("CARGO_PKG_NAME")` at
    /// the macro's expansion site; see
    /// [`PillValueTypeDescriptor::crate_name`] for how the host uses it.
    pub crate_name: &'static str,
    /// Rust method name, snake_case (`get_sum`); the codegen maps it to
    /// PascalCase for the C# instance method.
    pub name: &'static str,
    /// Rust name of the generated trampoline (`pill_mirror_OmoMO_get_sum`).
    /// The host calls the trampoline through `address`, not by this name.
    pub symbol: &'static str,
    /// Type tag of the return value (`u64`, `str`, `val:Mesh`,
    /// `result:handle:Mesh`, ...; see `pill_engine::mirror`); empty for `()`.
    pub return_tag: &'static str,
    /// Type tags of the arguments after the receiver, in declaration order.
    pub arg_tags: &'static [&'static str],
    /// Argument names from the Rust source, in declaration order (`alpha`,
    /// `beta`), so the generated C# mirror names its parameters identically
    /// instead of inventing `arg0`, `arg1`. Parallel to `arg_tags`; the macro
    /// always emits one name per tag (a positional fallback when the pattern
    /// is not a plain identifier).
    pub arg_names: &'static [&'static str],
    /// How the function takes its owner: `""` not at all (a free or
    /// associated function), `"ref"` as `&self`, `"mut"` as `&mut self`, or
    /// `"value"` as `self`.
    pub receiver: &'static str,
    /// What the owning type is, as one of the [`OWNER_KIND_OBJECT`],
    /// [`OWNER_KIND_ENUM`], [`OWNER_KIND_RESOURCE`], [`OWNER_KIND_MODULE`] or
    /// [`OWNER_KIND_UNKNOWN`] constants.
    pub owner_kind: &'static str,
    /// The trampoline's address; null for a type row with no trampoline.
    pub address: ExportAddress,
}

/// The name of a [`PillMethodDescriptor`] row that describes a type rather
/// than a function.
pub const MIRROR_TYPE_ROW: &str = "__type";

/// A row whose owner is a `#[pill_mirror_object]`.
pub const OWNER_KIND_OBJECT: &str = "object";

/// A row whose owner is a mirrored enum.
pub const OWNER_KIND_ENUM: &str = "enum";

/// A row whose owner is a mirrored resource.
pub const OWNER_KIND_RESOURCE: &str = "resource";

/// A row for a free function, whose owner is the declaring module.
pub const OWNER_KIND_MODULE: &str = "module";

/// A row whose owner kind the attribute could not determine.
pub const OWNER_KIND_UNKNOWN: &str = "";

/// A named C-ABI function an artifact offers to the host, found by name.
///
/// Submit one beside the function with `pill_engine::submit!`, so a host that
/// links the artifact statically - a shipping build, or a host that links a
/// data crate directly - finds it through [`find_export`] with no symbol table
/// to search. A loaded module offers the same function as a `#[no_mangle]`
/// export under the same name, which the host resolves from the DLL instead.
///
/// Keep the shape minimal (name and address): every such function carries its
/// own signature, which the host states where it calls it.
#[derive(Clone, Copy, Debug)]
pub struct PillExportDescriptor {
    /// The function's export name, e.g. `pill_render_data_shader_changed`.
    pub name: &'static str,
    /// The function's address.
    pub address: ExportAddress,
}

/// The address of an exported function, as data.
///
/// A newtype so descriptors can live in a `static` inventory: a raw function
/// pointer is neither `Send` nor `Sync` on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExportAddress(pub *const ());

// SAFETY: the address names immutable code in the image that submitted it; it
// is only ever read and turned back into the function pointer it was made from.
unsafe impl Send for ExportAddress {}
// SAFETY: as above - nothing is written through it.
unsafe impl Sync for ExportAddress {}

inventory::collect!(PillComponentDescriptor);
inventory::collect!(PillValueTypeDescriptor);
inventory::collect!(PillMethodDescriptor);
inventory::collect!(PillExportDescriptor);
inventory::collect!(PillFieldAccessorDescriptor);

/// The address of the export named `export_name` among those linked into the
/// calling artifact, if one was submitted.
///
/// Sees only this artifact's own inventory: a function in a separately loaded
/// module is not found here, and is resolved from that module's DLL instead.
pub fn find_export(export_name: &str) -> Option<ExportAddress> {
    inventory::iter::<PillExportDescriptor>
        .into_iter()
        .find(|descriptor| descriptor.name == export_name)
        .map(|descriptor| descriptor.address)
}

/// Every value type this artifact declares with `#[derive(PillMirror)]` or
/// `pill_value_type!`, sorted by type name so the host consumes them
/// deterministically.
pub fn value_type_descriptors() -> Vec<&'static PillValueTypeDescriptor> {
    let mut descriptors: Vec<&'static PillValueTypeDescriptor> =
        inventory::iter::<PillValueTypeDescriptor>().collect();
    descriptors.sort_by_key(|descriptor| descriptor.type_name);
    descriptors
}

/// Every mirror row this artifact declares: methods
/// (`#[pill_mirror_impl]`/`#[pill_mirror_method]`), free functions
/// (`#[pill_mirror_fn]`) and type rows, sorted by type name then row name so
/// the host consumes them deterministically.
pub fn mirror_method_descriptors() -> Vec<&'static PillMethodDescriptor> {
    let mut descriptors: Vec<&'static PillMethodDescriptor> =
        inventory::iter::<PillMethodDescriptor>().collect();
    descriptors.sort_by_key(|descriptor| (descriptor.type_name, descriptor.name));
    descriptors
}

/// Every heap-field accessor this artifact declares with
/// `#[derive(PillComponent)]`, sorted by component type name then field name
/// so the host consumes them deterministically.
///
/// The returned descriptors live in this artifact's static data, so their
/// symbol strings stay valid for as long as the artifact is mapped.
pub fn field_accessor_descriptors() -> Vec<&'static PillFieldAccessorDescriptor> {
    let mut descriptors: Vec<&'static PillFieldAccessorDescriptor> =
        inventory::iter::<PillFieldAccessorDescriptor>().collect();
    descriptors.sort_by_key(|descriptor| (descriptor.type_name, descriptor.field_name));
    descriptors
}

/// Copy up to `max_descriptor_count` descriptors into `out_descriptors`,
/// returning how many were written.
///
/// The three copy exports - value types, mirror methods, field accessors -
/// answer the same question: the host sizes a buffer from the matching count
/// export, then asks for a copy of at most that many. The clamp and the write
/// loop live here once rather than in each expansion. A maximum larger than
/// the list copies the list; a maximum of zero writes nothing.
///
/// # Safety
///
/// `out_descriptors` must point at `max_descriptor_count` writable `T` slots
/// owned by the caller for the duration of this call, or be null when
/// `max_descriptor_count` is zero.
pub unsafe fn copy_descriptors<T: Copy>(
    descriptors: &[&'static T],
    out_descriptors: *mut T,
    max_descriptor_count: u32,
) -> u32 {
    let descriptor_count = (descriptors.len() as u32).min(max_descriptor_count);
    for (index, descriptor) in descriptors
        .iter()
        .take(descriptor_count as usize)
        .enumerate()
    {
        // SAFETY: `index < descriptor_count <= max_descriptor_count`, so
        // `out_descriptors.add(index)` stays inside the buffer the caller
        // promised, and `T: Copy` makes the write a copy out of the static
        // descriptor.
        unsafe { out_descriptors.add(index).write(**descriptor) };
    }
    descriptor_count
}

/// Register every component this artifact declares with the derive.
///
/// Called by the macro-generated `init` entry point before the user's own
/// registration code runs, so entity seeding and system registration can rely
/// on every component type already being known to the world.
///
/// While the loop runs, this artifact's value types are installed as the
/// world's [`ValueTypeLayoutResolver`], so a component field of a
/// `#[derive(PillMirror)]` type expands into editable rows. The previous
/// resolver is restored afterwards, so none is left pointing into this DLL.
///
/// # Errors
///
/// Returns the first error a registration recorded during the loop - the
/// component-type ceiling, a name collision, or a layout disagreement with an
/// already registered type - so the generated `init` can fail the reload
/// transactionally instead of running with a half-registered component set.
pub fn register_all_components(world: &mut World) -> Result<(), WorldError> {
    // Step 1: install this artifact's value types as the layout resolver for
    // the duration of the loop.
    let previous_resolver = install_value_type_layouts(world);
    // Step 2: register every component this image declares - the entries in
    // front of the baseline the host took before mapping it. The registry
    // lists the newest submission first.
    let baseline = crate::declarations::declaration_baseline().components;
    let mut descriptors: Vec<&PillComponentDescriptor> = inventory::iter::<PillComponentDescriptor>
        .into_iter()
        .collect();
    descriptors.truncate(descriptors.len().saturating_sub(baseline));
    let mut gizmo_declarations: Vec<GizmoDeclaration> = Vec::new();
    for descriptor in descriptors {
        (descriptor.register)(world);
        if let Some(declaration) = descriptor.gizmo {
            gizmo_declarations.push(declaration);
        }
    }
    // Step 2b: hand the declared gizmos to the world. The registry is declared
    // by every generation, so a reload that declares no gizmo at all still
    // claims it and the host keeps the value the last generation published.
    world.register_resource::<GizmoRegistry>();
    if !gizmo_declarations.is_empty() {
        if !world.has_resource::<GizmoRegistry>() {
            world.insert_resource(GizmoRegistry::default());
        }
        if let Some(registry) = world.get_resource_mut::<GizmoRegistry>() {
            registry.publish(&gizmo_declarations);
        }
    }
    // Step 3: put the previous resolver back and report the first error a
    // registration recorded.
    restore_value_type_layouts(world, previous_resolver);
    world.take_registration_error().map_or(Ok(()), Err)
}

/// Install this artifact's value types as the world's field-layout resolver,
/// returning the resolver that was installed before.
///
/// Private on purpose: the raw pair lets a caller forget the restore, which
/// leaves the world holding a function pointer into an image the host may have
/// unmapped. [`ValueTypeLayoutScope`] owns both halves, and
/// [`run_artifact_entry`] is the only code that needs them.
fn install_value_type_layouts(world: &mut World) -> PreviousLayoutResolvers {
    PreviousLayoutResolvers {
        value_types: world.replace_value_type_layout_resolver(Some(resolve_value_type_layout)),
        enums: world.replace_enum_layout_resolver(Some(resolve_enum_layout)),
    }
}

/// Put back the resolvers [`install_value_type_layouts`] replaced.
fn restore_value_type_layouts(world: &mut World, previous_resolvers: PreviousLayoutResolvers) {
    world.replace_value_type_layout_resolver(previous_resolvers.value_types);
    world.replace_enum_layout_resolver(previous_resolvers.enums);
}

/// The layout resolvers a world held before this artifact installed its own.
#[derive(Clone, Copy)]
struct PreviousLayoutResolvers {
    /// The value-type resolver installed before.
    value_types: Option<ValueTypeLayoutResolver>,
    /// The enum resolver installed before.
    enums: Option<EnumLayoutResolver>,
}

/// Holds this artifact's value types installed as the world's layout resolver.
///
/// Restores the previous resolver when it drops, which is the point: an `init`
/// that unwinds would otherwise leave the world holding a resolver that is a
/// function pointer into the generation the host just rejected, and the world
/// would call it after the reload.
struct ValueTypeLayoutScope {
    /// World whose resolver this scope owns.
    ///
    /// Raw because the entry point calls back into the engine between
    /// construction and drop, so no reference to the world can be held for
    /// that whole span.
    world: *mut World,
    /// Resolvers the world held before this scope installed its own.
    previous_resolver: PreviousLayoutResolvers,
}

impl ValueTypeLayoutScope {
    /// Installs this artifact's resolver and takes responsibility for it.
    fn install(engine: &mut Engine) -> Self {
        let world = engine.world_mut();
        let previous_resolver = install_value_type_layouts(world);
        Self {
            world: world as *mut World,
            previous_resolver,
        }
    }
}

impl Drop for ValueTypeLayoutScope {
    fn drop(&mut self) {
        // SAFETY: the scope is created and dropped inside one call to
        // `run_artifact_entry`, which owns the engine for its whole body; no
        // borrow of the world outlives the call, and nothing else observes the
        // pointer in between, so it is still the live world here.
        let world = unsafe { &mut *self.world };
        restore_value_type_layouts(world, self.previous_resolver);
    }
}

/// Run one artifact entry point: the transaction every posture shares.
///
/// The order is the whole contract, and it lives here once so a DLL wrapper, a
/// linked project and anything added later cannot disagree about it:
///
/// 1. this artifact's value types become the world's field-layout resolver;
/// 2. every component the artifact declares is registered;
/// 3. every system the artifact declares is registered, in `(phase, order,
///    name)` order;
/// 4. every script component the artifact declares is registered, with the
///    lifecycle methods its `#[script]` declaration states;
/// 5. every resource the artifact declares is declared, so a `register` half
///    that stores a value finds the declaration already in place;
/// 6. every asset type the artifact declares is declared, so the import
///    registry points its entry points at this image;
/// 7. `before_init` runs - a project loads the startup scene it queued here,
///    because the component types exist and the project has not yet seeded its
///    own entities over them;
/// 8. the artifact's `register` runs - the half a later generation repeats, so
///    the systems it registers belong to the image that is running;
/// 9. the user's one-time `init` runs;
/// 10. the registration-error slot is read once more, for an error recorded
///     after the drain in step 2 looked at it.
///
/// The resolver is held for the whole entry point - a `struct:`-tagged field (a
/// value type, or an array of one) only expands into editable rows while one is
/// installed - and [`ValueTypeLayoutScope`] puts the previous one back on every
/// exit, including an unwind out of `init`.
pub fn run_artifact_entry(
    engine: &mut Engine,
    before_init: impl FnOnce(&mut Engine),
    register: fn(&mut Engine) -> u32,
    init: impl FnOnce(&mut Engine) -> u32,
) -> u32 {
    // Step 1: install this artifact's value types as the world's layout
    // resolver.
    let scope = ValueTypeLayoutScope::install(engine);
    // Steps 2-7: register the components, the declared systems, scripts,
    // resources and asset types, load what `before_init` queued, run the
    // artifact's registration, then its one-time `init`.
    let mut status = match register_all_components(engine.world_mut()) {
        Ok(()) => {
            crate::declarations::register_all_systems(engine);
            crate::declarations::register_all_scripts(engine.world_mut());
            crate::declarations::register_all_resources(engine.world_mut());
            crate::declarations::register_all_assets(engine.world_mut());
            before_init(engine);
            match register(engine) {
                0 => init(engine),
                registration_status => registration_status,
            }
        }
        // The failure was logged; a half-registered artifact must not run.
        Err(_error) => u32::MAX,
    };
    // Step 9: catch a registration error raised after the drain in step 2.
    if status != u32::MAX && engine.world_mut().take_registration_error().is_some() {
        status = u32::MAX;
    }
    drop(scope);
    status
}

/// [`run_artifact_entry`] for a module or a project that loads no startup
/// document.
pub fn run_artifact_init(
    engine: &mut Engine,
    register: fn(&mut Engine) -> u32,
    init: fn(&mut Engine) -> u32,
) -> u32 {
    run_artifact_entry(engine, |_engine| {}, register, init)
}

/// Runs one artifact's registration, and nothing else.
///
/// This is the half a later generation repeats: [the value types and the
/// components](run_artifact_entry) come back with the new image, the declared
/// systems, resources and asset types are declared again, and the artifact's
/// `register` runs last. The one-time work - `before_init`, the user's `init` -
/// is deliberately absent, so a reload cannot seed a world twice.
///
/// Returns zero on success, exactly like the entry points it serves.
pub fn run_artifact_registration(engine: &mut Engine, register: fn(&mut Engine) -> u32) -> u32 {
    let scope = ValueTypeLayoutScope::install(engine);
    let mut status = match register_all_components(engine.world_mut()) {
        Ok(()) => {
            crate::declarations::register_all_systems(engine);
            crate::declarations::register_all_scripts(engine.world_mut());
            crate::declarations::register_all_resources(engine.world_mut());
            crate::declarations::register_all_assets(engine.world_mut());
            register(engine)
        }
        // The failure was logged; a half-registered artifact must not run.
        Err(_error) => u32::MAX,
    };
    if status != u32::MAX && engine.world_mut().take_registration_error().is_some() {
        status = u32::MAX;
    }
    drop(scope);
    status
}

/// The registration function of an artifact that declares none.
///
/// A project that keeps everything in its `init` still loads and still
/// registers its components through the entry point; this is what the host's
/// registration-only call finds in its place.
pub fn no_registration(_engine: &mut Engine) -> u32 {
    0
}

/// [`run_artifact_entry`] for a project, which also loads the startup scene the
/// front end queued.
///
/// The load runs after registration and before the user's `init`: a scene
/// loaded earlier would find no component types to attach, and one loaded later
/// would double every entity the project seeds.
pub fn run_project_init(
    engine: &mut Engine,
    register: fn(&mut Engine) -> u32,
    init: fn(&mut Engine) -> u32,
) -> u32 {
    run_artifact_entry(
        engine,
        |engine| {
            if let Some(Err(reason)) = pill_engine_core::apply_startup_scene(engine) {
                pill_core::warn!(
                    target: telemetry_target::ENGINE,
                    reason,
                    "startup scene not loaded; the project starts without it"
                );
            }
        },
        register,
        init,
    )
}

/// The layout of the value type a `struct:<path>` field tag names, among the
/// ones this artifact declares with `#[derive(PillMirror)]` or
/// `pill_value_type!`.
///
/// The tag is the path as written at the field site, usually an imported
/// short name (`Color`), so an exact type name is tried first and a unique
/// `::<path>` suffix second. An ambiguous suffix resolves to `None`: an opaque
/// field is readable, a wrong one is not.
pub fn resolve_value_type_layout(path: &str) -> Option<&'static [ComponentFieldDescriptor]> {
    let descriptors = value_type_descriptors();
    let resolved_type_name = pill_engine_core::reflection::component_registry::resolve_unique_name(
        path,
        descriptors.iter().map(|descriptor| descriptor.type_name),
    )?;
    descriptors
        .into_iter()
        .find(|descriptor| descriptor.type_name == resolved_type_name)
        .map(|descriptor| descriptor.fields)
}

/// The declared shape of the enum a `struct:<path>` field tag names, among the
/// fieldless enums this artifact declares with `#[derive(PillMirror)]`.
///
/// Uses the same name rule as [`resolve_value_type_layout`]. An enum whose
/// mirror row does not read back (a discriminant that is not an integer) is
/// treated as undeclared, which leaves the field opaque.
pub fn resolve_enum_layout(path: &str) -> Option<EnumLayout> {
    let enum_rows: Vec<&'static PillMethodDescriptor> = mirror_method_descriptors()
        .into_iter()
        .filter(|descriptor| {
            descriptor.owner_kind == OWNER_KIND_ENUM && descriptor.name == MIRROR_TYPE_ROW
        })
        .collect();
    let resolved_type_name = pill_engine_core::reflection::component_registry::resolve_unique_name(
        path,
        enum_rows.iter().map(|descriptor| descriptor.type_name),
    )?;
    let row = enum_rows
        .into_iter()
        .find(|descriptor| descriptor.type_name == resolved_type_name)?;
    // An enum type row carries the representation as its return tag, the
    // discriminants as its argument tags and the variant names as its argument
    // names.
    let mut variants = Vec::with_capacity(row.arg_names.len());
    for (name, discriminant) in row.arg_names.iter().zip(row.arg_tags) {
        variants.push((*name, discriminant.parse::<i64>().ok()?));
    }
    Some(EnumLayout {
        representation: row.return_tag,
        variants,
    })
}

/// Fingerprint of the set of persistable components this artifact declares.
///
/// The value is the hash of every persistable component's type name, taken in
/// sorted order, so two builds that declare the same set produce the same
/// fingerprint. The project entry-point macro exports it as
/// `project_schema_fingerprint`. Keeping it derived from the same registry that
/// drives registration means a new persistable component cannot be missing
/// from the fingerprint.
pub fn persistable_schema_fingerprint() -> u64 {
    let mut descriptors: Vec<&PillComponentDescriptor> = inventory::iter::<PillComponentDescriptor>
        .into_iter()
        .collect();
    descriptors.sort_by_key(|descriptor| descriptor.type_name);

    let mut hasher = DefaultHasher::new();
    for descriptor in descriptors {
        if descriptor.persistable {
            descriptor.type_name.hash(&mut hasher);
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Component, ComponentId, World};

    // These components declare by hand everything the derive would generate,
    // and submit descriptors exactly as the macro does.

    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    struct TestPersistableComponent {
        value: u32,
    }
    impl Component for TestPersistableComponent {}

    fn register_test_persistable(world: &mut World) {
        world.register_persistable_component::<TestPersistableComponent>();
    }

    #[derive(Clone, Debug)]
    struct TestPlainComponent;
    impl Component for TestPlainComponent {}

    fn register_test_plain(world: &mut World) {
        world.register_component::<TestPlainComponent>();
    }

    inventory::submit! {
        PillComponentDescriptor {
            type_name: "pill_engine::component_registry::tests::TestPersistableComponent",
            persistable: true,
            fields: &[],
            register: register_test_persistable,
            gizmo: None,
        }
    }
    inventory::submit! {
        PillComponentDescriptor {
            type_name: "pill_engine::component_registry::tests::TestPlainComponent",
            persistable: false,
            fields: &[],
            register: register_test_plain,
            gizmo: None,
        }
    }

    /// The registration loop registers every submitted component exactly once.
    #[test]
    fn registry_registers_every_submitted_component() {
        let mut world = World::new();
        register_all_components(&mut world).expect("registration must succeed");

        assert!(world
            .component_registry()
            .get_bit(&ComponentId::of::<TestPersistableComponent>())
            .is_some());
        assert!(world
            .component_registry()
            .get_bit(&ComponentId::of::<TestPlainComponent>())
            .is_some());
    }

    /// Registration is idempotent, so running the loop twice is safe.
    #[test]
    fn registry_registration_is_idempotent() {
        let mut world = World::new();
        register_all_components(&mut world).expect("registration must succeed");
        register_all_components(&mut world).expect("re-registration must succeed");

        assert!(world
            .component_registry()
            .get_bit(&ComponentId::of::<TestPersistableComponent>())
            .is_some());
    }

    /// Mirrored-method descriptors submitted through the inventory are
    /// queryable, sorted by type name then method name.
    #[test]
    fn mirror_method_descriptors_are_collected_and_sorted() {
        crate::submit! {
            PillMethodDescriptor {
                type_name: "pill_engine::test::Zulu",
                is_free_function: false,
                crate_name: "pill_engine",
                name: "alpha",
                symbol: "pill_mirror_Zulu_alpha",
                return_tag: "u64",
                arg_tags: &[],
                arg_names: &[],
                receiver: "ref",
                owner_kind: "value",
                address: ExportAddress(std::ptr::null()),
            }
        }
        crate::submit! {
            PillMethodDescriptor {
                type_name: "pill_engine::test::Alpha",
                is_free_function: false,
                crate_name: "pill_engine",
                name: "beta",
                symbol: "pill_mirror_Alpha_beta",
                return_tag: "u32",
                arg_tags: &["f32", "u8"],
                arg_names: &["blend", "count"],
                receiver: "ref",
                owner_kind: "value",
                address: ExportAddress(std::ptr::null()),
            }
        }

        let methods = mirror_method_descriptors();
        let mut matching: Vec<(&'static str, &'static str)> = methods
            .iter()
            .filter(|descriptor| descriptor.type_name.starts_with("pill_engine::test::"))
            .map(|descriptor| (descriptor.type_name, descriptor.name))
            .collect();
        matching.dedup_by(|left, right| left == right);
        assert_eq!(
            matching,
            vec![
                ("pill_engine::test::Alpha", "beta"),
                ("pill_engine::test::Zulu", "alpha"),
            ]
        );

        let beta = methods
            .iter()
            .find(|descriptor| descriptor.name == "beta")
            .expect("the submitted beta descriptor must be present");
        assert_eq!(beta.return_tag, "u32");
        assert_eq!(beta.arg_tags, &["f32", "u8"]);
        assert_eq!(beta.arg_names, &["blend", "count"]);
        assert_eq!(beta.symbol, "pill_mirror_Alpha_beta");
    }

    /// The persistable-only fingerprint is stable across calls.
    #[test]
    fn persistable_fingerprint_is_deterministic() {
        let first = persistable_schema_fingerprint();
        let second = persistable_schema_fingerprint();
        assert_eq!(first, second);
    }
}
