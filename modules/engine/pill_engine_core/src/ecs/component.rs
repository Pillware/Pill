//! Component trait, type identification, and change-detection primitives.
//!
//! # Responsibilities
//!
//! - Defines the [`Component`] marker trait required by all ECS component types.
//! - Provides [`ComponentId`] for type-erased component identification.
//! - Implements [`Tick`] and [`ComponentTicks`] for frame-based change detection.
//! - Manages the [`ComponentRegistry`] that assigns bit indices for archetype masks.
//! - Defines [`ComponentMask`] (u128) for O(1) archetype matching.
//!
//! # Design
//!
//! A component type is registered at runtime and gets one bit in a 128-bit
//! mask. Each archetype carries the mask of its component set, and a query
//! builds a mask from the types it asks for. Matching an archetype is then one
//! bitwise AND, whatever the number of types. Change detection uses one world
//! tick that moves forward each frame. Each component row records the tick
//! when it was added and when it last changed, and a filter compares those
//! ticks with the tick at which the calling system last ran.

// Standard library
use std::any::TypeId;
use std::collections::HashMap;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_core::warn;

// Current crate
use crate::component_registry::ComponentFieldDescriptor;
use crate::error::WorldError;

// =============================================================================
// Constants
// =============================================================================

/// Half the tick range, which is where the wrapping comparison stops telling
/// "ahead" from "behind".
///
/// A distance larger than this half is read as a tick in the other direction,
/// so two ticks more than `u32::MAX / 2` apart cannot be ordered. That is the
/// limit the wrapping comparison accepts, and it is far longer than any single
/// frame.
const TICK_WRAP_HALF_RANGE: u32 = u32::MAX / 2;

/// Salts that keep the two 128-bit identity variants apart when they hold the
/// same value.
///
/// The values are arbitrary odd constants; only the fact that they differ
/// matters.
const DESCRIPTOR_HASH_SALT: u64 = 0x9e37_79b9_7f4a_7c15;
const SHARED_HASH_SALT: u64 = 0xbf58_476d_1ce4_e5b9;

// =============================================================================
// Component
// =============================================================================

/// Marker trait that every ECS component type implements.
///
/// A component is `Send` and `'static`, nothing more. `Send` is enough for a
/// mutable query because the scheduler never runs two systems that write the
/// same component at the same time, and inside one system each thread works on
/// its own rows. A read query is different: two readers of one component do not
/// conflict, so they can run at the same time. That is why
/// [`QueryTarget`](crate::query::QueryTarget) is implemented for `&T` only when
/// `T: Sync`. The bound sits on the query rather than on this trait, because a
/// component that is only ever written never needs to be `Sync`.
///
/// A component with interior mutability ([`Cell`](std::cell::Cell),
/// [`RefCell`](std::cell::RefCell), ...) is not `Sync`, so it cannot be read
/// through an `&T` query. Keep the mutable part in a separate component and
/// reach it with a `&mut T` query.
///
/// An `&T` query over such a component does not compile:
///
/// ```compile_fail
/// # use pill_engine::*;
/// use std::cell::Cell;
///
/// struct Stats {
///     hits: Cell<u32>,
/// }
/// impl Component for Stats {}
///
/// # let mut world = World::new();
/// # world.register_component::<Stats>();
/// let _query = Query::<&Stats>::new(&mut world);
/// ```
///
/// # Examples
///
/// ```
/// # use pill_engine::component::Component;
/// struct Position { x: f32, y: f32 }
///
/// impl Component for Position {}
/// ```
pub trait Component: Send + 'static {
    /// The stable name this component is known by across binaries, or `None`
    /// for the ordinary per-binary identity.
    ///
    /// A component type linked into more than one binary in the same process -
    /// the host, the project, a module DLL - gets a different [`TypeId`] in each
    /// binary. `TypeId` is built from the crate name, its `-C metadata`
    /// disambiguator and the type path, so every build produces its own value.
    /// One type then becomes several components with several columns, and
    /// neither binary sees the entities the other created. Declaring a name
    /// replaces that identity with one derived from the name itself. Every
    /// binary computes it the same way, so all of them end up with one
    /// [`ComponentId`], one mask bit and one column.
    ///
    /// The name must be unique across the process (`"pill_spline::Spline"`, not
    /// `"Spline"`) and must never change. It is written down by hand rather than
    /// taken from [`std::any::type_name`], because that output is not guaranteed
    /// to stay the same across compiler versions.
    ///
    /// Set it with `#[pill(shared)]` on `#[derive(PillComponent)]` rather than
    /// by hand. See [`ComponentId::of`] for what a name changes.
    ///
    /// The method is gated on `where Self: Sized` instead of being an associated
    /// constant: an associated constant would make `Component` unusable as a
    /// trait object, and the storage layer does move components as
    /// `dyn Component`.
    fn shared_name() -> Option<&'static str>
    where
        Self: Sized,
    {
        None
    }

    /// The stable identity that [`Self::shared_name`] hashes to, or `None`.
    ///
    /// The default builds it from `shared_name`, so an implementor only writes
    /// the name and the two cannot disagree. `#[derive(PillComponent)]` replaces
    /// the default with a compile-time constant. That matters because
    /// `ComponentId::of` runs on every `get_component`: hashing the name there
    /// costs measurable time, while a constant costs one load, the same as
    /// `TypeId::of`.
    ///
    /// Override it only with `shared_component_identity(Self::shared_name())`.
    /// Any other value silently gives the component an identity that no other
    /// binary derives from its name.
    fn shared_identity() -> Option<u128>
    where
        Self: Sized,
    {
        Self::shared_name().map(shared_component_identity)
    }

    /// The structural hash of the field layout that *this binary's* type
    /// declares, or `None` when it declares none.
    ///
    /// A query that targets a shared component compares this hash with the one
    /// the registry holds (see [`crate::Query::new`]). The column itself can only
    /// check size and alignment, and those cannot tell `{x: f32, y: f32}` from
    /// `{y: f32, x: f32}`. Without the hash, a binary still running code built
    /// against an older layout - a module whose data crate was reloaded under
    /// it - would read the new rows through the old field offsets.
    ///
    /// `#[derive(PillComponent)]` overrides it using the type's field
    /// descriptors, the same ones registration hashes. A hand-written impl calls
    /// `component_schema_hash(Self::FIELD_LAYOUT)`. The default declares nothing,
    /// which leaves the size-and-alignment check as the only check.
    fn declared_schema_hash() -> Option<u64>
    where
        Self: Sized,
    {
        None
    }
}

