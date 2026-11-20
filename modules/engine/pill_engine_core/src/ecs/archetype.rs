//! Archetype-based component storage with SoA layout.
//!
//! An archetype is a unique combination of component types. All entities with
//! the same set of components are stored together in one archetype, so
//! iterating them stays cache-friendly.
//!
//! # Responsibilities
//!
//! - Group entities by their exact component set into [`Archetype`] instances.
//! - Own the contiguous, type-erased component storage for each archetype,
//!   for native and layout-described components alike: every column is a
//!   `ComponentColumn`, held in the archetype's `ComponentColumns`.
//! - Track change-detection [`ComponentTicks`] for every component instance.
//!
//! # Design
//!
//! Components of one type are stored contiguously in memory: a structure of
//! arrays rather than an array of structures. Iterating every entity with
//! `Position + Velocity` therefore reads the entity array and one array per
//! component, in sequence. Reading all components of one entity instead costs
//! one lookup per component, and bulk iteration makes that rare.
//!
//! Entity removal uses swap-remove to keep the arrays dense. The removed row is
//! replaced by the last row of each component array, the arrays shrink by one
//! row, and the moved entity's location is updated. Removal stays O(1) and
//! leaves no gaps.
//!
//! # Invariants
//!
//! The raw-byte storage rests on four rules, each enforced at the boundary
//! that can check it:
//!
//! - **Element identity.** [`ColumnIdentity`] records how much the compiler
//!   can still check about a column: an exact `TypeId` (native), size and
//!   alignment plus the registry's field-layout comparison (shared), or no Rust
//!   type at all (descriptor). The typed accessors enforce the first two, and
//!   [`ComponentColumn::get`] panics on a mismatch. A descriptor column has no
//!   typed reading, because no Rust type names its rows.
//! - **Allocation alignment.** A column's buffer is allocated with the row's
//!   alignment, so every row is aligned for the type the column was built for.
//!   Registration refuses a shared component whose layout changed, unless the
//!   registering generation replaces the old one. Otherwise rows spawned into
//!   the old column would be read through the incoming type at the old stride.
//! - **Release through the ops table.** Everything that depends on what the
//!   bytes mean goes through [`ColumnOps`], a plain-data table that a reload
//!   refreshes before any row is touched. `drop_range` releases a row exactly
//!   once - on `swap_remove_discard` and on `Drop` - and never runs for a
//!   column whose `trivial_drop` is set. A [`Blittability`] value records the
//!   proof that a component's rows are plain data, so that claim is checked
//!   instead of assumed.
//! - **Row lifecycle.** `len` counts initialized rows. Growth leaves the new
//!   tail uninitialized until a write fills it, and `swap_remove` moves the
//!   tail row over the hole before lowering the length, so no row is read or
//!   dropped twice.

// Standard library
use std::alloc::{alloc, dealloc, handle_alloc_error, Layout};
use std::collections::HashMap;
use std::ptr::NonNull;

// External crates
use pill_core::telemetry::telemetry_target;

// Current crate
use crate::component::{Component, ComponentId, ComponentMask, ComponentTicks};
use crate::entity::Entity;
use crate::error::WorldError;

// =============================================================================
// StorageFactory
// =============================================================================

/// Strategy for creating component storage for a specific component type.
///
/// A factory is registered per [`ComponentId`] and used when an [`Archetype`]
/// is created, so the archetype can allocate storage without knowing the
/// concrete component types.
pub enum StorageFactory {
    /// Creates a type-erased native storage column inside the archetype's
    /// [`ComponentColumns`].
    ///
    /// It carries only data - the type id, the layout and the per-type
    /// function table - never a closure. The column is a concrete
    /// [`ComponentColumn`] with no trait-object vtable, so the column stays
    /// valid when a module unloads. The engine refreshes the function table on
    /// every reload.
    Native(NativeColumnInfo),
    /// Carries the runtime layout of a component owned by another language.
    Descriptor(ColumnLayout),
}

// =============================================================================
// Blittability
// =============================================================================

/// Proof that a descriptor component's rows are plain old data.
///
/// The descriptor-component design rests on one premise: a row can be moved by
/// copying its bytes, owns nothing, and can be freed without running
/// destructors. `ComponentColumn` copies rows with `ptr::copy`, frees buffers
/// without touching the elements, and is `Send + Sync` - and each of those is
/// only sound while the premise holds.
///
/// A value of this type is created by a check and stored next to the layout it
/// protects, so a column cannot be built without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blittability {
    /// Private unit field: the only ways in are the constructors below.
    _private: (),
}

impl Blittability {
    /// The proof a manifest-driven registration carries.
    ///
    /// `pill_csharp_bridge::components` asks for one only for an entry that
    /// `parse_and_validate_manifest` accepted. That check rejected every field
    /// type that is not a blittable value type, so the entry's fields are known
    /// to be plain data. The name records where the check ran.
    pub fn from_manifest_fields() -> Self {
        Self { _private: () }
    }

    /// The proof for shapes written out in this crate's tests.
    ///
    /// Test registrations name value-type fields literally. Code outside the
    /// crate gets the proof through [`Self::from_manifest_fields`].
    #[cfg(test)]
    pub(crate) fn engine_verified() -> Self {
        Self { _private: () }
    }

    /// Claims the proof without a check.
    ///
    /// # Safety
    ///
    /// Every field of the component must be a blittable value type: no
    /// pointers, references or other owners. A caller that gets this wrong
    /// hands `ComponentColumn` ownership it will never release.
    pub unsafe fn assume() -> Self {
        Self { _private: () }
    }
}

// =============================================================================
// NativeColumnInfo
// =============================================================================

/// Everything an archetype needs to build a column for a Rust component type,
/// as plain data.
///
/// It is data, not a closure. A factory that captured one generation's code
/// would keep that image's vtable alive after the image was unmapped.
#[derive(Debug, Clone, Copy)]
pub struct NativeColumnInfo {
    /// Runtime identity of the element type.
    pub type_id: std::any::TypeId,
    /// Size of one row in bytes.
    pub size: usize,
    /// Alignment of one row in bytes.
    pub align: usize,
    /// Whether the element is identified by layout instead of by `TypeId`,
    /// for a type compiled into more than one binary.
    pub shared: bool,
    /// Per-type behaviour, regenerated by whichever image registered last.
    pub ops: ColumnOps,
}

impl NativeColumnInfo {
    /// Describes the column for a concrete Rust element type.
    pub fn of<T: 'static>(shared: bool) -> Self {
        Self {
            type_id: std::any::TypeId::of::<T>(),
            size: std::mem::size_of::<T>(),
            align: std::mem::align_of::<T>(),
            shared,
            ops: ColumnOps::of::<T>(),
        }
    }
}

// =============================================================================
// ColumnIdentity
// =============================================================================

/// What a column requires of a `T` before it will hand out a typed reference.
///
/// The three cases are the three ways this engine knows a component type,
/// ordered by how much the compiler can still check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnIdentity {
    /// An ordinary Rust component: only the exact `TypeId` may read the rows.
    Native(std::any::TypeId),
    /// A Rust component compiled into more than one binary. Each binary has a
    /// different `TypeId` for the same type, so the layout is the only check
    /// left.
    ///
    /// The registry compares the declared field layout through a schema hash,
    /// **when both sides declare one**. Without field descriptors there is no
    /// such evidence, so the comparison falls back to size and alignment. That
    /// fallback cannot tell `{f32, f32}` from `{u32, u32}`. It is deliberate,
    /// because a hand-registered component could not be registered otherwise,
    /// and it is warned about once per component id. It is a convention rather
    /// than an invariant, so a shared component should always declare its
    /// fields.
    Shared,
    /// No Rust type names these rows. They are reached as bytes.
    ///
    /// Asking for a typed accessor on such a column is a bug in the calling
    /// code, not a runtime condition: a project that uses only Rust components
    /// never has a `T` to ask with.
    Descriptor,
}

// =============================================================================
// ColumnOps
// =============================================================================

/// Per-type behaviour for one column, as plain data.
///
/// A column holds raw bytes plus a layout. Releasing a row is the only
/// operation that has to know what those bytes mean, and it runs through this
/// table.
///
/// [`ColumnOps::of`] builds the table from `T`'s own code, so dropping a row
/// runs the element type's destructor. [`ColumnOps::blittable`] needs no
/// per-type code: a validated descriptor component is plain data, so dropping
/// a row does nothing and moving one is a `memcpy`.
///
/// The table is data, never a trait object. A column outlives the DLL that
/// filled it, so it can be pointed at the new generation's table instead of
/// dangling into an unmapped image.
#[derive(Clone, Copy)]
pub struct ColumnOps {
    /// Drops `row_count` initialized rows starting at `rows_ptr`, or does
    /// nothing for plain data.
    ///
    /// # Safety
    ///
    /// `rows_ptr` must address `row_count` live, correctly aligned rows of this
    /// column's element type.
    pub drop_range: unsafe fn(*mut u8, usize),
    /// Whether [`Self::drop_range`] does nothing.
    ///
    /// Read instead of called on the hot paths: a plain-data column skips the
    /// call entirely, rather than paying for an indirect jump to an empty
    /// function.
    pub trivial_drop: bool,
}

