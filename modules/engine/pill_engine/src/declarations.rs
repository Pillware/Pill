//! The systems, resources and assets an artifact declares, as opposed to the
//! ones it registers by hand.
//!
//! # Responsibilities
//!
//! - Declare the descriptor types the `#[system]` attribute, the `#[script]`
//!   attribute and the `#[derive(PillResource)]`/`#[derive(PillAsset)]`
//!   derives submit into [`inventory`], and the collections themselves.
//! - Turn those collections into registered systems, registered scripts,
//!   declared resources and declared asset types at the artifact's entry
//!   point, in a deterministic order.
//! - Refuse an artifact that declares one type name twice within a kind
//!   ([`check_declared_names_unique`]), because a stored name is
//!   `<crate>::<Type>` and two types of one name would share one identity.
//!
//! # Design
//!
//! This is the shape [`component_registry`](crate::component_registry) gives
//! components, applied to systems, resources and assets. A macro submits one
//! descriptor per declaration into a process-wide collection, and the entry
//! point walks that collection once per generation.
//!
//! The collection is shared by every image mapped in the process: it lives in
//! the engine dylib the artifacts link, and a hot reload keeps the retired
//! images mapped so their pointers stay valid. The entry point therefore keeps
//! only the declarations an image added after the [`DeclarationBaseline`]
//! taken before it was loaded, so a new generation registers its own
//! declarations and never the retired generation's. A static build and a test
//! binary register everything.
//!
//! Registration order decides the order conflicting systems run in, and
//! `inventory` order is link order rather than source order. The system
//! descriptor therefore carries an explicit order key, the phase and a name to
//! break ties, so the schedule a project gets is the schedule its declarations
//! describe. Resources and asset types have no execution order; they are
//! declared sorted by type name so the claim order is the same in every build.
//!
//! # Refused declarations
//!
//! The attribute refuses an argument it cannot honour, and names the ones it
//! accepts. An unknown phase spelling is refused that way rather than guessed:
//!
//! ```compile_fail
//! use pill_engine::*;
//!
//! #[system(name = "bad_phase", run_phase = "pre_frame")]
//! fn bad_phase() {}
//! ```
//!
//! A script component carries its declaration in `#[script]`, so leaving the
//! attribute out is a compile error that names the missing declaration rather
//! than a script the engine silently never runs:
//!
//! ```compile_fail
//! use pill_engine::*;
//!
//! #[derive(Debug)]
//! struct NoDeclaration;
//!
//! impl Component for NoDeclaration {}
//!
//! impl ScriptComponent for NoDeclaration {}
//! ```
//!
//! The attribute reads lifecycle methods out of a `ScriptComponent` block, so a
//! block that implements something else is refused instead of registering
//! nothing:
//!
//! ```compile_fail
//! use pill_engine::*;
//!
//! #[derive(Debug)]
//! struct NotAScript;
//!
//! #[script]
//! impl Component for NotAScript {}
//! ```

// Standard library
use std::cell::Cell;
use std::collections::{HashMap, HashSet};

// Current crate
use crate::component_registry::{ComponentFieldDescriptor, PillComponentDescriptor};
use crate::{Engine, RunPhase, World};

// =============================================================================
// Declaration Baseline
// =============================================================================

/// How many declarations of each kind the process held when an artifact's
/// image was mapped.
///
/// The registry lists the newest submission first, so an entry point keeps the
/// first `total - baseline` entries: exactly the declarations its image added.
/// The declarations the process already holds - an earlier module's, or a
/// generation the host keeps mapped after a reload - are left alone. A
/// statically linked build and a test binary have no baseline and keep
/// everything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeclarationBaseline {
    /// Components declared before the image was mapped.
    pub components: usize,
    /// Systems declared before the image was mapped.
    pub systems: usize,
    /// Script components declared before the image was mapped.
    pub scripts: usize,
    /// Resources declared before the image was mapped.
    pub resources: usize,
    /// Asset types declared before the image was mapped.
    pub assets: usize,
    /// Value types declared before the image was mapped.
    pub value_types: usize,
}

impl DeclarationBaseline {
    /// Counts every declaration the process holds now.
    pub fn current() -> Self {
        Self {
            components: inventory::iter::<PillComponentDescriptor>
                .into_iter()
                .count(),
            systems: inventory::iter::<PillSystemDescriptor>.into_iter().count(),
            scripts: inventory::iter::<PillScriptDescriptor>.into_iter().count(),
            resources: inventory::iter::<PillResourceDescriptor>
                .into_iter()
                .count(),
            assets: inventory::iter::<PillAssetDescriptor>.into_iter().count(),
            value_types: inventory::iter::<crate::component_registry::PillValueTypeDescriptor>
                .into_iter()
                .count(),
        }
    }
}