// =============================================================================
// Tick
// =============================================================================

/// A counter that says whether a component is new or changed.
///
/// The world keeps one counter and increases it once per frame, and also when a
/// write happens between frames. Each component row carries its own
/// `ComponentTicks`, which record when the row was added and when it was last
/// changed. A system later compares those ticks with its own `last_run` tick to
/// find new or changed rows.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Tick(pub u32);

impl Tick {
    // -------------------------------------------------------------------------
    // Construction
    // -------------------------------------------------------------------------

    /// Constructs a tick with an explicit counter value.
    #[inline]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    // -------------------------------------------------------------------------
    // Queries
    // -------------------------------------------------------------------------

    /// Returns the underlying counter value.
    #[inline]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Returns `true` when this tick is strictly newer than `last_run` and not
    /// in the future relative to `this_run`.
    ///
    /// The counter is a `u32` that wraps, so both comparisons use the distance
    /// from the baseline instead of the numbers themselves. A tick written just
    /// after a wrap is *smaller* than the baseline, but only a few ticks ahead
    /// of it. A distance in the upper half of the range means the other
    /// direction. That is how the comparison tells the two directions apart
    /// without knowing where the counter is.
    #[inline]
    pub fn is_newer_than(self, last_run: Tick, this_run: Tick) -> bool {
        let since_last_run = self.0.wrapping_sub(last_run.0);
        let since_this_run = self.0.wrapping_sub(this_run.0);
        since_last_run != 0
            && since_last_run <= TICK_WRAP_HALF_RANGE
            && (since_this_run == 0 || since_this_run > TICK_WRAP_HALF_RANGE)
    }
}

// =============================================================================
// ComponentTicks
// =============================================================================

/// The change-detection ticks of one component row.
///
/// Each component column keeps a `Vec<ComponentTicks>` beside its row data, so
/// the ticks for row `i` sit at index `i`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ComponentTicks {
    /// Tick at which this component was added to its current entity.
    pub added: Tick,
    /// Tick at which this component was most recently mutated through `Mut<T>`.
    pub changed: Tick,
}

impl ComponentTicks {
    // -------------------------------------------------------------------------
    // Construction
    // -------------------------------------------------------------------------

    /// Creates new ticks with both `added` and `changed` set to the given tick.
    #[inline]
    pub fn new(tick: Tick) -> Self {
        Self {
            added: tick,
            changed: tick,
        }
    }

    /// Returns `true` when this component was added between `last_run` and
    /// `this_run`.
    #[inline]
    pub fn is_added(&self, last_run: Tick, this_run: Tick) -> bool {
        self.added.is_newer_than(last_run, this_run)
    }

    /// Returns `true` when this component changed, or was added, between
    /// `last_run` and `this_run`.
    #[inline]
    pub fn is_changed(&self, last_run: Tick, this_run: Tick) -> bool {
        self.changed.is_newer_than(last_run, this_run)
    }

    // -------------------------------------------------------------------------
    // Mutations
    // -------------------------------------------------------------------------

    /// Sets the `changed` tick directly, bypassing the normal `Mut<T>` path.
    #[inline]
    pub fn set_changed(&mut self, tick: Tick) {
        self.changed = tick;
    }
}

// =============================================================================
// ComponentId
// =============================================================================

/// Identifier for a registered component type, with the Rust type erased.
///
/// A native component keeps its Rust [`TypeId`]. A runtime-defined component
/// uses the stable 128-bit identity its external manifest supplies. A native
/// component that declares [`Component::shared_name`] uses a stable identity
/// derived from that name instead of its `TypeId`, so every binary that links
/// the type reaches the same id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ComponentId {
    /// Component backed by a concrete Rust type, identified per binary.
    Native(TypeId),
    /// Component described at runtime by an external language manifest.
    Descriptor(u128),
    /// Component backed by a concrete Rust type that may be linked into more
    /// than one binary, identified by the stable name it declares.
    ///
    /// Storage is native, exactly as for [`Self::Native`] - only the identity
    /// differs - so [`Self::is_native_storage`] holds for both.
    Shared(u128),
}

/// Hashes a name to one 64-bit half of a stable component identity.
///
/// This is FNV-1a, so the result depends only on the bytes of the name: there
/// is no seed, no runtime state and no dependence on the compiler version or on
/// the process. Two separately compiled binaries - and the managed runtime,
/// which derives the same value from the same name - can therefore agree on an
/// identity without exchanging anything.
pub const fn component_name_hash(name: &str, offset_basis: u64) -> u64 {
    let bytes = name.as_bytes();
    let mut hash = offset_basis;
    let mut index = 0;
    while index < bytes.len() {
        hash ^= bytes[index] as u64;
        hash = hash.wrapping_mul(0x100000001b3);
        index += 1;
    }
    hash
}

/// Derives the stable 128-bit identity of a component from its declared name.
///
/// Two FNV-1a passes with different offsets, concatenated, so the 128-bit
/// space is actually used rather than a 64-bit hash being zero-padded into it.
pub const fn shared_component_identity(name: &str) -> u128 {
    let low = component_name_hash(name, 0xcbf29ce484222325);
    let high = component_name_hash(name, 0x84222325cbf29ce4);
    ((high as u128) << 64) | low as u128
}