impl ColumnOps {
    /// The generated table for a concrete Rust element type.
    pub fn of<T: 'static>() -> Self {
        Self {
            drop_range: drop_range_of::<T>,
            trivial_drop: !std::mem::needs_drop::<T>(),
        }
    }

    /// The generic table for plain-data rows described only by a descriptor.
    ///
    /// It requires the same [`Blittability`] value as the column, so a table
    /// that says there is nothing to drop cannot be built without the check
    /// that made that true.
    pub fn blittable(_witness: Blittability) -> Self {
        Self {
            drop_range: drop_range_of_nothing,
            trivial_drop: true,
        }
    }
}

impl std::fmt::Debug for ColumnOps {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ColumnOps")
            .field("trivial_drop", &self.trivial_drop)
            .finish()
    }
}

// =============================================================================
// ColumnLayout
// =============================================================================

/// Runtime layout for a component whose concrete type belongs to another
/// language.
///
/// It describes the memory footprint of one opaque column, so its rows can be
/// copied in and out as raw bytes without knowing the concrete type.
///
/// A schema hash is always present: a column only exists once something
/// described its rows well enough to store them, so holding one of these means
/// the hash is known, not optional.
#[derive(Debug, Clone)]
pub struct ColumnLayout {
    /// Size in bytes of a single component instance.
    pub size: usize,
    /// Alignment in bytes required by a single component instance.
    pub align: usize,
    /// Hash identifying the component's schema across language boundaries.
    pub schema_hash: u64,
    /// Proof that rows of this shape are plain old data.
    ///
    /// Carried so the column that stores the rows never has to re-derive the
    /// premise: `Send`, `Sync` and the destructor-free paths all rely on it.
    pub blittability: Blittability,
}

impl ColumnLayout {
    /// Builds a layout, checking that size and alignment can describe storage.
    ///
    /// The descriptor constructor: every layout carries a [`Blittability`], so
    /// no column exists without one.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorSizeZero`],
    /// [`WorldError::DescriptorAlignmentInvalid`] or
    /// [`WorldError::DescriptorLayoutInvalid`] when the size and alignment cannot
    /// describe an allocation.
    pub fn new(
        size: usize,
        align: usize,
        schema_hash: u64,
        blittability: Blittability,
    ) -> Result<Self, WorldError> {
        validate_component_layout(size, align)?;
        Ok(Self {
            size,
            align,
            schema_hash,
            blittability,
        })
    }

    /// The same constructor, for a layout that came from a Rust type instead
    /// of a manifest.
    ///
    /// # Errors
    ///
    /// As [`Self::new`], except that a zero size is accepted - see
    /// [`validate_native_component_layout`].
    pub fn new_native(
        size: usize,
        align: usize,
        schema_hash: u64,
        blittability: Blittability,
    ) -> Result<Self, WorldError> {
        validate_native_component_layout(size, align)?;
        Ok(Self {
            size,
            align,
            schema_hash,
            blittability,
        })
    }
}

// =============================================================================
// LayoutField
// =============================================================================

/// One top-level field of a layout, as a migration plan sees it.
///
/// Only the top level is described. A nested struct field moves as a single
/// block, so a plan never maps its inner members: the block is copied or
/// zero-filled as a whole rather than migrated field by field. A plan copies a
/// field only when both layouts declare its name and its type tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutField<'a> {
    /// Field name. Matching is by name, never by position.
    pub name: &'a str,
    /// Type tag from the engine's field vocabulary (`f32`, `u32`,
    /// `struct:<path>`, ...).
    ///
    /// Matching compares this as well as the name. A name alone cannot tell a
    /// field that moved from one that changed meaning: an `i32` and an `f32`
    /// are both four bytes, so copying between them reinterprets the bits as a
    /// different number instead of preserving the value.
    pub type_tag: &'a str,
    /// Byte offset of the field inside a row.
    pub offset: usize,
    /// Byte size of the field.
    pub size: usize,
}

// =============================================================================
// FieldSource
// =============================================================================

/// Where one field of a new layout takes its bytes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldSource {
    /// Copy from this byte offset in the old row.
    OldOffset(usize),
    /// Leave the field zeroed. The relayout zeroes every row before it applies
    /// the plan, so a field with no source is defined rather than stale.
    ZeroFill,
    /// Write a default the caller supplied - a manifest's declared value for an
    /// added or retyped field - over the zeroed destination.
    Default {
        /// The default value's bytes, native-endian, left-aligned.
        bytes: [u8; 8],
        /// How many of the leading bytes are part of the value.
        len: u8,
    },
}

// =============================================================================
// PlannedField
// =============================================================================

/// One instruction in a migration plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedField {
    /// Byte offset of the field in the new row.
    pub offset: usize,
    /// The field's width in the new row, in bytes. A [`FieldSource::Default`]
    /// writes only its own `len` bytes of that width.
    pub bytes: usize,
    /// Where those bytes come from.
    pub source: FieldSource,
}

// =============================================================================
// FieldPlan
// =============================================================================

/// A byte-level plan for moving rows from one layout of a component to another.
///
/// It is data, not code. The caller computes it from the two field lists it
/// already has: the managed manifest carries field names, offsets and sizes, so
/// the C# path builds one plan per changed component. The engine then applies
/// the plan to every stored row. Any byte the plan does not cover is left zero.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldPlan {
    /// The instructions, in the order they are applied.
    fields: Vec<PlannedField>,
    /// Names of fields the plan reset because their type changed.
    ///
    /// Kept so the caller can report them. A field that silently becomes a
    /// different number is the failure this list exists to prevent, and
    /// resetting one without saying so is a smaller version of the same
    /// problem.
    retyped: Vec<String>,
}

impl FieldPlan {
    /// An empty plan: every byte of every new row is zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            fields: Vec::new(),
            retyped: Vec::new(),
        }
    }

    /// Fields this plan reset because their declared type changed.
    pub fn retyped_fields(&self) -> &[String] {
        &self.retyped
    }

    /// Appends one instruction.
    pub fn push(&mut self, offset: usize, bytes: usize, source: FieldSource) {
        self.fields.push(PlannedField {
            offset,
            bytes,
            source,
        });
    }

    /// Builds a plan from the top-level fields of the old and new layout.
    ///
    /// Fields are matched **by name**, so a reorder keeps every value with its
    /// own name. A field only the new layout has becomes a zero fill, and a
    /// field only the old layout had is left out, which is how a removed
    /// field's bytes leave the rows. A name repeated in the old list matches
    /// its first occurrence and the later ones are ignored, so a malformed
    /// input still produces a deterministic plan instead of one that depends
    /// on iteration order.
    ///
    /// A matched pair whose sizes disagree copies the smaller size and leaves
    /// the rest zeroed. A field that grew therefore keeps its bytes and gains a
    /// defined tail, instead of reading past the old row.
    #[must_use]
    pub fn between(old_fields: &[LayoutField<'_>], new_fields: &[LayoutField<'_>]) -> Self {
        let mut plan = Self::new();
        for new_field in new_fields {
            match old_fields
                .iter()
                .find(|old_field| old_field.name == new_field.name)
            {
                // A name match whose type also matches carries its bytes over.
                Some(old_field) if old_field.type_tag == new_field.type_tag => plan.push(
                    new_field.offset,
                    old_field.size.min(new_field.size),
                    FieldSource::OldOffset(old_field.offset),
                ),
                // A name match whose type changed is reset, not copied.
                //
                // Copying would reinterpret the bits: an `i32` of 5 read as an
                // `f32` is 7e-45, which is a different number, not the same
                // value migrated. Resetting does what the native path already
                // does for the same edit: serde fails to read the old shape and
                // falls back to the type's default. It also keeps an ordinary
                // retype from costing a host restart. The field is recorded so
                // the caller can report it.
                Some(old_field) => {
                    plan.retyped.push(new_field.name.to_owned());
                    let _ = old_field;
                    plan.push(new_field.offset, new_field.size, FieldSource::ZeroFill);
                }
                None => plan.push(new_field.offset, new_field.size, FieldSource::ZeroFill),
            }
        }
        plan
    }

    /// The instructions, in application order.
    #[must_use]
    pub fn fields(&self) -> &[PlannedField] {
        &self.fields
    }

    /// Gives one planned field a default value to write instead of zero.
    ///
    /// This is for fields that an added or reshaped layout leaves empty. A
    /// field the plan already copies keeps its carried value and this call
    /// does nothing, so a declared default can never overwrite a value that
    /// survived the reload.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorRowInvalid`] when no planned field
    /// starts at `offset`, when `bytes` is longer than a field can carry
    /// (eight bytes - the widest primitive), or when it is longer than the
    /// planned field itself.
    pub fn set_default(&mut self, offset: usize, bytes: &[u8]) -> Result<(), WorldError> {
        if bytes.len() > 8 {
            return Err(WorldError::DescriptorRowInvalid);
        }
        let Some(field) = self.fields.iter_mut().find(|field| field.offset == offset) else {
            return Err(WorldError::DescriptorRowInvalid);
        };
        if matches!(field.source, FieldSource::OldOffset(_)) {
            return Ok(());
        }
        if bytes.len() > field.bytes {
            return Err(WorldError::DescriptorRowInvalid);
        }
        let mut padded = [0_u8; 8];
        padded[..bytes.len()].copy_from_slice(bytes);
        field.source = FieldSource::Default {
            bytes: padded,
            len: bytes.len() as u8,
        };
        Ok(())
    }

    /// Whether the plan carries no instructions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Writes one row's bytes according to this plan.
    ///
    /// `source` is the old row, `destination` the new one. A caller that needs
    /// the bytes no instruction covers to be defined zeroes the destination
    /// first, because `ZeroFill` writes nothing of its own.
    ///
    /// Every case is handled here, so a default set with
    /// [`Self::set_default`] is written the same way whichever path applies the
    /// plan. Row sizes are not re-checked: callers run [`Self::validate`] once
    /// before applying the plan to every stored row, so the check costs one
    /// pass rather than one per row.
    pub fn apply(&self, source: &[u8], destination: &mut [u8]) {
        for field in &self.fields {
            match field.source {
                FieldSource::OldOffset(offset) => destination
                    [field.offset..field.offset + field.bytes]
                    .copy_from_slice(&source[offset..offset + field.bytes]),
                // Only the leading bytes of a default are part of the value;
                // the rest of the field keeps the destination's zero fill.
                FieldSource::Default { bytes, len } => {
                    let len = usize::from(len);
                    destination[field.offset..field.offset + len].copy_from_slice(&bytes[..len]);
                }
                FieldSource::ZeroFill => {}
            }
        }
    }

    /// Checks every instruction against the two row sizes.
    ///
    /// Callers run this before touching a row, so a plan that does not fit is
    /// refused while the storage is still unchanged. The error is
    /// [`WorldError::DescriptorRowInvalid`], because a plan holds layout-level
    /// data and has no component id to build a richer message from.
    pub fn validate(&self, old_size: usize, new_size: usize) -> Result<(), WorldError> {
        for field in &self.fields {
            if out_of_bounds(field.offset, field.bytes, new_size) {
                return Err(WorldError::DescriptorRowInvalid);
            }
            match field.source {
                FieldSource::OldOffset(offset) => {
                    if out_of_bounds(offset, field.bytes, old_size) {
                        return Err(WorldError::DescriptorRowInvalid);
                    }
                }
                // The value has to fit the planned field and the destination;
                // the remainder of the field keeps its zeroed fill.
                FieldSource::Default { len, .. } => {
                    if usize::from(len) > field.bytes
                        || out_of_bounds(field.offset, usize::from(len), new_size)
                    {
                        return Err(WorldError::DescriptorRowInvalid);
                    }
                }
                FieldSource::ZeroFill => {}
            }
        }
        Ok(())
    }
}