thread_local! {
    /// The position the current registration walk starts at, when one is set.
    static DECLARATION_BASELINE: Cell<Option<DeclarationBaseline>> = const { Cell::new(None) };
}

/// Runs `register` with `baseline` as the number of declarations the process
/// already held.
///
/// The host wraps one artifact entry point in this call with the baseline it
/// took before mapping that artifact's image, so the walk keeps only the
/// declarations that image added. Everything else - a static build's entry
/// point, a test, a module the host links - runs without one and registers
/// every declaration the process holds.
pub fn with_declaration_baseline<R>(
    baseline: DeclarationBaseline,
    register: impl FnOnce() -> R,
) -> R {
    let previous = DECLARATION_BASELINE.with(|cell| cell.replace(Some(baseline)));
    let result = register();
    DECLARATION_BASELINE.with(|cell| cell.set(previous));
    result
}

/// The baseline the current registration walk starts at, or zero.
pub fn declaration_baseline() -> DeclarationBaseline {
    DECLARATION_BASELINE.with(Cell::get).unwrap_or_default()
}

/// Whether a registration walk is running under a declaration baseline.
///
/// Without one, the declaration collections hold everything the process
/// carries - several images' submissions - so a name can repeat because one
/// crate was linked into more than one image rather than because two types
/// share it.
pub fn has_declaration_baseline() -> bool {
    DECLARATION_BASELINE.with(Cell::get).is_some()
}

// =============================================================================
// Types
// =============================================================================

/// One ECS system declared with `#[system]`.
pub struct PillSystemDescriptor {
    /// Name the system is registered under, which is what logs, the editor's
    /// Systems tab and a hot patch all use to find it.
    pub name: &'static str,
    /// Sort key within the phase; a lower value registers first. Equal values
    /// are ordered by [`Self::name`] here, and by registration across
    /// artifacts.
    pub order: i32,
    /// The phase of the frame the system runs in.
    ///
    /// The walk sorts by it first, so a post-project declaration is registered
    /// after every project one whatever its order number is.
    pub phase: RunPhase,
    /// Registers the system with the engine.
    ///
    /// A function pointer rather than the system itself: the attribute emits
    /// the call, so the system's parameter tuple is inferred at its own
    /// declaration site, exactly as a hand-written
    /// `register_system_with_options` call would do.
    pub register: fn(&mut Engine),
}

inventory::collect!(PillSystemDescriptor);

/// One script component declared with `#[script]`.
pub struct PillScriptDescriptor {
    /// Registers the script with a world.
    ///
    /// A function pointer rather than the type itself: the attribute emits the
    /// call, so the component type is named at its own declaration site.
    pub register: fn(&mut World),
}

inventory::collect!(PillScriptDescriptor);

/// One ECS resource declared with `#[derive(PillResource)]`.
pub struct PillResourceDescriptor {
    /// The type's stored name, `<crate>::<Type>`.
    pub type_name: &'static str,
    /// Declares the resource with a world.
    ///
    /// A function pointer rather than the type itself: the derive emits the
    /// call, so whether the type is a plain resource or a manager resource is
    /// decided where the type is declared.
    pub register: fn(&mut World),
}

inventory::collect!(PillResourceDescriptor);

/// One asset type declared with `#[derive(PillAsset)]`.
pub struct PillAssetDescriptor {
    /// The type's stored name, `<crate>::<Type>`.
    pub type_name: &'static str,
    /// Declares the asset type with a world.
    ///
    /// A function pointer rather than the type itself: the derive emits the
    /// call, so whether the type imports from a source file or is its own
    /// document file is decided where the type is declared.
    pub register: fn(&mut World),
}

inventory::collect!(PillAssetDescriptor);

/// A type name this artifact declares twice within one kind of declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateDeclaredName {
    /// The kind both declarations are: `component`, `resource`, `asset type`
    /// or `value type`.
    pub kind: &'static str,
    /// The name both declarations carry.
    pub type_name: &'static str,
}

/// What one declaration says about its type's shape.
///
/// A name can repeat because the same type arrived twice - one crate linked
/// into two images of one build - which is harmless, or because two different
/// types share it, which is the collision. The shape is what tells them apart.
/// Not `Eq`: a field decorator can carry a float.
#[derive(Clone, Copy, PartialEq)]
enum DeclaredShape {
    /// A component: its persistable flag and field layout.
    Component {
        persistable: bool,
        fields: &'static [ComponentFieldDescriptor],
    },
    /// A mirrored value type: its size, alignment and field layout.
    ValueType {
        size: usize,
        align: usize,
        fields: &'static [ComponentFieldDescriptor],
    },
}

// =============================================================================
// Free Functions
// =============================================================================