/// Hashes a 128-bit identity as 64 bits.
///
/// [`TypeId`] does the same: it holds 128 bits and hashes only half. Hashing
/// part of a value stays correct because a hash map resolves a collision with
/// `Eq`, which still compares every bit. Hashing the second 8-byte block buys
/// no correctness and costs a compression round.
///
/// The variant is mixed in as a salt instead of being hashed as a value of its
/// own, which keeps the variants apart for free.
impl std::hash::Hash for ComponentId {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            // `TypeId`'s own impl already does the 128-to-64 narrowing.
            Self::Native(type_id) => type_id.hash(state),
            Self::Descriptor(identity) => (*identity as u64 ^ DESCRIPTOR_HASH_SALT).hash(state),
            Self::Shared(identity) => (*identity as u64 ^ SHARED_HASH_SALT).hash(state),
        }
    }
}

impl ComponentId {
    /// Returns the [`ComponentId`] for the concrete Rust type `T`.
    ///
    /// For an ordinary component this is its [`TypeId`], which differs between
    /// binaries. For one that declares [`Component::shared_name`] it is the
    /// stable identity derived from that name, which does not differ. Every
    /// binary that links the type therefore produces the same id here, and the
    /// engine gives them one bit and one column between them.
    ///
    /// The bound is [`Component`] rather than `'static` so that this
    /// distinction cannot be bypassed: there is no second entry point that
    /// could forget to consult the declaration.
    ///
    /// Either path costs the same as `TypeId::of`. For a derived component,
    /// `shared_identity` is a compile-time constant, so the shared branch is a
    /// load too and neither branch hashes the name. A type that writes
    /// `shared_name` by hand without overriding `shared_identity` hashes its
    /// name here on every call.
    #[inline]
    pub fn of<T: Component>() -> Self {
        match T::shared_identity() {
            Some(identity) => Self::Shared(identity),
            None => Self::Native(TypeId::of::<T>()),
        }
    }

    /// Builds a descriptor component ID from the stable 128-bit identity
    /// supplied by an external runtime manifest.
    pub const fn descriptor(stable_id: u128) -> Self {
        Self::Descriptor(stable_id)
    }

    /// Wraps an existing Rust [`TypeId`] in a native component ID.
    pub const fn native(type_id: TypeId) -> Self {
        Self::Native(type_id)
    }

    /// Returns the wrapped [`TypeId`] when this component is identified by one.
    ///
    /// The answer is `None` for a descriptor component, which has no Rust type
    /// at all. It is also `None` for a shared component: that one has a Rust
    /// type in every binary that links it, but no single `TypeId` names it. A
    /// caller asking "are these rows native storage?" wants
    /// [`Self::is_native_storage`] instead.
    pub const fn native_type_id(self) -> Option<TypeId> {
        match self {
            Self::Native(type_id) => Some(type_id),
            Self::Descriptor(_) | Self::Shared(_) => None,
        }
    }

    /// Whether this component's rows live in a native column rather than in a
    /// byte-oriented descriptor one.
    ///
    /// True for [`Self::Native`] and [`Self::Shared`] alike: shared identity
    /// changes how a component is *named*, never how it is stored.
    pub const fn is_native_storage(self) -> bool {
        matches!(self, Self::Native(_) | Self::Shared(_))
    }

    /// The stable identity behind a shared component id, if this is one.
    pub const fn shared_identity(self) -> Option<u128> {
        match self {
            Self::Shared(identity) => Some(identity),
            Self::Native(_) | Self::Descriptor(_) => None,
        }
    }
}

// =============================================================================
// ComponentRegistration
// =============================================================================

/// What one component registration declared about its layout.
///
/// Stored per [`ComponentId`] so a later registration of the same component can
/// be compared with the first. A shared component's record must match, because
/// two binaries reach one column through it and nothing else proves that they
/// agree about what a row contains. An ordinary component's record is simply
/// refreshed; a redefined type is handled by the persistence migration.
///
/// This is the layout the type declared, not the layout of the stored column.
/// [`ColumnLayout`](crate::archetype::ColumnLayout) describes the column, and
/// its `schema_hash` is a plain `u64` rather than the `Option` below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComponentRegistration {
    /// Byte size of one component value.
    pub size: usize,
    /// Byte alignment of one component value.
    pub align: usize,
    /// Structural hash over the declared field layout, or `None` when the
    /// component was registered without one (hand-registered or unit types).
    pub schema_hash: Option<u64>,
}

impl ComponentRegistration {
    /// Builds the layout record for `T` from its declared field descriptors.
    ///
    /// An empty `fields` slice records no schema hash rather than the hash of
    /// nothing, so a registration that simply carries no field metadata is not
    /// mistaken for one describing an empty struct.
    pub fn of<T: Component>(fields: &[ComponentFieldDescriptor]) -> Self {
        Self {
            size: std::mem::size_of::<T>(),
            align: std::mem::align_of::<T>(),
            schema_hash: (!fields.is_empty()).then(|| component_schema_hash(fields)),
        }
    }

    /// Whether `other` describes the same memory shape as this layout.
    ///
    /// Size and alignment must always agree. The structural hash is compared
    /// only when both records have one: a layout-less registration carries no
    /// evidence either way, and treating its absence as a mismatch would
    /// reject a hand-registered component that is in fact identical.
    ///
    /// Size and alignment alone do not prove that two layouts match: `{f32,
    /// f32}` and `{u32, u32}` agree on both and would read each other's rows
    /// wrongly, with no error. That is why the schema hash exists, and why a
    /// shared component should always be registered with its field
    /// descriptors.
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        if self.size != other.size || self.align != other.align {
            return false;
        }
        match (self.schema_hash, other.schema_hash) {
            (Some(own_schema_hash), Some(other_schema_hash)) => {
                own_schema_hash == other_schema_hash
            }
            _ => true,
        }
    }
}