// =============================================================================
// ComponentColumns
// =============================================================================

/// The component columns of one archetype, keyed by [`ComponentId`].
///
/// The key is [`ComponentId`], not [`TypeId`](std::any::TypeId): a component
/// type linked into two binaries has two `TypeId`s, so the binary that did not
/// create the column could not find it. Every other structure - the registry's
/// bit, the archetype's `component_types`, the tick vectors, the storage
/// factories - already identifies the component by one `ComponentId`.
#[derive(Default)]
pub struct ComponentColumns {
    /// One contiguous column per component in the archetype.
    columns: HashMap<ComponentId, ComponentColumn>,
}

impl ComponentColumns {
    /// Creates an empty set of columns.
    pub fn new() -> Self {
        Self {
            columns: HashMap::new(),
        }
    }

    /// Creates an empty set of columns sized for `capacity` component types.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            columns: HashMap::with_capacity(capacity),
        }
    }

    /// Returns the column for `component_id`, or `None` when the archetype
    /// does not store that component.
    #[inline]
    pub fn get(&self, component_id: ComponentId) -> Option<&ComponentColumn> {
        self.columns.get(&component_id)
    }

    /// Returns the column for `component_id` mutably.
    #[inline]
    pub fn get_mut(&mut self, component_id: ComponentId) -> Option<&mut ComponentColumn> {
        self.columns.get_mut(&component_id)
    }

    /// Returns the column storing `T`.
    ///
    /// # Panics
    ///
    /// Panics when the archetype does not store `T`, which means the caller
    /// reached a column the archetype's mask says is not there.
    #[inline]
    pub fn column_of<T: Component>(&self) -> &ComponentColumn {
        self.get(ComponentId::of::<T>())
            .unwrap_or_else(|| missing_column::<T>())
    }

    /// Returns the column storing `T`, mutably.
    ///
    /// # Panics
    ///
    /// Panics under the same condition as [`Self::column_of`].
    #[inline]
    pub fn column_of_mut<T: Component>(&mut self) -> &mut ComponentColumn {
        self.get_mut(ComponentId::of::<T>())
            .unwrap_or_else(|| missing_column::<T>())
    }

    /// Inserts a column for `component_id`.
    ///
    /// # Panics
    ///
    /// Panics when a column for that component already exists; an archetype
    /// builds each of its columns exactly once.
    pub fn insert(&mut self, component_id: ComponentId, column: ComponentColumn) {
        let replaced = self.columns.insert(component_id, column);
        assert!(
            replaced.is_none(),
            "component {component_id:?} already has a column in this archetype"
        );
    }

    /// Removes and returns the column for `component_id`.
    pub fn remove(&mut self, component_id: ComponentId) -> Option<ComponentColumn> {
        self.columns.remove(&component_id)
    }

    /// Whether a column exists for `component_id`.
    #[inline]
    pub fn contains(&self, component_id: ComponentId) -> bool {
        self.columns.contains_key(&component_id)
    }

    /// Number of columns stored.
    pub fn len(&self) -> usize {
        self.columns.len()
    }

    /// Whether no columns are stored.
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

// =============================================================================
// ComponentColumn
// =============================================================================

/// Aligned, densely packed, type-erased storage for one component's rows.
///
/// Owns a raw heap allocation whose element size and alignment come from a
/// [`ColumnLayout`]. Rows are written and read as raw bytes, which
/// lets components defined in other languages share storage with native ones.
pub struct ComponentColumn {
    /// Runtime layout of the stored element type.
    element_layout: ColumnLayout,
    /// Per-type behaviour, replaceable when a reload brings a newer table.
    ops: ColumnOps,
    /// What a typed accessor must prove before it may read these rows.
    identity: ColumnIdentity,
    /// Pointer to the heap allocation, or dangling before the first growth.
    row_buffer: NonNull<u8>,
    /// Number of initialized rows.
    len: usize,
    /// Number of rows the current allocation can hold.
    capacity: usize,
    /// Change-detection metadata, one entry per row at the same index.
    ///
    /// The column owns the ticks instead of keeping them beside it, so a push
    /// grows both and a removal shrinks both, and there is only one length to
    /// maintain. A tick is plain data with no code pointers, so it has none of
    /// the [`ColumnOps`] lifetime concerns. It is a separate allocation rather
    /// than interleaved with the rows, so iterating the component data keeps
    /// the same locality.
    ticks: Vec<ComponentTicks>,
}

impl ComponentColumn {
    /// Creates an empty column for the given runtime layout.
    ///
    /// Nothing is allocated until the first row is pushed. The column holds
    /// plain data only: rows are copied as raw bytes and freed without running
    /// destructors, so the layout must describe a blittable value type and
    /// carry a [`Blittability`] proof. The layout is checked again here, in
    /// every build profile.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorSizeZero`],
    /// [`WorldError::DescriptorAlignmentInvalid`] or
    /// [`WorldError::DescriptorLayoutInvalid`].
    pub fn new(layout: ColumnLayout) -> Result<Self, WorldError> {
        validate_component_layout(layout.size, layout.align)?;
        let ops = ColumnOps::blittable(layout.blittability);
        let row_buffer = unallocated_pointer(layout.align);
        Ok(Self {
            element_layout: layout,
            ops,
            identity: ColumnIdentity::Descriptor,
            row_buffer,
            len: 0,
            capacity: 0,
            ticks: Vec::new(),
        })
    }

