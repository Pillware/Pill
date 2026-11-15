//! The systems, resources and assets an artifact declares, as opposed to the
//! ones it registers by hand.
//!
//! # Responsibilities
//!
//! - Declare the descriptor types the `#[system]` attribute and the
//!   `#[derive(PillResource)]`/`#[derive(PillAsset)]` derives submit into
//!   [`inventory`], and the collections themselves.
//! - Turn those collections into registered systems, declared resources and
//!   declared asset types at the artifact's entry point, in a deterministic
//!   order.
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
//! descriptor therefore carries an explicit order key and a name to break
//! ties, so the schedule a project gets is the schedule its declarations
//! describe. Resources and asset types have no execution order; they are
//! declared sorted by type name so the claim order is the same in every build.

// Standard library
use std::cell::Cell;

// Current crate
use crate::component_registry::PillComponentDescriptor;
use crate::{Engine, World};

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
    /// Resources declared before the image was mapped.
    pub resources: usize,
    /// Asset types declared before the image was mapped.
    pub assets: usize,
}

impl DeclarationBaseline {
    /// Counts every declaration the process holds now.
    pub fn current() -> Self {
        Self {
            components: inventory::iter::<PillComponentDescriptor>
                .into_iter()
                .count(),
            systems: inventory::iter::<PillSystemDescriptor>.into_iter().count(),
            resources: inventory::iter::<PillResourceDescriptor>
                .into_iter()
                .count(),
            assets: inventory::iter::<PillAssetDescriptor>.into_iter().count(),
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

// =============================================================================
// Types
// =============================================================================

/// One ECS system declared with `#[system]`.
pub struct PillSystemDescriptor {
    /// Name the system is registered under, which is what logs, the editor's
    /// Systems tab and a hot patch all use to find it.
    pub name: &'static str,
    /// Sort key; a lower value registers first. Equal values are ordered by
    /// [`Self::name`].
    pub order: i32,
    /// Whether the system runs in the post-update stage, after the command
    /// flush.
    pub post_update: bool,
    /// Whether the system also runs while the editor is editing, and while the
    /// engine is paused.
    ///
    /// The editor's own systems and the renderer opt in; a game system leaves
    /// it false, which is what makes Play stop the game rather than only
    /// painting it differently.
    pub edit_mode: bool,
    /// Registers the system with the engine.
    ///
    /// A function pointer rather than the system itself: the attribute emits
    /// the call, so the system's parameter tuple is inferred at its own
    /// declaration site, exactly as a hand-written `register_system` call
    /// would do.
    pub register: fn(&mut Engine),
}

inventory::collect!(PillSystemDescriptor);

/// One ECS resource declared with `#[derive(PillResource)]`.
pub struct PillResourceDescriptor {
    /// Fully-qualified type path: the declaring module's path plus the type's
    /// name.
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
    /// Fully-qualified type path: the declaring module's path plus the type's
    /// name.
    pub type_name: &'static str,
    /// Declares the asset type with a world.
    ///
    /// A function pointer rather than the type itself: the derive emits the
    /// call, so whether the type imports from a source file or is its own
    /// document file is decided where the type is declared.
    pub register: fn(&mut World),
}

inventory::collect!(PillAssetDescriptor);

// =============================================================================
// Free Functions
// =============================================================================

/// Registers every system this artifact declares.
///
/// Called by the artifact entry points before the artifact's own `register`
/// runs, so the same declarations reach a first load, every reload and a
/// statically linked build.
///
/// Systems are registered in `(order, name)` order. The engine appends each
/// one to its list and the scheduler batches by that order, so this is what
/// keeps the execution order of conflicting systems stable across builds
/// instead of following the linker.
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
        left.order
            .cmp(&right.order)
            .then_with(|| left.name.cmp(right.name))
    });
    // Step 2: Register each one through the function the attribute emitted.
    for descriptor in descriptors {
        (descriptor.register)(engine);
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
///
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