/// Registers every system this artifact declares.
///
/// Called by the artifact entry points before the artifact's own `register`
/// runs, so the same declarations reach a first load, every reload and a
/// statically linked build.
///
/// Systems are registered in `(phase, order, name)` order. The engine keeps
/// its whole list sorted by `(phase, order)` and equal pairs in registration
/// order, so this walk only has to make equal orders deterministic within one
/// artifact; across artifacts the engine's insertion order decides.
pub fn register_all_systems(engine: &mut Engine) {
    // Step 1: Collect this image's declarations - the entries in front of the
    // baseline, because the registry lists the newest submission first - and
    // sort them into the order the schedule should run them in.
    let baseline = declaration_baseline().systems;
    let mut descriptors: Vec<&'static PillSystemDescriptor> =
        inventory::iter::<PillSystemDescriptor>
            .into_iter()
            .collect();
    descriptors.truncate(descriptors.len().saturating_sub(baseline));
    descriptors.sort_by(|left, right| {
        left.phase
            .cmp(&right.phase)
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| left.name.cmp(right.name))
    });
    // Step 2: Register each one through the function the attribute emitted.
    for descriptor in descriptors {
        (descriptor.register)(engine);
    }
}

/// Registers every script component this artifact declares.
///
/// Called by the artifact entry points after [`register_all_systems`]. A
/// script's registration also registers its component type, and records the
/// lifecycle methods its `#[script]` declaration states, so a first load, every
/// reload and a statically linked build all point the lifecycle method table at
/// the image that is running.
///
/// The walk's order does not matter: the update pass reads each lifecycle
/// method's own order from the registration, and equal orders are broken by
/// type name rather than by registration order.
pub fn register_all_scripts(world: &mut World) {
    // Step 1: Collect this image's declarations.
    let baseline = declaration_baseline().scripts;
    let mut descriptors: Vec<&'static PillScriptDescriptor> =
        inventory::iter::<PillScriptDescriptor>
            .into_iter()
            .collect();
    descriptors.truncate(descriptors.len().saturating_sub(baseline));
    // Step 2: Register each one through the function the attribute emitted.
    for descriptor in descriptors {
        (descriptor.register)(world);
    }
}

/// Declares every resource this artifact declares.
///
/// Called by the artifact entry points after [`register_all_systems`], so a
/// first load, every reload and a statically linked build all declare the same
/// types before the artifact's own `register` runs. That ordering is what lets
/// `insert_resource` in `register` find the declaration already in place: an
/// inserted value replaces a stored one, it does not declare the type.
///
/// Declarations are sorted by type name. Unlike systems they have no execution
/// order, and the sort keeps the claim order - and so the order any claim error
/// is recorded in - the same in every build.
pub fn register_all_resources(world: &mut World) {
    // Step 1: Collect this image's declarations in a stable order.
    let baseline = declaration_baseline().resources;
    let mut descriptors: Vec<&'static PillResourceDescriptor> =
        inventory::iter::<PillResourceDescriptor>
            .into_iter()
            .collect();
    descriptors.truncate(descriptors.len().saturating_sub(baseline));
    descriptors.sort_by_key(|descriptor| descriptor.type_name);
    // Step 2: Declare each one through the function the derive emitted.
    for descriptor in descriptors {
        (descriptor.register)(world);
    }
}

/// Declares every asset type this artifact declares.
///
/// Called by the artifact entry points after [`register_all_resources`]. An
/// imported asset's declaration also records the type's import entry points,
/// which live in this image, so the declaration is what keeps the world's
/// import registry pointing at code that is still mapped.
pub fn register_all_assets(world: &mut World) {
    // Step 1: Collect this image's declarations in a stable order.
    let baseline = declaration_baseline().assets;
    let mut descriptors: Vec<&'static PillAssetDescriptor> =
        inventory::iter::<PillAssetDescriptor>.into_iter().collect();
    descriptors.truncate(descriptors.len().saturating_sub(baseline));
    descriptors.sort_by_key(|descriptor| descriptor.type_name);
    // Step 2: Declare each one through the function the derive emitted.
    for descriptor in descriptors {
        (descriptor.register)(world);
    }
}