    /// Creates an empty column for a compile-time Rust element type.
    ///
    /// The counterpart of [`Self::new`]: same buffer, same descriptor, but the
    /// ops table is generated from `T` and typed accessors are allowed.
    ///
    /// `shared` selects [`ColumnIdentity::Shared`], for a type compiled into
    /// more than one binary. See that variant for what the caller must have
    /// checked first.
    ///
    /// # Errors
    ///
    /// As [`Self::new`], when size and alignment cannot describe an allocation.
    pub fn new_native<T: 'static>(schema_hash: u64, shared: bool) -> Result<Self, WorldError> {
        Self::from_native_info(NativeColumnInfo::of::<T>(shared), schema_hash)
    }

    /// Creates an empty column from a native description.
    ///
    /// The non-generic twin of [`Self::new_native`], for the archetype, which
    /// builds columns from a registered factory and has no `T` in scope.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorAlignmentInvalid`] or
    /// [`WorldError::DescriptorLayoutInvalid`] when size and alignment cannot
    /// describe an allocation.
    pub fn from_native_info(
        column_info: NativeColumnInfo,
        schema_hash: u64,
    ) -> Result<Self, WorldError> {
        // SAFETY: this proof is never read for a native column, whose drop code
        // comes from `column_info.ops`. The layout type requires one, though,
        // so the claim is made where it is provably unused.
        let blittability = unsafe { Blittability::assume() };
        let layout = ColumnLayout::new_native(
            column_info.size,
            column_info.align,
            schema_hash,
            blittability,
        )?;
        let row_buffer = unallocated_pointer(layout.align);
        Ok(Self {
            element_layout: layout,
            ops: column_info.ops,
            identity: if column_info.shared {
                ColumnIdentity::Shared
            } else {
                ColumnIdentity::Native(column_info.type_id)
            },
            row_buffer,
            len: 0,
            capacity: 0,
            ticks: Vec::new(),
        })
    }

    /// The per-type table this column releases its rows through.
    pub fn ops(&self) -> ColumnOps {
        self.ops
    }

    /// Points the column at a newer generation's table.
    ///
    /// A column outlives the image that filled it, so its drop code has to be
    /// pointed at code that is still mapped. Before dropping old rows, the
    /// migration path also points it at the *retiring* generation's table, so
    /// the code that allocated those rows is the code that releases them.
    pub fn refresh_ops(&mut self, ops: ColumnOps) {
        self.ops = ops;
    }

    /// What a typed accessor must prove before reading these rows.
    pub fn identity(&self) -> ColumnIdentity {
        self.identity
    }

    /// Panics unless `T` may be used to read this column's rows.
    ///
    /// The check mirrors the identity's own promise: an exact `TypeId` for a
    /// native column, layout equality for a shared one, and a panic for a
    /// descriptor column, which has no typed reading.
    fn assert_element_type<T: 'static>(&self) {
        match self.identity {
            ColumnIdentity::Native(type_id) => assert!(
                type_id == std::any::TypeId::of::<T>(),
                "column holds a different component type than the requested one"
            ),
            ColumnIdentity::Shared => assert!(
                std::mem::size_of::<T>() == self.element_layout.size
                    && std::mem::align_of::<T>() == self.element_layout.align,
                "shared column layout {}/{} does not match the requested type's {}/{}",
                self.element_layout.size,
                self.element_layout.align,
                std::mem::size_of::<T>(),
                std::mem::align_of::<T>()
            ),
            ColumnIdentity::Descriptor => {
                panic!("a descriptor-only column has no Rust type; reach its rows as bytes")
            }
        }
    }

    /// Returns the number of initialized rows in this column.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether this column holds no rows.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the size in bytes of a single stored component instance.
    pub fn element_size(&self) -> usize {
        self.element_layout.size
    }

    /// Returns the alignment in bytes required by a single stored instance.
    pub fn alignment(&self) -> usize {
        self.element_layout.align
    }

    /// Returns the schema hash identifying the stored component type.
    pub fn schema_hash(&self) -> u64 {
        self.element_layout.schema_hash
    }

    /// Returns a raw pointer to the underlying buffer.
    ///
    /// The pointer is dangling before the first allocation and is invalidated
    /// by any later reallocation or by dropping the column.
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.row_buffer.as_ptr()
    }

    /// Refuses a raw-byte write when this column's rows own resources.
    ///
    /// The byte mutators below write rows with `memcpy` and never consult
    /// [`ColumnOps::drop_range`]. That is right for the plain data a descriptor
    /// column stores, and wrong for a column whose element type has a
    /// destructor: writing a row would leak the value it replaced. The
    /// [`Self::take_row_from`] move is exempt, because it transfers ownership
    /// instead of creating a new one.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::ColumnRowsAreNotPlainData`] when the column's ops
    /// table reports a non-trivial drop.
    #[inline]
    fn require_plain_data(&self) -> Result<(), WorldError> {
        if self.ops.trivial_drop {
            return Ok(());
        }
        Err(WorldError::ColumnRowsAreNotPlainData)
    }

    /// Appends a zero-initialized row to this column.
    ///
    /// Grows the column when needed and leaves the new row's bytes zeroed.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorLayoutInvalid`] when the element layout
    /// cannot describe the next allocation, or
    /// [`WorldError::ColumnRowsAreNotPlainData`] when the column's rows own
    /// resources - see [`Self::require_plain_data`].
    pub fn push_zeroed(&mut self) -> Result<(), WorldError> {
        self.require_plain_data()?;
        self.reserve_one_row()?;
        // SAFETY: reserve_one_row guarantees one writable, correctly aligned
        // slot.
        unsafe {
            std::ptr::write_bytes(
                self.row_buffer
                    .as_ptr()
                    .add(self.len * self.element_layout.size),
                0,
                self.element_layout.size,
            )
        };
        self.ticks.push(ComponentTicks::default());
        self.len += 1;
        Ok(())
    }

    /// Appends a row containing a copy of the given bytes.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::ColumnRowsAreNotPlainData`] when the column's rows
    /// own resources - see [`Self::require_plain_data`] - or
    /// [`WorldError::DescriptorSizeMismatch`] when `bytes` does not contain
    /// exactly `element_size()` bytes, or
    /// [`WorldError::DescriptorLayoutInvalid`] when the element layout cannot
    /// describe the next allocation.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<(), WorldError> {
        self.require_plain_data()?;
        if bytes.len() != self.element_layout.size {
            return Err(WorldError::DescriptorSizeMismatch);
        }
        self.reserve_one_row()?;
        // SAFETY: source and destination are valid for exactly one element and
        // cannot overlap because the source is outside this column's spare slot.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.row_buffer
                    .as_ptr()
                    .add(self.len * self.element_layout.size),
                self.element_layout.size,
            );
        }
        self.ticks.push(ComponentTicks::default());
        self.len += 1;
        Ok(())
    }

    /// Moves the row at `index` out of `source` and appends it here.
    ///
    /// This is how an entity changes archetype, and the only way a row moves
    /// from one column to another. Ownership moves with the bytes: they are
    /// copied here, and the source row is released **without** running its
    /// destructor, because whatever it owned is now owned here. That is what
    /// makes the move correct for a row that holds resources - a `Vec`, a
    /// `String` - which the byte-copying mutators refuse.
    ///
    /// Both halves happen in one call, so every row is dropped exactly once:
    /// no intermediate state lets a caller forget the release or run it twice.
    /// A move makes no plain-data claim, but it can still be the push that
    /// grows this column. Call [`Self::reserve_row`] first when a multi-column
    /// move must not fail partway.
    ///
    /// # Errors
    ///
    /// [`WorldError::DescriptorSizeMismatch`] when the two columns differ in
    /// element size, [`WorldError::DescriptorRowInvalid`] when `index` is out of
    /// bounds of `source`, and [`WorldError::DescriptorLayoutInvalid`] when the
    /// next allocation cannot be represented. Nothing is moved in any of those
    /// cases.
    pub fn take_row_from(&mut self, source: &mut Self, index: usize) -> Result<(), WorldError> {
        if self.element_layout.size != source.element_layout.size {
            return Err(WorldError::DescriptorSizeMismatch);
        }
        if index >= source.len {
            return Err(WorldError::DescriptorRowInvalid);
        }
        self.reserve_one_row()?;
        // SAFETY: `reserve_one_row` guarantees one writable, correctly aligned
        // slot here, the source row is initialized and within its allocation,
        // and the two columns are distinct objects so the regions cannot
        // overlap.
        unsafe {
            std::ptr::copy_nonoverlapping(
                source
                    .row_buffer
                    .as_ptr()
                    .add(index * source.element_layout.size),
                self.row_buffer
                    .as_ptr()
                    .add(self.len * self.element_layout.size),
                self.element_layout.size,
            );
        }
        // The row's change-detection metadata travels with it, which is what
        // makes a migrated component keep the ticks it was carrying instead of
        // looking freshly added to every `Changed<T>` system.
        self.ticks
            .push(source.ticks.get(index).copied().unwrap_or_default());
        self.len += 1;
        source.swap_remove_forget(index);
        Ok(())
    }

    /// Reserves room for one more row, growing by the column's own policy.
    ///
    /// A caller that moves a whole entity's row uses this to make every
    /// destination column ready before it moves the first one, which makes a
    /// multi-column move all-or-nothing. Prefer it over [`Self::reserve_rows`]
    /// for a single row: `reserve_rows` allocates the exact target, which would
    /// defeat amortized growth if it ran on every archetype move.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorLayoutInvalid`] when the doubled
    /// capacity cannot be represented.
    pub fn reserve_row(&mut self) -> Result<(), WorldError> {
        self.reserve_one_row()
    }

    /// Overwrites the row at `index` with a copy of the given bytes.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::ColumnRowsAreNotPlainData`] when the column's rows
    /// own resources - see [`Self::require_plain_data`] - or
    /// [`WorldError::DescriptorRowInvalid`] when `index` is out of bounds or
    /// `bytes` does not contain exactly `element_size()` bytes.
    pub fn set_bytes(&mut self, index: usize, bytes: &[u8]) -> Result<(), WorldError> {
        self.require_plain_data()?;
        if index >= self.len || bytes.len() != self.element_layout.size {
            return Err(WorldError::DescriptorRowInvalid);
        }
        // SAFETY: the checked row is initialized and bytes has one element's size.
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                self.row_buffer
                    .as_ptr()
                    .add(index * self.element_layout.size),
                self.element_layout.size,
            );
        }
        Ok(())
    }

    // ---------------------------------------------------------------------
    // Change detection
    //
    // A column's ticks live beside its rows and use the same index, so nothing
    // outside has to maintain a second container in parallel. `relayout` needs
    // no tick handling for the same reason: reshaping a row does not change
    // which rows exist, and the ticks belong to the column being reshaped
    // rather than to something left behind.
    // ---------------------------------------------------------------------

    /// Change-detection metadata for every row, in row order.
    pub fn ticks(&self) -> &[ComponentTicks] {
        &self.ticks
    }

    /// Mutable change-detection metadata for every row, in row order.
    pub fn ticks_mut(&mut self) -> &mut [ComponentTicks] {
        &mut self.ticks
    }

    /// Change-detection metadata for one row, or `None` when out of bounds.
    pub fn row_ticks(&self, index: usize) -> Option<&ComponentTicks> {
        self.ticks.get(index)
    }

    /// Mutable change-detection metadata for one row, or `None` when out of
    /// bounds.
    pub fn row_ticks_mut(&mut self, index: usize) -> Option<&mut ComponentTicks> {
        self.ticks.get_mut(index)
    }

    /// Replaces one row's change-detection metadata.
    ///
    /// Returns `false` when `index` names no row, so a caller that computed
    /// the row elsewhere finds out rather than silently writing nothing.
    pub fn set_row_ticks(&mut self, index: usize, ticks: ComponentTicks) -> bool {
        match self.ticks.get_mut(index) {
            Some(slot) => {
                *slot = ticks;
                true
            }
            None => false,
        }
    }

    /// Mutable change-detection metadata for one row, with no bounds check.
    ///
    /// # Safety
    ///
    /// `index` must be in range, and no other reference to that row's ticks may
    /// exist for the lifetime of the returned one. The query layer resolves the
    /// bound once per archetype rather than per row, which is why this exists.
    pub unsafe fn row_ticks_mut_unchecked(&mut self, index: usize) -> &mut ComponentTicks {
        // SAFETY: the caller guarantees the index and the exclusivity.
        unsafe { &mut *self.ticks.as_mut_ptr().add(index) }
    }

    /// The typed rows and their ticks, both mutable at once.
    ///
    /// Rows and ticks are separate fields, so one call can hand out both
    /// without the caller needing two independent borrows of the column - which
    /// is what the language bindings need to expose writable component data.
    ///
    /// # Panics
    ///
    /// If `T` is not this column's element type.
    pub fn rows_and_ticks_mut<T: 'static>(&mut self) -> (&mut [T], &mut [ComponentTicks]) {
        self.assert_element_type::<T>();
        // SAFETY: the element type was just checked, rows `0..len` are
        // initialized, and the buffer is allocated and aligned for `T`.
        let rows = unsafe {
            std::slice::from_raw_parts_mut(self.row_buffer.as_ptr().cast::<T>(), self.len)
        };
        (rows, &mut self.ticks)
    }

    /// The raw row buffer and the ticks beside it, both mutable at once.
    ///
    /// The type-erased twin of [`Self::rows_and_ticks_mut`], for the language
    /// bindings that address rows as bytes. The pointer is valid only until the
    /// column next grows or is dropped.
    pub fn raw_rows_and_ticks_mut(&mut self) -> (*mut u8, usize, &mut [ComponentTicks]) {
        (self.row_buffer.as_ptr(), self.len, &mut self.ticks)
    }

    /// The rows as `T`, read-only.
    ///
    /// The counterpart of [`Self::rows_and_ticks_mut`] for a caller that copies
    /// rows out without moving them, such as the play-mode storage copy.
    ///
    /// # Panics
    ///
    /// If `T` is not this column's element type.
    pub fn rows<T: 'static>(&self) -> &[T] {
        self.assert_element_type::<T>();
        // SAFETY: the element type was just checked, rows `0..len` are
        // initialized, and the buffer is allocated and aligned for `T`.
        unsafe { std::slice::from_raw_parts(self.row_buffer.as_ptr().cast::<T>(), self.len) }
    }

    /// Returns the raw bytes of the row at `index`, or `None` when out of bounds.
    pub fn bytes(&self, index: usize) -> Option<&[u8]> {
        if index >= self.len {
            return None;
        }
        // SAFETY: the row is initialized and lives for the returned shared borrow.
        Some(unsafe {
            std::slice::from_raw_parts(
                self.row_buffer
                    .as_ptr()
                    .add(index * self.element_layout.size),
                self.element_layout.size,
            )
        })
    }

    // ---------------------------------------------------------------------
    // Typed access
    //
    // Available only where a Rust type names the rows. `assert_element_type`
    // is what separates the native and descriptor cases at the point of use.
    // The arithmetic is the same either way - `row_buffer + index *
    // element_size` - so a typed read costs exactly what a byte read costs.
    // ---------------------------------------------------------------------

    /// Appends a typed value, growing the column when needed.
    ///
    /// # Panics
    ///
    /// If `T` is not this column's element type, or the column cannot grow.
    /// Growth failure panics instead of returning an error: a native column's
    /// layout was validated at registration, so the only way to get here is a
    /// capacity overflow, and the callers - command flush, migration, archetype
    /// move - have no way to recover from that.
    pub fn push<T: 'static>(&mut self, value: T) {
        self.assert_element_type::<T>();
        self.reserve_one_row()
            .expect("a native column must be able to grow for one more row");
        self.ticks.push(ComponentTicks::default());
        // SAFETY: `reserve_one_row` guaranteed capacity for one more row, and
        // the element type was just checked, so the slot is correctly aligned
        // and sized for `T`. `write` does not drop the uninitialized
        // destination.
        unsafe {
            self.row_buffer
                .as_ptr()
                .add(self.len * self.element_layout.size)
                .cast::<T>()
                .write(value);
        }
        self.len += 1;
    }

    /// Shared reference to one row, bounds- and type-checked.
    ///
    /// # Panics
    ///
    /// If `T` is not this column's element type, or `index` is out of range.
    pub fn get<T: 'static>(&self, index: usize) -> &T {
        self.assert_element_type::<T>();
        assert!(index < self.len, "component index {index} out of range");
        // SAFETY: index and element type were both just checked.
        unsafe { self.get_unchecked::<T>(index) }
    }

    /// Mutable twin of [`Self::get`].
    ///
    /// # Panics
    ///
    /// As [`Self::get`].
    pub fn get_mut<T: 'static>(&mut self, index: usize) -> &mut T {
        self.assert_element_type::<T>();
        assert!(index < self.len, "component index {index} out of range");
        // SAFETY: index and element type were both just checked.
        unsafe { self.get_mut_unchecked::<T>(index) }
    }

    /// Shared reference to one row with no checks.
    ///
    /// # Safety
    ///
    /// `T` must be this column's element type and `index` must be in range.
    /// The query layer resolves both once per archetype rather than per row,
    /// which is why this exists.
    pub unsafe fn get_unchecked<T: 'static>(&self, index: usize) -> &T {
        // SAFETY: the caller guarantees the type and the index.
        unsafe {
            &*self
                .row_buffer
                .as_ptr()
                .add(index * self.element_layout.size)
                .cast::<T>()
        }
    }

    /// Mutable twin of [`Self::get_unchecked`].
    ///
    /// # Safety
    ///
    /// As [`Self::get_unchecked`], and no other reference to the row may
    /// exist for the lifetime of the returned one.
    pub unsafe fn get_mut_unchecked<T: 'static>(&mut self, index: usize) -> &mut T {
        // SAFETY: the caller guarantees the type, the index and the exclusivity.
        unsafe {
            &mut *self
                .row_buffer
                .as_ptr()
                .add(index * self.element_layout.size)
                .cast::<T>()
        }
    }

    /// The rows as a typed slice.
    ///
    /// # Panics
    ///
    /// If `T` is not this column's element type.
    pub fn as_slice<T: 'static>(&self) -> &[T] {
        self.assert_element_type::<T>();
        if self.len == 0 {
            return &[];
        }
        // SAFETY: the element type was checked and `len` rows are initialized.
        unsafe { std::slice::from_raw_parts(self.row_buffer.as_ptr().cast::<T>(), self.len) }
    }

    /// Mutable twin of [`Self::as_slice`].
    ///
    /// # Panics
    ///
    /// As [`Self::as_slice`].
    pub fn as_mut_slice<T: 'static>(&mut self) -> &mut [T] {
        self.assert_element_type::<T>();
        if self.len == 0 {
            return &mut [];
        }
        // SAFETY: as above, with exclusivity from `&mut self`.
        unsafe { std::slice::from_raw_parts_mut(self.row_buffer.as_ptr().cast::<T>(), self.len) }
    }

    /// Removes one row by swapping the last row into its place and returns it.
    ///
    /// # Panics
    ///
    /// If `T` is not this column's element type, or `index` is out of range.
    pub fn swap_remove<T: 'static>(&mut self, index: usize) -> T {
        self.assert_element_type::<T>();
        assert!(index < self.len, "component index {index} out of range");
        // SAFETY: the row is initialized and the type was checked, so it can be
        // moved out. The tail row is then moved over the hole and the length
        // drops, so no row is ever read twice or dropped twice.
        unsafe {
            let base = self.row_buffer.as_ptr();
            let taken = base
                .add(index * self.element_layout.size)
                .cast::<T>()
                .read();
            let last = self.len - 1;
            if index != last {
                std::ptr::copy_nonoverlapping(
                    base.add(last * self.element_layout.size),
                    base.add(index * self.element_layout.size),
                    self.element_layout.size,
                );
            }
            self.len = last;
            self.ticks.swap_remove(index);
            taken
        }
    }

    /// Reserves capacity for at least `additional` more rows.
    ///
    /// Allocates the exact target rather than doubling toward it: a caller
    /// that says how many rows are coming knows better than the growth policy.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorLayoutInvalid`] when the capacity or the
    /// allocation layout it implies cannot be represented.
    ///
    /// # Panics
    ///
    /// Panics when the tick vector cannot reserve `additional` more entries,
    /// because the capacity it would need overflows what a `Vec` can hold.
    pub fn reserve_rows(&mut self, additional: usize) -> Result<(), WorldError> {
        let target = self
            .len
            .checked_add(additional)
            .ok_or(WorldError::DescriptorLayoutInvalid)?;
        self.grow_to(target)?;
        self.ticks.reserve(additional);
        Ok(())
    }

    /// Returns a read pointer to one row's bytes.
    ///
    /// The byte-level counterpart of the typed accessors, for a caller that
    /// has established a row's layout by other means - the renderer reads a
    /// shared component this way, because the component it wants was validated
    /// by name and layout rather than by `TypeId`.
    ///
    /// Returns `None` when `index` is out of range.
    pub fn row_ptr(&self, index: usize) -> Option<*const u8> {
        if index >= self.len {
            return None;
        }
        // SAFETY: the index is in range, so the offset stays inside the
        // allocation's initialized prefix.
        Some(unsafe {
            self.row_buffer
                .as_ptr()
                .add(index * self.element_layout.size)
        })
    }

    /// Returns a raw read pointer to the first row.
    pub fn raw_ptr(&self) -> *const u8 {
        self.row_buffer.as_ptr()
    }

    /// Runtime identity of the element type, or `None` when no exact `TypeId`
    /// names it (a shared or descriptor column).
    pub fn element_type_id(&self) -> Option<std::any::TypeId> {
        match self.identity {
            ColumnIdentity::Native(type_id) => Some(type_id),
            ColumnIdentity::Shared | ColumnIdentity::Descriptor => None,
        }
    }

    /// Whether the element type is identified by layout rather than `TypeId`.
    pub fn has_shared_identity(&self) -> bool {
        matches!(self.identity, ColumnIdentity::Shared)
    }

    /// Returns a typed read pointer to the first row.
    ///
    /// # Panics
    ///
    /// If `T` is not this column's element type.
    pub fn as_ptr<T: 'static>(&self) -> *const T {
        self.assert_element_type::<T>();
        self.row_buffer.as_ptr().cast::<T>()
    }

    /// Iterates the rows as `T`.
    ///
    /// # Panics
    ///
    /// If `T` is not this column's element type.
    pub fn iter<T: 'static>(&self) -> impl Iterator<Item = &T> {
        self.as_slice::<T>().iter()
    }

    /// Reserves capacity for `additional` more rows of `T`.
    ///
    /// # Panics
    ///
    /// If `T` is not this column's element type, or the capacity cannot
    /// describe an allocation - the one caller reserves a capacity it chose.
    pub fn reserve<T: 'static>(&mut self, additional: usize) {
        self.assert_element_type::<T>();
        self.reserve_rows(additional)
            .expect("reserved capacity must describe a valid allocation");
    }

    /// Rewrites every row into a new layout, following `plan`.
    ///
    /// The column keeps its row count and row order, so indices callers hold -
    /// including every `EntityLocation` that names a row - stay valid. Its
    /// change ticks also stay as they are: a reshaped component was not added
    /// or changed. Any byte the plan does not cover is zeroed, so a field added
    /// to a foreign-language component starts at a defined value.
    ///
    /// Each row is transformed through a scratch copy, so a plan that moves
    /// fields inside a row cannot read a byte it has already overwritten. The
    /// buffer is rewritten in place when the element size and alignment are
    /// unchanged, and reallocated at the same capacity otherwise.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorSizeZero`],
    /// [`WorldError::DescriptorAlignmentInvalid`] or
    /// [`WorldError::DescriptorLayoutInvalid`] when the new layout cannot describe
    /// storage, and [`WorldError::DescriptorRowInvalid`] when a planned field reads
    /// or writes past the edge of a row. Nothing is modified in either case.
    #[cfg(feature = "development")]
    pub fn relayout(
        &mut self,
        layout: ColumnLayout,
        plan: &FieldPlan,
    ) -> Result<usize, WorldError> {
        validate_component_layout(layout.size, layout.align)?;
        plan.validate(self.element_layout.size, layout.size)?;
        let previous_size = self.element_layout.size;
        Ok(self.relayout_validated(layout, plan, previous_size))
    }

    /// [`Self::relayout`] for a caller that has already validated.
    ///
    /// This cannot fail: the only failure modes are the layout and plan checks
    /// above, and both depend only on data the caller can check once for every
    /// column it is about to rewrite. That lets a world-level relayout refuse a
    /// bad plan before it touches the first column, instead of migrating some
    /// columns and leaving others unchanged.
    #[cfg(feature = "development")]
    pub(crate) fn relayout_validated(
        &mut self,
        layout: ColumnLayout,
        plan: &FieldPlan,
        previous_size: usize,
    ) -> usize {
        // The plan was validated against `previous_size`, so that - not this
        // column's current element size - is what the scratch copy and the
        // source stride have to be. A disagreement is a bug in the engine: the
        // world-level relayout checks every column before calling here.
        debug_assert_eq!(
            previous_size, self.element_layout.size,
            "a column's element size disagrees with the layout its plan was validated against"
        );
        let rows = self.len;
        let old_size = previous_size;
        let old_align = self.element_layout.align;
        let previous_row_buffer = self.row_buffer;
        let in_place = layout.size == old_size && layout.align == old_align;

        // Step 1: Prepare the destination buffer. An unchanged shape keeps the
        // buffer it has; anything else gets a fresh one at the same capacity,
        // because a relayout is not a reason to re-grow on the next push.
        let mut new_row_buffer = previous_row_buffer;
        if !in_place {
            new_row_buffer = if self.capacity == 0 || layout.size == 0 {
                // Nothing to hold, so nothing to allocate - a shape that became
                // a marker keeps only its row count.
                unallocated_pointer(layout.align)
            } else {
                let bytes = layout
                    .size
                    .checked_mul(self.capacity)
                    .expect("component column too large");
                let allocation = Layout::from_size_align(bytes, layout.align)
                    .expect("component layout validated by the caller");
                // SAFETY: the allocation has non-zero size, checked above.
                let pointer = unsafe { alloc(allocation) };
                NonNull::new(pointer).unwrap_or_else(|| handle_alloc_error(allocation))
            };
        }

        // Step 2: Transform one row at a time. The scratch copy is what makes
        // this correct in place as well as across buffers: every source byte is
        // read before the destination row is zeroed.
        if rows != 0 {
            let mut scratch = vec![0_u8; old_size];
            for row in 0..rows {
                // SAFETY: `row < rows` counts only initialized rows, and both
                // buffers are allocated for at least `rows` rows.
                unsafe {
                    let source = previous_row_buffer.as_ptr().add(row * old_size);
                    std::ptr::copy_nonoverlapping(source, scratch.as_mut_ptr(), old_size);
                    let destination = new_row_buffer.as_ptr().add(row * layout.size);
                    std::ptr::write_bytes(destination, 0, layout.size);
                    plan.apply(
                        &scratch,
                        std::slice::from_raw_parts_mut(destination, layout.size),
                    );
                }
            }
        }

        // Step 3: Release the buffer that is no longer the column's, and adopt
        // the new shape. `capacity != 0` is the same "is there an allocation"
        // test `Drop` uses, so a column that never allocated never reaches
        // `dealloc`.
        if !in_place {
            // As in `Drop`: a zero-sized predecessor allocated nothing, however
            // much capacity it reported.
            if self.capacity != 0 && old_size != 0 {
                // SAFETY: this is the live allocation, created with this layout.
                unsafe {
                    dealloc(
                        previous_row_buffer.as_ptr(),
                        Layout::from_size_align_unchecked(old_size * self.capacity, old_align),
                    );
                }
            }
            self.row_buffer = new_row_buffer;
        }
        self.element_layout = layout;
        rows
    }

    /// Removes the row at `index` without releasing what it owns.
    ///
    /// [`Self::swap_remove_discard`] minus the `drop_range` call, for the one
    /// case where that call would be wrong: the caller has moved the row
    /// elsewhere, so releasing it here would be a double free. Private because
    /// [`Self::take_row_from`] is the only correct way to reach it - it pairs
    /// the removal with the push that took ownership.
    ///
    /// # Panics
    ///
    /// Panics when `index` is out of bounds.
    fn swap_remove_forget(&mut self, index: usize) {
        assert!(index < self.len);
        let last = self.len - 1;
        if index != last {
            // SAFETY: both rows are within this allocation; copy permits overlap.
            unsafe {
                std::ptr::copy(
                    self.row_buffer
                        .as_ptr()
                        .add(last * self.element_layout.size),
                    self.row_buffer
                        .as_ptr()
                        .add(index * self.element_layout.size),
                    self.element_layout.size,
                );
            }
        }
        self.len = last;
        self.ticks.swap_remove(index);
    }

    /// Removes the row at `index` by swapping in the last row.
    ///
    /// Keeps the column dense and runs in O(1), but does not preserve row
    /// ordering.
    ///
    /// # Panics
    ///
    /// Panics when `index` is out of bounds.
    pub fn swap_remove_discard(&mut self, index: usize) {
        assert!(index < self.len);
        // Release the row through the ops table, if the column has one, before
        // the swap-in overwrites its bytes.
        if !self.ops.trivial_drop {
            // SAFETY: index < len, so the row is initialized and addressable.
            unsafe {
                (self.ops.drop_range)(
                    self.row_buffer
                        .as_ptr()
                        .add(index * self.element_layout.size),
                    1,
                )
            };
        }
        let last = self.len - 1;
        if index != last {
            // SAFETY: both rows are within this allocation; copy permits overlap.
            unsafe {
                std::ptr::copy(
                    self.row_buffer
                        .as_ptr()
                        .add(last * self.element_layout.size),
                    self.row_buffer
                        .as_ptr()
                        .add(index * self.element_layout.size),
                    self.element_layout.size,
                );
            }
        }
        self.len = last;
        self.ticks.swap_remove(index);
    }

    /// Ensures capacity for at least one more row, growing the buffer when full.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorLayoutInvalid`] when the doubled capacity
    /// or the allocation layout it implies cannot be represented.
    fn reserve_one_row(&mut self) -> Result<(), WorldError> {
        // Step 1: Return early when the column still has a spare slot.
        if self.len < self.capacity {
            return Ok(());
        }

        // Step 2: Compute the doubled capacity and hand the rest to `grow_to`.
        //
        // The floor applies after doubling, so the first allocation is four
        // rows; applied first it would be eight.
        let new_capacity = self
            .capacity
            .checked_mul(2)
            .ok_or(WorldError::DescriptorLayoutInvalid)?
            .max(4);
        self.grow_to(new_capacity)
    }

    /// Reallocates the buffer to hold exactly `new_capacity` rows.
    ///
    /// This is the single growth implementation. [`Self::reserve_one_row`]
    /// picks the doubling policy and [`Self::reserve_rows`] picks an exact
    /// target, but both allocate and copy through here, so the two can never
    /// disagree about how a buffer is moved.
    ///
    /// # Errors
    ///
    /// Returns [`WorldError::DescriptorLayoutInvalid`] when the capacity or the
    /// allocation layout it implies cannot be represented.
    fn grow_to(&mut self, new_capacity: usize) -> Result<(), WorldError> {
        // Step 1: Nothing to do when the current allocation is already large
        // enough.
        if new_capacity <= self.capacity {
            return Ok(());
        }
        // Step 2: A zero-sized element needs no allocation. Every row is the
        // same (aligned, dangling) address. The capacity is still tracked,
        // because `reserve_one_row` doubles it and the row count is the
        // column's only real state.
        if self.element_layout.size == 0 {
            self.capacity = new_capacity;
            return Ok(());
        }
        // Step 3: Compute the allocation layout, allocate the new buffer and
        // migrate the existing rows.
        let allocation_layout = Layout::from_size_align(
            self.element_layout
                .size
                .checked_mul(new_capacity)
                .ok_or(WorldError::DescriptorLayoutInvalid)?,
            self.element_layout.align,
        )
        .map_err(|_| WorldError::DescriptorLayoutInvalid)?;
        // SAFETY: allocation_layout has non-zero size because component size is
        // validated.
        let new_row_buffer = unsafe { alloc(allocation_layout) };
        let new_row_buffer =
            NonNull::new(new_row_buffer).unwrap_or_else(|| handle_alloc_error(allocation_layout));
        if self.len != 0 {
            // SAFETY: both allocations are valid and non-overlapping.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.row_buffer.as_ptr(),
                    new_row_buffer.as_ptr(),
                    self.len * self.element_layout.size,
                );
                dealloc(
                    self.row_buffer.as_ptr(),
                    Layout::from_size_align_unchecked(
                        self.element_layout.size * self.capacity,
                        self.element_layout.align,
                    ),
                );
            }
        }

        // Step 4: Adopt the new allocation as the column's buffer.
        self.row_buffer = new_row_buffer;
        self.capacity = new_capacity;
        Ok(())
    }
}