/// Structural hash over a component's declared field layout.
///
/// The hash is FNV-1a over the field descriptors rather than a `DefaultHasher`,
/// because the value is compared between separately compiled binaries. The
/// standard hasher's algorithm is explicitly unspecified and may change between
/// Rust releases, while this is a fixed function of the bytes.
///
/// Every field's name, type tag, offset, size, alignment and element count is
/// included, so a field that is reordered, retyped, padded differently or
/// resized changes the result even when the struct's total size does not.
pub fn component_schema_hash(fields: &[ComponentFieldDescriptor]) -> u64 {
    // Start from the FNV-1a offset basis. `component_name_hash` continues from
    // this value, so a field's text and its numbers end up in one running hash.
    let mut hash = 0xcbf29ce484222325u64;
    for field in fields {
        hash = component_name_hash(field.name, hash);
        hash = component_name_hash(field.type_tag, hash);
        for field_number in [field.offset, field.size, field.align, field.element_count] {
            for byte in (field_number as u64).to_le_bytes() {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    hash
}

// =============================================================================
// ComponentMask
// =============================================================================

/// A set of component types, one bit per type.
///
/// One `u128` caps the ECS at 128 component types. That is the price of O(1)
/// archetype matching with a bitwise AND, and of masks that stay on the stack
/// and are `Copy`. Registration reports the ceiling as
/// [`WorldError::ComponentTypeLimitExceeded`] instead of panicking; a project
/// stays under it by keeping related fields in one component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ComponentMask(u128);

impl ComponentMask {
    /// Constructs a mask with no bits set.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Sets a bit in the mask.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `bit_index >= 128`.
    pub fn set(&mut self, bit_index: u8) {
        debug_assert!(
            bit_index < 128,
            "ComponentMask bit index {bit_index} out of range (max 127)"
        );
        self.0 |= 1u128 << bit_index;
    }

    /// Returns whether a bit is set (O(1) component type check).
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `bit_index >= 128`.
    #[inline]
    pub fn has_bit(&self, bit_index: u8) -> bool {
        debug_assert!(
            bit_index < 128,
            "ComponentMask bit index {bit_index} out of range (max 127)"
        );
        (self.0 & (1u128 << bit_index)) != 0
    }

    /// Returns whether every bit set in `other` is also set in this mask.
    ///
    /// The query hot path uses it to check whether an archetype has all the
    /// components a query asks for.
    #[inline]
    pub fn contains_all(&self, other: &ComponentMask) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Returns the bitwise AND of two masks: the bits set in both.
    #[inline]
    pub fn intersection(a: &ComponentMask, b: &ComponentMask) -> ComponentMask {
        ComponentMask(a.0 & b.0)
    }

    /// Returns `true` when no bit is set.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Returns `true` when any bit set in `other` is also set in this mask.
    #[inline]
    pub fn intersects(&self, other: &ComponentMask) -> bool {
        (self.0 & other.0) != 0
    }

    /// Returns the raw `u128` bitfield, which is used to build a unique
    /// [`ArchetypeId`](crate::archetype::ArchetypeId).
    #[inline]
    pub(crate) fn bits(self) -> u128 {
        self.0
    }
}

// =============================================================================
// ComponentRegistry
// =============================================================================

/// Maps component types to bit indices in the component mask.
///
/// It registers component types and keeps the [`ComponentId`] to bit index
/// mapping that mask operations need.
pub struct ComponentRegistry {
    /// Maps each registered [`ComponentId`] to its assigned bit index.
    bit_indices_by_id: HashMap<ComponentId, u8>,
    /// Registered name of each component type, by id. Diagnostics print it, and
    /// so do the name-keyed lookups persistence and the C# bindings make.
    names_by_id: HashMap<ComponentId, String>,
    /// Layout record of each registered component type, by id.
    layouts_by_id: HashMap<ComponentId, ComponentRegistration>,
    /// The Rust type name that claimed each shared identity, so a second claim
    /// by a *different* type can be told apart from the same type compiled
    /// twice.
    ///
    /// Owned rather than the `&'static str` that `type_name` returns: that
    /// pointer lives in the declaring artifact's read-only data and dangles once
    /// a module DLL is unloaded, while the registry outlives every module.
    shared_declaring_types: HashMap<ComponentId, String>,
    /// Shared components already reported as registered without field
    /// descriptors, so the warning fires once per id rather than once per
    /// registration - a reload registers the same types again every time.
    shared_without_layout_reported: std::collections::HashSet<ComponentId>,
    /// Next bit index to assign to a newly registered component.
    next_bit_index: u8,
    /// Bit indices reclaimed by [`Self::remove`], reused before `next_bit_index`
    /// advances.
    ///
    /// Without this pool, a descriptor manifest that retires and introduces
    /// types across reloads would keep increasing `next_bit_index` until the
    /// 128-type limit refused further registrations, even though only a handful
    /// of components are alive at a time.
    free_bits: Vec<u8>,
}

/// What a registration call found and did.
///
/// Registration is idempotent, so "registered" and "was already registered" are
/// both successes. They are still different facts, and collapsing them is how a
/// stale recorded layout goes unnoticed after a reload replaces a component's
/// definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "registration reports whether the type was already present; use `register_bit` if only the bit index is wanted"]
pub enum Registration {
    /// The type was not present and has been assigned this bit.
    Created(u8),
    /// The type was already registered under this bit; nothing changed.
    AlreadyPresent(u8),
}

impl Registration {
    /// The bit index, whichever case this is.
    #[must_use]
    pub fn bit(self) -> u8 {
        match self {
            Self::Created(bit) | Self::AlreadyPresent(bit) => bit,
        }
    }

    /// Whether this call is what created the registration.
    #[must_use]
    pub fn is_new(self) -> bool {
        matches!(self, Self::Created(_))
    }
}

impl ComponentRegistry {
    /// Creates an empty registry with no components registered.
    pub fn new() -> Self {
        Self {
            bit_indices_by_id: HashMap::new(),
            names_by_id: HashMap::new(),
            layouts_by_id: HashMap::new(),
            shared_declaring_types: HashMap::new(),
            shared_without_layout_reported: std::collections::HashSet::new(),
            next_bit_index: 0,
            free_bits: Vec::new(),
        }
    }
}

impl ComponentRegistry {
    /// Registers a component type and gives it a bit index.
    ///
    /// Returns [`Registration::Created`] with the new bit, or
    /// [`Registration::AlreadyPresent`] with the existing one. It is idempotent
    /// by design: a reload re-runs `init`, which registers every type again, and
    /// the two cases stay distinguishable for a caller that cares.
    /// [`Self::register_bit`] throws the distinction away for callers that do
    /// not.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::ComponentTypeLimitExceeded`] when all 128 bits are
    /// taken and [`Self::remove`](ComponentRegistry::remove) has not reclaimed
    /// one. Component types come from user data, so reaching the limit is a
    /// configuration outcome rather than a programming error - it is reported
    /// instead of panicking.
    pub fn register<T: Component>(&mut self) -> Result<Registration, WorldError> {
        self.register_with_layout::<T>(&[])
    }

    /// Registers a component type together with the field layout it declares.
    ///
    /// The layout is recorded so a later registration of the same component - a
    /// hot-reload generation, or a second binary that linked the same shared
    /// type - can be checked against it.
    ///
    /// # Errors
    ///
    /// In addition to [`Self::register`]'s error, returns
    /// [`WorldError::SharedComponentLayoutMismatch`] when a component with a
    /// declared shared identity is registered a second time with a different
    /// memory shape. That check is what makes reaching one column from two
    /// binaries sound, so it is an error rather than a debug assertion.
    pub fn register_with_layout<T: Component>(
        &mut self,
        fields: &[ComponentFieldDescriptor],
    ) -> Result<Registration, WorldError> {
        self.register_with_layout_superseding::<T>(fields, false)
    }

    /// [`Self::register_with_layout`] for a registration that replaces the
    /// previous generation of the same subject.
    ///
    /// With `supersedes` set, a shared component whose layout differs from the
    /// recorded one takes the new layout instead of being refused. The host has
    /// announced that this registration is a reload of a subject that registered
    /// the component before, and the reload moves the component's rows to the
    /// new layout. Every other binary still built against the old layout is now
    /// stale: its queries are refused by the declared schema check, and the host
    /// suspends its systems until it is rebuilt.
    ///
    /// # Errors
    ///
    /// As [`Self::register_with_layout`], except that a layout disagreement is
    /// not an error when `supersedes` is set.
    pub fn register_with_layout_superseding<T: Component>(
        &mut self,
        fields: &[ComponentFieldDescriptor],
        supersedes: bool,
    ) -> Result<Registration, WorldError> {
        // Step 1: Return the existing bit index when the type is already
        // registered, after checking that both registrations describe the same
        // memory shape.
        let component_id = ComponentId::of::<T>();
        let layout = ComponentRegistration::of::<T>(fields);
        if let Some(&bit) = self.bit_indices_by_id.get(&component_id) {
            // A shared name is a process-wide identity, so two types that hold
            // it are one component: one bit, one column, and every write
            // through either type reaches the other's rows. When their layouts
            // also agree, nothing downstream can notice; the reads succeed and
            // return another component's data.
            //
            // The case this check must not reject is one type compiled into two
            // binaries, which is the whole point of shared identity. Those
            // copies agree on the type's own name (`Spline` is `Spline` in
            // whichever artifact compiled it), while two different components
            // do not. Only the final path segment is compared, because the
            // module path differs between an in-process stand-in and the real
            // cross-binary case, while the type's name does not.
            if let Some(shared_name) = T::shared_name() {
                let incoming_type_name = Self::declaring_type_name::<T>();
                if let Some(existing_type_name) = self.shared_declaring_types.get(&component_id) {
                    if existing_type_name != incoming_type_name {
                        return Err(WorldError::SharedComponentNameConflict {
                            shared_name: shared_name.to_string(),
                            existing_type: existing_type_name.clone(),
                            incoming_type: incoming_type_name.to_string(),
                        });
                    }
                }
            }
            // A shared component registered without field descriptors cannot be
            // checked structurally: `is_compatible_with` compares only size and
            // alignment, which agree for `{f32,f32}` and `{u32,u32}` alike. The
            // fallback is deliberate - a hand-registered component carries no
            // evidence either way - but it must not happen silently, because the
            // point of shared identity is that two binaries agree about the
            // contents of one row. The warning is logged once per component id.
            if let Some(shared_name) = T::shared_name() {
                if layout.schema_hash.is_none()
                    && self.shared_without_layout_reported.insert(component_id)
                {
                    warn!(
                        target: telemetry_target::ECS,
                        shared_name,
                        type_name = std::any::type_name::<T>(),
                        "shared component registered without field descriptors; cross-binary agreement falls back to size and alignment, which cannot tell two same-shaped layouts apart"
                    );
                }
            }
            let recorded_layout = self.layouts_by_id.get(&component_id).copied();
            match (recorded_layout, T::shared_name()) {
                // A shared component reaching this branch is the normal
                // cross-binary case: a second binary registers a type the first
                // one already owns. Both will read and write the same rows
                // through their own `T`, so a layout disagreement would make
                // them read rows at the wrong offsets; it stops the registration
                // instead. A reload of a subject that registered this component
                // before is the one case where the new layout wins. The host
                // moves the rows after `init`, and binaries still built against
                // the old layout are refused by the declared schema check.
                (Some(recorded), Some(shared_name))
                    if supersedes && !recorded.is_compatible_with(&layout) =>
                {
                    warn!(
                        target: telemetry_target::ECS,
                        shared_name,
                        type_name = std::any::type_name::<T>(),
                        previous_size = recorded.size,
                        incoming_size = layout.size,
                        "shared component re-laid out by a reloading generation; binaries built against the previous layout are stale until they are rebuilt"
                    );
                }
                (Some(recorded), Some(shared_name)) if !recorded.is_compatible_with(&layout) => {
                    return Err(WorldError::SharedComponentLayoutMismatch {
                        shared_name: shared_name.to_string(),
                        type_name: std::any::type_name::<T>().to_string(),
                        existing_size: recorded.size,
                        existing_align: recorded.align,
                        incoming_size: layout.size,
                        incoming_align: layout.align,
                    });
                }
                // An ordinary component keeps the identity of its `TypeId`, so
                // a layout change here means a reload replaced the definition
                // while the compiler happened to reuse the id. The persistence
                // migration handles that; the record is refreshed so it
                // describes the definition now in force.
                _ => {}
            }
            // A layout-less re-registration (`register::<T>()`, fields `&[]`)
            // records no schema hash of its own, so a recorded hash is carried
            // forward: the record may gain a hash, but it must never lose one.
            // Otherwise the shared-layout check above would accept any layout
            // forever, and two binaries that disagree about a component's
            // fields would register without a warning.
            let mut merged_layout = layout;
            if merged_layout.schema_hash.is_none() {
                if let Some(recorded) = recorded_layout {
                    merged_layout.schema_hash = recorded.schema_hash;
                }
            }
            self.layouts_by_id.insert(component_id, merged_layout);
            return Ok(Registration::AlreadyPresent(bit));
        }
        // Step 2: Assign the next bit - either one reclaimed by `remove`, or a
        // fresh one. When neither is available the 128-type ceiling is hit,
        // which is reported as a typed error rather than an assert.
        let Some(bit) = self.allocate_bit() else {
            return Err(WorldError::ComponentTypeLimitExceeded {
                type_name: std::any::type_name::<T>().to_string(),
                count: self.bit_indices_by_id.len() as u8,
            });
        };
        // Step 3: Record the type's metadata under the assigned bit.
        self.bit_indices_by_id.insert(component_id, bit);
        // A shared component is recorded under its declared name, not
        // `std::any::type_name`: the declared name is what both binaries agree
        // on, and it is what every name-keyed lookup - persistence, the C#
        // bindings, the editor - has to find it by.
        self.names_by_id
            .insert(component_id, Self::registered_name::<T>());
        self.layouts_by_id.insert(component_id, layout);
        if T::shared_name().is_some() {
            self.shared_declaring_types
                .insert(component_id, Self::declaring_type_name::<T>().to_string());
        }
        Ok(Registration::Created(bit))
    }

    /// Registers a component type and returns its bit, ignoring whether it was
    /// already present.
    ///
    /// The common case: callers that only need the bit index.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::ComponentTypeLimitExceeded`] when the 128-type
    /// limit is reached, as [`Self::register`] does.
    pub fn register_bit<T: Component>(&mut self) -> Result<u8, WorldError> {
        self.register::<T>().map(Registration::bit)
    }

    /// [`Self::register_with_layout`] for callers that only need the bit.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::register_with_layout`].
    pub fn register_bit_with_layout<T: Component>(
        &mut self,
        fields: &[ComponentFieldDescriptor],
    ) -> Result<u8, WorldError> {
        self.register_with_layout::<T>(fields)
            .map(Registration::bit)
    }

    /// The Rust type's own name, without its module path.
    ///
    /// `pill_spline::Spline` and `tests::module_copy::Spline` both give
    /// `Spline`. The module path is dropped on purpose: it is what differs
    /// between an in-process stand-in for the cross-binary case and the real
    /// thing, while the type's name is what two copies of one type always
    /// share.
    pub fn declaring_type_name<T: ?Sized>() -> &'static str {
        let path = std::any::type_name::<T>();
        path.rsplit("::").next().unwrap_or(path)
    }

    /// The name a component type is registered under.
    ///
    /// Its declared shared name when it has one, otherwise its
    /// [`stable_type_name`](crate::type_names::stable_type_name); the engine's
    /// own types read `pill_engine::...`. A shared component has to be findable
    /// by the name both binaries wrote down, not by one binary's rendering of
    /// its own Rust path.
    pub fn registered_name<T: Component>() -> String {
        T::shared_name()
            .map(str::to_string)
            .unwrap_or_else(|| crate::type_names::stable_type_name::<T>().to_string())
    }

    /// Registers a component whose concrete type is defined outside Rust.
    ///
    /// Records only what registration itself owns: the bit index and the name.
    /// A descriptor component's storage shape belongs to the factory that
    /// builds its columns, and that factory is what readers consult. A second
    /// copy here could only go stale.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::ComponentTypeLimitExceeded`] when the 128-type
    /// limit is reached and no bit has been reclaimed by [`Self::remove`].
    pub fn register_descriptor(
        &mut self,
        stable_id: u128,
        name: impl Into<String>,
    ) -> Result<u8, WorldError> {
        // Step 1: Return the existing bit index when the stable ID is already
        // registered.
        let component_id = ComponentId::descriptor(stable_id);
        if let Some(&bit) = self.bit_indices_by_id.get(&component_id) {
            return Ok(bit);
        }

        // Step 2: Assign the next bit - reclaimed or fresh - reporting the
        // ceiling as a typed error instead of panicking.
        let name = name.into();
        let Some(bit) = self.allocate_bit() else {
            return Err(WorldError::ComponentTypeLimitExceeded {
                type_name: name,
                count: self.bit_indices_by_id.len() as u8,
            });
        };

        // Step 3: Record the descriptor component's identity.
        self.bit_indices_by_id.insert(component_id, bit);
        self.names_by_id.insert(component_id, name);
        Ok(bit)
    }

    /// Reserves one bit index for a newly registered type, preferring a bit
    /// reclaimed by [`Self::remove`] over a fresh one.
    ///
    /// Returns `None` once both the reclaimed pool and the fresh range are
    /// exhausted - that is, at the 128-type ceiling.
    fn allocate_bit(&mut self) -> Option<u8> {
        if let Some(bit) = self.free_bits.pop() {
            return Some(bit);
        }
        if self.next_bit_index < 128 {
            let bit = self.next_bit_index;
            self.next_bit_index += 1;
            return Some(bit);
        }
        None
    }

    /// Unregisters a component type and returns its bit to the reclaim pool.
    ///
    /// The caller must have removed every row of the type first. An archetype's
    /// mask is also its id, so a bit that is handed out again while an archetype
    /// still maps it would let the next component of that bit reach the stale
    /// archetype and read its columns.
    ///
    /// A later registration takes a bit from the pool first, so the freed bit
    /// is normally handed out again. The pool is what keeps a descriptor
    /// manifest that retires and introduces types across reloads from
    /// increasing `next_bit_index` toward the 128-type ceiling.
    pub fn remove(&mut self, component_id: &ComponentId) {
        if let Some(bit) = self.bit_indices_by_id.remove(component_id) {
            self.free_bits.push(bit);
        }
        self.names_by_id.remove(component_id);
        self.layouts_by_id.remove(component_id);
        self.shared_declaring_types.remove(component_id);
    }

    /// Number of component types that can still be registered before the
    /// 128-type ceiling is reached. Reclaimed bits and the unused fresh range
    /// both count.
    ///
    /// The host reports this so a project sees the ceiling coming before
    /// registrations start failing.
    pub fn available_slots(&self) -> usize {
        self.free_bits.len() + (128 - usize::from(self.next_bit_index))
    }

    /// Returns the bit index of a registered component, or `None`.
    pub fn get_bit(&self, component_id: &ComponentId) -> Option<u8> {
        self.bit_indices_by_id.get(component_id).copied()
    }

    /// Returns the registered name of a component, or `None`.
    pub fn get_name(&self, component_id: &ComponentId) -> Option<&str> {
        self.names_by_id.get(component_id).map(|s| s.as_str())
    }

    /// Returns the layout a Rust type declared when it registered.
    ///
    /// This is the declaration, not the storage. The persistence migration
    /// compares a new generation's declared shape with the one its predecessor
    /// registered. A descriptor component declares nothing in Rust, so it has
    /// no entry here. Storage shapes come from
    /// [`World::component_layout`](crate::World::component_layout), which asks
    /// the factory that builds the columns.
    pub fn get_layout(&self, component_id: &ComponentId) -> Option<ComponentRegistration> {
        self.layouts_by_id.get(component_id).copied()
    }

    /// Checks whether a component type has been registered.
    ///
    /// Returns `true` if `T` has been registered via [`register`](Self::register).
    pub fn is_registered<T: Component>(&self) -> bool {
        self.bit_indices_by_id.contains_key(&ComponentId::of::<T>())
    }

    /// Iterates over all registered components.
    ///
    /// Yields `(ComponentId, bit_index, type_name)` for each registered
    /// component type, in no particular order. Used to turn an id back into its
    /// name, to dump the registry, and to count registrations.
    pub fn registered_components(&self) -> impl Iterator<Item = (ComponentId, u8, &str)> {
        self.bit_indices_by_id.iter().map(|(id, &bit)| {
            (
                *id,
                bit,
                self.names_by_id.get(id).map(|s| s.as_str()).unwrap_or("?"),
            )
        })
    }

    /// Number of registered component types.
    #[inline]
    pub fn len(&self) -> usize {
        self.bit_indices_by_id.len()
    }

    /// Returns `true` if no component types are registered.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bit_indices_by_id.is_empty()
    }
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that `Tick` is exactly 4 bytes (a single u32).
    #[test]
    fn tick_size() {
        assert_eq!(std::mem::size_of::<Tick>(), 4);
        assert_eq!(std::mem::align_of::<Tick>(), 4);
    }

    /// Verifies that `ComponentTicks` is exactly 8 bytes (two u32 fields).
    #[test]
    fn component_ticks_size() {
        assert_eq!(std::mem::size_of::<ComponentTicks>(), 8);
        assert_eq!(std::mem::align_of::<ComponentTicks>(), 4);
    }

    /// `ComponentRegistry::remove` fully unregisters a type so a later
    /// re-registration works again; the entry is completely gone in between.
    #[test]
    fn remove_unregisters_and_reregistration_works() {
        #[derive(Clone, Debug)]
        struct ReRegisterTestComponent;
        impl Component for ReRegisterTestComponent {}

        let mut registry = ComponentRegistry::new();
        let component_id = ComponentId::of::<ReRegisterTestComponent>();

        let first_registration = registry.register::<ReRegisterTestComponent>().unwrap();
        assert!(
            first_registration.is_new(),
            "the first registration creates the bit"
        );
        let first_bit = first_registration.bit();
        assert!(registry.is_registered::<ReRegisterTestComponent>());
        assert_eq!(registry.get_bit(&component_id), Some(first_bit));

        // Registering the same type again reports that, rather than looking
        // identical to a fresh registration.
        let repeat_registration = registry.register::<ReRegisterTestComponent>().unwrap();
        assert_eq!(repeat_registration, Registration::AlreadyPresent(first_bit));
        assert!(!repeat_registration.is_new());

        registry.remove(&component_id);
        assert!(!registry.is_registered::<ReRegisterTestComponent>());
        assert_eq!(registry.get_bit(&component_id), None);
        assert_eq!(registry.get_name(&component_id), None);
        assert_eq!(registry.get_layout(&component_id), None);

        // Re-registering works and reuses the freed bit: `remove` returns the
        // bit to the reclaim pool, so a descriptor manifest that retires and
        // introduces types across reloads does not increase `next_bit_index`
        // toward the 128-type ceiling.
        let second_registration = registry.register::<ReRegisterTestComponent>().unwrap();
        assert!(
            second_registration.is_new(),
            "after removal it is a fresh registration"
        );
        let second_bit = second_registration.bit();
        assert!(registry.is_registered::<ReRegisterTestComponent>());
        assert_eq!(registry.get_bit(&component_id), Some(second_bit));
        assert_eq!(first_bit, second_bit, "the freed bit is reused");
    }

    /// Registering the 129th component type reports a typed error instead of
    /// panicking - the ceiling is a configuration outcome, not a programming
    /// error.
    #[test]
    fn the_129th_component_type_errors_instead_of_panicking() {
        // Each name is a distinct fake type, registered from descriptors so
        // the test does not need 129 real Rust types.
        let mut registry = ComponentRegistry::new();
        for index in 0..128 {
            let registration =
                registry.register_descriptor(index as u128 + 1, format!("Project.FakeType{index}"));
            assert!(registration.is_ok(), "slot {index} must register");
        }

        // One more than the ceiling: the registry reports the limit, naming
        // the offending type and the current count.
        let error = registry
            .register_descriptor(u128::MAX, "Project.OneTooMany")
            .unwrap_err();
        assert_eq!(
            error,
            WorldError::ComponentTypeLimitExceeded {
                type_name: "Project.OneTooMany".to_string(),
                count: 128,
            }
        );

        // A freed bit reopens a slot, so the same registry accepts a new type
        // after `remove` - the ceiling is not a permanent dead end. The first
        // registered type (stable id 1) holds bit 0, so freeing it reopens bit 0.
        registry.remove(&ComponentId::descriptor(1));
        let reused_bit = registry
            .register_descriptor(u128::MAX - 1, "Project.AfterFree")
            .unwrap();
        assert_eq!(reused_bit, 0, "the reclaimed bit is handed out again");
    }

    /// Reclaimed bits keep the registry's bit counter from growing toward the
    /// 128-type ceiling while a project keeps adding and removing types.
    #[test]
    fn removed_bits_are_reclaimed_and_reported_as_headroom() {
        let mut registry = ComponentRegistry::new();
        for index in 0..8 {
            registry
                .register_descriptor(index as u128 + 10, format!("Project.C{index}"))
                .unwrap();
        }
        assert_eq!(registry.available_slots(), 120);

        // Remove half the types: their bits rejoin the pool.
        for index in 0..4 {
            registry.remove(&ComponentId::descriptor(index as u128 + 10));
        }
        assert_eq!(registry.available_slots(), 124);

        // New registrations reuse the reclaimed bits (LIFO: the most recently
        // freed first) rather than consuming fresh ones.
        let new_registration_bit = registry.register_descriptor(999, "Project.New").unwrap();
        assert_eq!(
            new_registration_bit, 3,
            "the most recently freed bit is reused first"
        );
        assert_eq!(registry.available_slots(), 123);
    }

    /// A re-registration without fields must not disable the shared-layout
    /// check. The recorded schema hash survives it, so a later declaration with
    /// the same size but a different field shape is still refused, unless the
    /// declaration supersedes the previous generation and gives it the new
    /// layout.
    #[test]
    fn schema_hash_survives_layout_less_reregistration() {
        // Two copies of one shared type, as two binaries compile them: the same
        // final type name (so the name check passes) and the same size. Only
        // the field descriptors they declare differ, and those are what the
        // schema hash covers.
        mod first_copy {
            use crate::component::Component;

            /// The first copy of the shared type, as one binary declares it.
            #[derive(Clone, Debug)]
            pub struct SharedProbe {
                // The declared shape is what the schema hash covers; no code
                // reads the field value.
                #[allow(dead_code)]
                pub value: u32,
            }
            impl Component for SharedProbe {
                fn shared_name() -> Option<&'static str> {
                    Some("audit::SharedProbe")
                }
            }
        }

        mod second_copy {
            use crate::component::Component;

            /// The second copy of the same type, as another binary declares it:
            /// the same name and the same fields.
            #[derive(Clone, Debug)]
            pub struct SharedProbe {
                // Only the declared shape reaches the schema hash; nothing reads
                // the field value.
                #[allow(dead_code)]
                pub value: u32,
            }
            impl Component for SharedProbe {
                fn shared_name() -> Option<&'static str> {
                    Some("audit::SharedProbe")
                }
            }
        }

        // The two copies are identical in Rust; what differs is the field name
        // in the descriptors each binary passes, which is one of the inputs to
        // the schema hash.
        static FIELDS_A: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
            name: "first",
            type_tag: "u32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }];
        static FIELDS_B: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
            name: "second",
            type_tag: "u32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }];

        let mut registry = ComponentRegistry::new();
        let _first_declaration = registry
            .register_with_layout::<first_copy::SharedProbe>(FIELDS_A)
            .expect("the first declaration is recorded");

        // This registration carries no fields. The recorded hash has to survive
        // it, or the later check would accept any layout.
        let _re_registration = registry
            .register::<first_copy::SharedProbe>()
            .expect("re-registration is idempotent");

        let error = registry
            .register_with_layout::<second_copy::SharedProbe>(FIELDS_B)
            .expect_err("the hash recorded by the first declaration must survive");
        assert!(matches!(
            error,
            WorldError::SharedComponentLayoutMismatch { .. }
        ));

        // A reload of the subject that registered the component may give it a
        // new layout. The new layout is recorded, so from then on the old one
        // is the mismatch.
        let relaid_registration = registry
            .register_with_layout_superseding::<second_copy::SharedProbe>(FIELDS_B, true)
            .expect("a superseding registration re-lays out the component");
        assert!(!relaid_registration.is_new(), "the component keeps its bit");
        assert_eq!(
            registry
                .get_layout(&ComponentId::of::<first_copy::SharedProbe>())
                .and_then(|layout| layout.schema_hash),
            Some(component_schema_hash(FIELDS_B)),
            "the new layout is the recorded one"
        );
        let stale_error = registry
            .register_with_layout::<first_copy::SharedProbe>(FIELDS_A)
            .expect_err("a binary built against the old layout is now the stale one");
        assert!(matches!(
            stale_error,
            WorldError::SharedComponentLayoutMismatch { .. }
        ));
    }
}