/// Checks that this artifact declares every type name once per kind.
///
/// A declared type is named `<crate>::<Type>`, without its module path, so two
/// types of one name in different modules of a crate would claim one identity:
/// one column, one scene key, one C# struct. A macro sees only its own item and
/// cannot catch that, so the artifact entry points check here, before anything
/// registers. Kinds are checked separately, because one type may legitimately
/// be, for example, both a mirrored value type and an asset.
///
/// The same type can legitimately arrive twice: one crate linked into two
/// images of one build submits its descriptors in each, and a walk without a
/// [`declaration_baseline`] sees both. Components and value types carry their
/// layout, so a repeat whose shape agrees is read as that second copy. A
/// resource or an asset descriptor carries no layout, so a repeat of one of
/// those can only be checked when the walk is scoped to one image.
///
/// # Errors
///
/// Returns the first [`DuplicateDeclaredName`] found.
pub fn check_declared_names_unique() -> Result<(), DuplicateDeclaredName> {
    let baseline = declaration_baseline();

    let mut component_descriptors: Vec<&PillComponentDescriptor> =
        inventory::iter::<PillComponentDescriptor>
            .into_iter()
            .collect();
    component_descriptors.truncate(
        component_descriptors
            .len()
            .saturating_sub(baseline.components),
    );
    let mut component_shapes: HashMap<&'static str, DeclaredShape> = HashMap::new();
    for descriptor in component_descriptors {
        let shape = DeclaredShape::Component {
            persistable: descriptor.persistable,
            fields: descriptor.fields,
        };
        match component_shapes.get(descriptor.type_name) {
            Some(previous) if *previous == shape => {}
            Some(_) => {
                return Err(DuplicateDeclaredName {
                    kind: "component",
                    type_name: descriptor.type_name,
                })
            }
            None => {
                component_shapes.insert(descriptor.type_name, shape);
            }
        }
    }

    // The inventory's own order, not `value_type_descriptors()`: that helper
    // sorts for the host's ABI, and the baseline cut reads the newest
    // submissions first.
    let mut value_type_descriptors: Vec<&crate::component_registry::PillValueTypeDescriptor> =
        inventory::iter::<crate::component_registry::PillValueTypeDescriptor>()
            .into_iter()
            .collect();
    value_type_descriptors.truncate(
        value_type_descriptors
            .len()
            .saturating_sub(baseline.value_types),
    );
    let mut value_type_shapes: HashMap<&'static str, DeclaredShape> = HashMap::new();
    for descriptor in value_type_descriptors {
        let shape = DeclaredShape::ValueType {
            size: descriptor.size,
            align: descriptor.align,
            fields: descriptor.fields,
        };
        match value_type_shapes.get(descriptor.type_name) {
            Some(previous) if *previous == shape => {}
            Some(_) => {
                return Err(DuplicateDeclaredName {
                    kind: "value type",
                    type_name: descriptor.type_name,
                })
            }
            None => {
                value_type_shapes.insert(descriptor.type_name, shape);
            }
        }
    }

    // Without a baseline the lists hold every image's declarations, where one
    // crate linked twice repeats names by design; only a scoped walk can tell
    // a collision from a copy for the kinds that carry no layout.
    if has_declaration_baseline() {
        let mut resource_descriptors: Vec<&PillResourceDescriptor> =
            inventory::iter::<PillResourceDescriptor>
                .into_iter()
                .collect();
        resource_descriptors.truncate(
            resource_descriptors
                .len()
                .saturating_sub(baseline.resources),
        );
        let mut asset_descriptors: Vec<&PillAssetDescriptor> =
            inventory::iter::<PillAssetDescriptor>.into_iter().collect();
        asset_descriptors.truncate(asset_descriptors.len().saturating_sub(baseline.assets));
        let opaque_kinds: [(&'static str, Vec<&'static str>); 2] = [
            (
                "resource",
                resource_descriptors
                    .into_iter()
                    .map(|descriptor| descriptor.type_name)
                    .collect(),
            ),
            (
                "asset type",
                asset_descriptors
                    .into_iter()
                    .map(|descriptor| descriptor.type_name)
                    .collect(),
            ),
        ];
        for (kind, names) in opaque_kinds {
            if let Some(duplicate) = first_repeated_name(kind, names) {
                return Err(duplicate);
            }
        }
    }
    Ok(())
}

/// Returns the first name in `names` that one kind of declaration carries
/// twice.
fn first_repeated_name(
    kind: &'static str,
    names: impl IntoIterator<Item = &'static str>,
) -> Option<DuplicateDeclaredName> {
    let mut seen_names = HashSet::new();
    for type_name in names {
        if !seen_names.insert(type_name) {
            return Some(DuplicateDeclaredName { kind, type_name });
        }
    }
    None
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A name one kind carries twice is refused, and the refusal says which
    /// kind and which name.
    #[test]
    fn a_name_declared_twice_is_reported_with_its_kind() {
        let duplicate = first_repeated_name("component", ["a::One", "a::Two", "a::One"])
            .expect("the repeat is found");
        assert_eq!(duplicate.kind, "component");
        assert_eq!(duplicate.type_name, "a::One");
    }

    /// Distinct names are accepted, in any order.
    #[test]
    fn distinct_names_are_accepted() {
        assert!(first_repeated_name("resource", ["a::One", "a::Two", "b::One"]).is_none());
        assert!(first_repeated_name("asset type", std::iter::empty()).is_none());
    }
}