// SAFETY: Two premises, each enforced by named code rather than asserted here.
//
// 1. Every field of a descriptor component is a blittable value type. The
//    engine holds the evidence: every `ColumnLayout` carries a `Blittability`
//    proof, the host builds its own only after the managed blittable vocabulary
//    vetted the manifest's fields, and no column can be built without a layout.
//    That is what makes the raw `ptr::copy` in `swap_remove` and the
//    destructor-free paths below correct: there is no ownership to duplicate or
//    release.
// 2. Access is serialised by the same scheduler rules as native columns. See
//    `SystemAccess::conflicts_with`, which reports a conflict whenever either
//    system writes a component the other one reads or writes.
unsafe impl Send for ComponentColumn {}
// SAFETY: Shared access to a column only permits reading rows, which is sound
// under the same two premises as `Send` above: rows are blittable value types
// and access is serialised by the scheduler, so no write can race a read.
unsafe impl Sync for ComponentColumn {}

impl Drop for ComponentColumn {
    /// Releases every row through the column's ops table, then frees the
    /// buffer itself.
    ///
    /// There is one release path for both kinds of column. A descriptor
    /// column's table reports `trivial_drop`, so the loop is skipped and the
    /// teardown stays destructor-free as the plain-data design promises. A
    /// native column's table runs the element type's own destructors.
    ///
    /// Which table that is matters during a reload: before the column is
    /// dropped, it is pointed at the *retiring* generation's table, so the code
    /// that allocated the rows is the code that releases them, not the code of
    /// the generation that is arriving.
    fn drop(&mut self) {
        if !self.ops.trivial_drop && self.len != 0 {
            // SAFETY: rows 0..len are initialized, correctly aligned for the
            // element type, and the table is the one that wrote them.
            unsafe { (self.ops.drop_range)(self.row_buffer.as_ptr(), self.len) };
        }
        // `size != 0` joins the capacity test because a zero-sized column
        // reports a capacity it never allocated for.
        if self.capacity != 0 && self.element_layout.size != 0 {
            // SAFETY: this is the live allocation created by reserve_one_row.
            unsafe {
                dealloc(
                    self.row_buffer.as_ptr(),
                    Layout::from_size_align_unchecked(
                        self.element_layout.size * self.capacity,
                        self.element_layout.align,
                    ),
                );
            }
        }
    }
}

// =============================================================================
// ArchetypeId
// =============================================================================

/// ArchetypeId uniquely identifies an archetype by its component mask.
///
/// Derived from the archetype's [`ComponentMask`], guaranteeing a 1:1
/// mapping without a separate lookup table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArchetypeId(
    /// The packed component mask bits identifying this archetype.
    pub u128,
);

// =============================================================================
// Archetype
// =============================================================================

/// Core storage unit grouping entities with the same component set.
///
/// Uses a Structure of Arrays (SoA) layout for cache-friendly bulk iteration.
pub struct Archetype {
    /// Unique identifier derived from this archetype's component mask.
    pub id: ArchetypeId,
    /// Component types stored in this archetype, in registration order.
    ///
    /// Needed for iteration and lookup.
    pub component_types: Vec<ComponentId>,
    /// Bitmask of the stored component types for fast query matching.
    pub component_mask: ComponentMask,
    /// Component storage, keyed by component id.
    ///
    /// One map holds both kinds of column: a column built for a Rust type and
    /// one built from a manifest descriptor have the same form - bytes, a
    /// layout and an ops table - so a second map would hold nothing extra.
    /// Which constructor built a column is recorded in its identity, not in
    /// where the column is stored.
    pub component_storages: ComponentColumns,
    /// Entities currently stored in this archetype.
    pub entities: Vec<Entity>,
}

impl Archetype {
    /// Creates a new archetype with storage for the specified component types.
    ///
    /// `storage_factories` provides a way to create storage for each component
    /// type by [`ComponentId`], allowing archetype creation without knowing the
    /// concrete types.
    ///
    /// # Panics
    ///
    /// Panics when a component type has no entry in `storage_factories`, which
    /// happens when `world.register_component::<T>()` was not called for it.
    pub fn new(
        id: ArchetypeId,
        component_types: Vec<ComponentId>,
        component_mask: ComponentMask,
        storage_factories: &HashMap<ComponentId, StorageFactory>,
    ) -> Self {
        // Step 1: Pre-size the storage maps for the component count.
        let component_count = component_types.len();
        let _zone = crate::profile_scope!(
            "archetype new",
            [("Component types in this archetype: {}", component_count)]
        );
        let mut component_storages = ComponentColumns::with_capacity(component_count);

        // Step 2: Create storage for each component type using its factory.
        for &component_id in &component_types {
            let factory = storage_factories.get(&component_id)
                .unwrap_or_else(|| panic!(
                    "Component type {:?} not registered. Call world.register_component::<T>() first.",
                    component_id
                ));
            match factory {
                StorageFactory::Native(column_info) => {
                    // Build the erased column from the registered type
                    // description. The column is a concrete `ComponentColumn`
                    // with no trait-object vtable, so it stays valid across
                    // module unloads. It is stored under the component's id
                    // rather than its `TypeId`, so a component shared between
                    // binaries resolves to it from either one.
                    component_storages.insert(
                        component_id,
                        ComponentColumn::from_native_info(*column_info, 0)
                            .expect("a registered native layout is valid by construction"),
                    );
                }
                StorageFactory::Descriptor(layout) => {
                    // Both paths into the registry - `register_component_descriptor`
                    // and the relayout pass - check the layout before storing
                    // it, so the constructor can only repeat that check here.
                    let column = ComponentColumn::new(layout.clone())
                        .expect("registration validated this layout");
                    component_storages.insert(component_id, column);
                }
            }
        }

        // Step 3: Emit allocation telemetry and assemble the archetype.
        crate::profile_message!(
            "archetype {:?} allocated with {} component storage columns for up to 0 entities",
            id,
            component_count,
        );

        Self {
            id,
            component_types,
            component_mask,
            component_storages,
            entities: Vec::new(),
        }
    }

    /// Checks whether this archetype stores the component type at the given mask bit.
    ///
    /// Uses the bitmask for O(1) lookup instead of a linear search through
    /// the component types.
    #[inline]
    pub fn has_component_bit(&self, bit: u8) -> bool {
        self.component_mask.has_bit(bit)
    }

    /// Checks whether this archetype stores the specified component type.
    ///
    /// Note: This uses an O(n) linear search. Prefer `has_component_bit` with
    /// a pre-looked-up bit index for hot paths.
    pub fn has_component<T: Component>(&self) -> bool {
        self.component_types.contains(&ComponentId::of::<T>())
    }

    /// Checks whether this archetype contains every component a query requires.
    ///
    /// Called in the query setup hot path for every archetype.
    #[inline]
    pub fn matches_mask(&self, required_mask: &ComponentMask) -> bool {
        self.component_mask.contains_all(required_mask)
    }

    /// Returns the number of entities in this archetype.
    #[inline]
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    /// Returns the number of entities in this archetype (alias for [`Self::len`]).
    ///
    /// Provided for API consistency with other collection types.
    #[inline]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Returns `true` when this archetype contains no entities.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Builds a human-readable summary of this archetype.
    ///
    /// Includes the archetype ID, the entity count, and the names of the
    /// stored component types. Unknown component IDs render as "Unknown".
    pub fn get_archetype_info(&self, registry: &crate::component::ComponentRegistry) -> String {
        let component_names: Vec<String> = self
            .component_types
            .iter()
            .map(|component_id| {
                registry
                    .get_name(component_id)
                    .unwrap_or("Unknown")
                    .to_string()
            })
            .collect();

        format!(
            "Archetype {:?}: {} entities, components: [{}]",
            self.id,
            self.entities.len(),
            component_names.join(", ")
        )
    }

    /// Logs information about this archetype (component names and entity count).
    #[cold]
    pub fn print_info(&self, registry: &crate::component::ComponentRegistry) {
        let component_names: Vec<&str> = self
            .component_types
            .iter()
            .map(|component_id| registry.get_name(component_id).unwrap_or("Unknown"))
            .collect();
        pill_core::info!(
            target: telemetry_target::ECS,
            archetype = ?self.id,
            entities = self.entities.len(),
            components = %component_names.join(", "),
            "archetype info"
        );
    }

    /// Estimates the memory footprint of this archetype in bytes.
    ///
    /// Adds up the entity array's capacity, every component column's rows at
    /// the entity count, and the change-detection tick vectors. Each column
    /// reports its own element size, because the archetype has no other way to
    /// learn it.
    pub fn memory_estimate(&self) -> usize {
        // Step 1: Account for the entity ID array (16 bytes per entity).
        let mut total = self.entities.capacity() * std::mem::size_of::<crate::entity::Entity>();

        // Step 2: Account for each component column (entity count as a lower bound).
        for &component_id in &self.component_types {
            if let Some(column) = self.component_storages.get(component_id) {
                // A column's capacity is not exposed, so the entity count is a
                // lower bound; the real allocation may be larger.
                total += self.entities.len() * column.element_size();
            }
        }

        // Step 3: Account for the change-detection tick vectors (8 bytes per tick).
        total += self.component_types.len() * self.entities.capacity() * 8;

        // Step 4: Approximate the `ComponentColumns` map's per-entry overhead.
        total += self.component_types.len() * 64;

        total
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Checks that a size and alignment can describe a descriptor component layout.
///
/// Registration and relayout both call this, so the two can never disagree
/// about what a usable layout is.
///
/// A zero width is refused here, and [`validate_native_component_layout`]
/// allows it. That difference is deliberate. A descriptor's width comes from a
/// manifest written by another language, where zero means the declaration is
/// wrong. A Rust type's width was computed by the compiler, and zero is the
/// normal way to write a marker component (`struct Enemy;`). The native path
/// turns this refusal into an `.expect`, so refusing a zero width there would
/// abort the process on every marker component.
pub(crate) fn validate_component_layout(size: usize, align: usize) -> Result<(), WorldError> {
    if size == 0 {
        return Err(WorldError::DescriptorSizeZero);
    }
    validate_native_component_layout(size, align)
}

/// Checks that a size and alignment can describe a native component layout.
///
/// This is [`validate_component_layout`] without the zero-width refusal: a
/// zero-sized Rust type is a marker component, and its column owns no
/// allocation.
pub(crate) fn validate_native_component_layout(
    size: usize,
    align: usize,
) -> Result<(), WorldError> {
    if align == 0 || !align.is_power_of_two() {
        return Err(WorldError::DescriptorAlignmentInvalid);
    }
    if Layout::from_size_align(size, align).is_err() {
        return Err(WorldError::DescriptorLayoutInvalid);
    }
    Ok(())
}

/// A non-null, correctly aligned pointer for a column that owns no allocation.
///
/// `NonNull::dangling()` gives address 1, which is aligned for `u8` and for
/// nothing wider. A zero-sized column keeps this pointer for its whole life,
/// because it never allocates, and its rows are read and written through it.
/// Deriving the address from the element's alignment keeps those accesses
/// well-formed. It is the same trick `NonNull::<T>::dangling` plays, written
/// out for an alignment that is only known at run time.
fn unallocated_pointer(align: usize) -> NonNull<u8> {
    debug_assert!(align.is_power_of_two(), "alignment was validated");
    // SAFETY: a validated alignment is a power of two, so it is never zero.
    unsafe { NonNull::new_unchecked(align as *mut u8) }
}

/// Whether `bytes` written at `offset` runs past the end of a row of `size`
/// bytes.
///
/// A helper rather than two `checked_add` expressions inline: the same question
/// is asked of the source and the destination of every instruction.
fn out_of_bounds(offset: usize, bytes: usize, size: usize) -> bool {
    match offset.checked_add(bytes) {
        Some(end) => end > size,
        None => true,
    }
}

/// Reports a lookup for a column the archetype does not have.
///
/// It is a separate function so the panic path stays out of the accessors'
/// inlined fast path. `#[cold]` marks it as unlikely, so the branch predictor
/// expects the other way.
#[cold]
#[inline(never)]
fn missing_column<T: Component>() -> ! {
    panic!(
        "archetype has no column for component {} ({:?}); the archetype's mask \
         and its columns disagree, or the component was never registered",
        std::any::type_name::<T>(),
        ComponentId::of::<T>(),
    )
}

/// Drops nothing: the element type owns no resources.
///
/// # Safety
///
/// Trivially safe - it reads neither argument.
unsafe fn drop_range_of_nothing(_rows_ptr: *mut u8, _row_count: usize) {}

/// Drops `row_count` values of type `T` starting at `rows_ptr`.
///
/// # Safety
///
/// `rows_ptr` must address `row_count` live, correctly aligned `T` values.
unsafe fn drop_range_of<T>(rows_ptr: *mut u8, row_count: usize) {
    if row_count == 0 || !std::mem::needs_drop::<T>() {
        return;
    }
    // SAFETY: the caller guarantees `row_count` live, aligned `T` values at
    // `rows_ptr`.
    unsafe {
        std::ptr::drop_in_place(std::ptr::slice_from_raw_parts_mut(
            rows_ptr.cast::<T>(),
            row_count,
        ));
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod archetype_tests;
