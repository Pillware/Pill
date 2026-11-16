//! Generic, type-erased component field access for editor-style tools.
//!
//! # Responsibilities
//!
//! - Own the field vocabulary an inspector speaks: [`FieldValue`], the
//!   [`ComponentFieldError`] returned by every refused edit, and the helpers
//!   that name a field's kind (`is_handle_tag`, `is_entity_tag`,
//!   `is_container_tag`).
//! - Keep the module's public paths stable. [`FieldValue`] and
//!   [`ComponentFieldError`] are the names the rest of the engine uses, and
//!   [`MAX_EXPANDED_ELEMENTS`] and [`format_component_debug`] are re-exported
//!   from the files that own them.
//!
//! # Design
//!
//! Components live in erased columns, and nothing downcasts through `dyn Any`.
//! So the field layout that `#[derive(PillComponent)]` and
//! `#[derive(PillMirror)]` register (`name`, `type_tag`, `offset`, `size`,
//! `align`, `element_count`) is the only generic way to reach a component's
//! value. This module is the safe wrapper over that layout, split by concern:
//!
//! - `mod.rs` (here): the vocabulary, the errors and the tag helpers.
//! - `row.rs`: finds a component's field in a live row, then reads or writes
//!   it. Every entry point that resolves a live row looks the component up
//!   through **the entity's own archetype**. That archetype is the one place
//!   that records which generation created the data; a lookup by name across
//!   the whole registry can find several registrations under one name. No
//!   pointer into the row is handed to the caller.
//! - `codec.rs`: turns a row's bytes into a [`FieldValue`] and back.
//! - `expand.rs`: flattens a nested `struct:` field into leaf rows while a
//!   component registers.
//!
//! [`FieldValue`] covers the closed `type_tag` vocabulary of the derive. A
//! `struct:` field, or any other tag the engine cannot interpret, is read as
//! [`FieldValue::Opaque`] and is never edited by value, so an inspector can
//! show such a field instead of hiding it. A restore writes the captured
//! bytes back exactly as they were read. A nested `struct:` field whose type
//! has a known layout never appears as `Opaque`: the world flattens it into
//! dotted leaf rows at registration
//! ([`World::register_component_with_layout`]), so `Opaque` appears only when
//! no layout could be found.
//!
//! A tag is the derive's name for a field's type, not a type itself. `f32`,
//! `u64` and `bool` are scalars, `struct:Handle<Mesh>` is an asset reference,
//! `entity` is an entity reference, `array:<tag>` is a fixed array, and
//! `struct:<path>` is anything the engine cannot interpret. Reading a tag is
//! always a string comparison against this closed set, never a type lookup.
//!
//! # Row byte convention
//!
//! A row's bytes are native-endian. That includes an asset handle's two `u32`s
//! (index first, then generation) and an entity reference's slot and
//! generation. The C# mirror reads the same row in the same process, so no
//! byte order crosses a machine boundary. A field that ever does would need its
//! own convention stated here.

// Submodules split by concern: the row layer, the codec and the layout
// expansion done at registration. The two `pub use` lines keep the public
// paths of the items those files own.
mod codec;
mod expand;
mod row;

// Current crate
use codec::ScalarKind;

pub use expand::MAX_EXPANDED_ELEMENTS;
pub use row::format_component_debug;

/// One read or edited value in the engine's reflected vocabulary.
///
/// Covers the `type_tag` vocabulary of the derive:
///
/// - the scalar primitives the engine can describe;
/// - whole fixed-size arrays;
/// - heap-backed containers: `List` reports a sequence's element count
///   instead of copying it, and `Text` decodes a string;
/// - handle and entity references;
/// - `Opaque`, for a field the engine can find but not interpret (a `struct:`
///   tag whose type is not resolvable, or any tag outside the vocabulary).
///
/// `Opaque` carries the field's raw bytes so a UI can show them. The UI never
/// builds one; it only hands back bytes it captured earlier. Those write back
/// verbatim when their tag and size match the field's own descriptor, which is
/// how a restore re-creates a destroyed row. `List` and `Text` are read-only
/// through this path; writes go through the container field's generated
/// accessors, which reach the live buffer instead of rebuilding the row.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    /// A 32-bit IEEE-754 floating point value.
    F32(f32),
    /// A 64-bit IEEE-754 floating point value.
    F64(f64),
    /// A signed 8-bit integer.
    I8(i8),
    /// A signed 16-bit integer.
    I16(i16),
    /// A signed 32-bit integer.
    I32(i32),
    /// A signed 64-bit integer.
    I64(i64),
    /// An unsigned 8-bit integer.
    U8(u8),
    /// An unsigned 16-bit integer.
    U16(u16),
    /// An unsigned 32-bit integer.
    U32(u32),
    /// An unsigned 64-bit integer.
    U64(u64),
    /// A boolean, stored as a single Rust byte in the component image.
    Bool(bool),
    /// A pointer-width unsigned integer.
    Usize(usize),
    /// A pointer-width signed integer.
    Isize(isize),
    /// A whole fixed-size array field, one entry per element in layout order.
    Array(Vec<FieldValue>),
    /// A heap-backed sequence field (`vec:<element_tag>` or
    /// `dynbuf:<element_tag>`), reported as its element count instead of being
    /// copied: a component `Vec` may hold millions of elements, and the length
    /// is what an inspector needs. Read-only here; use the field's accessors to
    /// edit elements.
    List {
        /// Element type tag from the closed vocabulary (`f32`, `struct:path`).
        element_tag: &'static str,
        /// Number of live elements in the buffer right now.
        element_count: usize,
    },
    /// A heap-backed UTF-8 string field (`string`), decoded for display.
    /// Read-only here; use the field's accessors to replace its contents.
    Text(String),
    /// A reference to an asset: a `Handle<T>` field's slot index and
    /// generation, with the asset type the field was registered with.
    ///
    /// Writable like a scalar, but a write that names another asset type than
    /// the field's is refused. `asset_type` is the type's registered name, the
    /// one `.meta` files and the import registry use; it is `None` when the
    /// field carries no record of it. A cleared reference holds
    /// [`Handle::INVALID`](crate::Handle::INVALID)'s pair, `u32::MAX` twice.
    Asset {
        /// The registered name of the referenced asset's type.
        asset_type: Option<String>,
        /// The handle's slot index.
        index: u32,
        /// The handle's generation.
        generation: u32,
    },
    /// A reference to another entity: an `Entity` field's slot id and
    /// generation.
    ///
    /// Writable like a scalar. The pair is the *handle*, so it resolves only
    /// in the world that issued it. A reference saved for a later run stores
    /// the entity's [`EntityId`](crate::EntityId) instead: the writer looks
    /// the id up, and the loader turns it back into a handle.
    Entity {
        /// The referenced entity's slot id.
        id: u64,
        /// The slot's generation, which tells a live entity from a recycled
        /// slot's previous occupant.
        generation: u32,
    },
    /// A field of a declared fieldless enum: its discriminant, read as the
    /// enum's integer representation.
    ///
    /// `type_tag` is the field's `enum:` tag, which carries every variant's
    /// name and discriminant ([`parse_enum_tag`] reads them back). A write is
    /// refused unless the tag is the field's own and the discriminant names
    /// one of its variants, so no row ever holds a value the enum cannot be.
    Enum {
        /// The field's registered `enum:` tag.
        type_tag: &'static str,
        /// The discriminant the row holds.
        discriminant: i64,
    },
    /// A field the engine can locate but not interpret.
    ///
    /// It is never edited by value. The only write it accepts is its own
    /// captured bytes, when their tag and size still match the field's
    /// descriptor.
    Opaque {
        /// The registered `type_tag` of the field (for example `struct:path`).
        type_tag: &'static str,
        /// The raw bytes of the field, copied out of the component row.
        bytes: Vec<u8>,
    },
}

impl FieldValue {
    /// The cleared-reference value: [`Handle::INVALID`](crate::Handle::INVALID)'s
    /// pair, with no recorded asset type.
    ///
    /// The field layer's one spelling of "this reference points at nothing",
    /// so a saver, a restorer and a clearer agree without re-deriving it.
    #[must_use]
    pub fn unset_asset() -> Self {
        Self::Asset {
            asset_type: None,
            index: crate::asset::INVALID_ASSET_INDEX,
            generation: crate::asset::INVALID_ASSET_GENERATION,
        }
    }

    /// Whether this value is a reference that points at nothing.
    ///
    /// True for a cleared asset reference ([`Self::unset_asset`]) whatever
    /// wrote it, and for the null entity handle.
    #[must_use]
    pub fn references_nothing(&self) -> bool {
        match self {
            Self::Asset {
                index, generation, ..
            } => {
                (*index, *generation)
                    == (
                        crate::asset::INVALID_ASSET_INDEX,
                        crate::asset::INVALID_ASSET_GENERATION,
                    )
            }
            Self::Entity { id, generation } => crate::Entity::from_raw(*id, *generation).is_null(),
            _ => false,
        }
    }

    /// The closed-vocabulary tag a value's variant corresponds to.
    ///
    /// Mirrors the `type_tag` strings the derive emits, so a mismatch can name
    /// both sides. `Array` and `Opaque` are not derive tags; they use the
    /// labels below for diagnostics.
    pub(crate) fn variant_tag(&self) -> &'static str {
        match self {
            Self::F32(_) => ScalarKind::F32.tag(),
            Self::F64(_) => ScalarKind::F64.tag(),
            Self::I8(_) => ScalarKind::I8.tag(),
            Self::I16(_) => ScalarKind::I16.tag(),
            Self::I32(_) => ScalarKind::I32.tag(),
            Self::I64(_) => ScalarKind::I64.tag(),
            Self::U8(_) => ScalarKind::U8.tag(),
            Self::U16(_) => ScalarKind::U16.tag(),
            Self::U32(_) => ScalarKind::U32.tag(),
            Self::U64(_) => ScalarKind::U64.tag(),
            Self::Bool(_) => ScalarKind::Bool.tag(),
            Self::Usize(_) => ScalarKind::Usize.tag(),
            Self::Isize(_) => ScalarKind::Isize.tag(),
            Self::Array(_) => "array",
            Self::List { .. } => "list",
            Self::Text(_) => "text",
            Self::Asset { .. } => "asset",
            Self::Entity { .. } => "entity",
            Self::Enum { .. } => "enum",
            Self::Opaque { .. } => "opaque",
        }
    }

    /// The name of the variant an enum value holds, or `None` for any other
    /// value and for a discriminant no variant declares.
    #[must_use]
    pub fn enum_variant_name(&self) -> Option<&'static str> {
        let Self::Enum {
            type_tag,
            discriminant,
        } = self
        else {
            return None;
        };
        parse_enum_tag(type_tag)?.variant_name(*discriminant)
    }
}

// =============================================================================
// EnumFieldTag
// =============================================================================

/// The parsed form of an `enum:` field tag.
///
/// The tag is `enum:<representation>:<type>{<Variant>=<discriminant>,...}`.
/// The world builds it once, when a component registers, from the enum's
/// declaration in the registering DLL; carrying the variants in the tag keeps
/// them readable after that DLL unloads, and lets a reader that holds only a
/// value or a descriptor list the choices without asking the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumFieldTag {
    /// The integer type the enum is stored as (`u8`, `i32`, ...).
    pub representation: &'static str,
    /// The enum's name as the component field spells it.
    pub type_name: &'static str,
    /// Every variant's name and discriminant, in declaration order.
    pub variants: Vec<(&'static str, i64)>,
}

impl EnumFieldTag {
    /// The name of the variant with `discriminant`, if one declares it.
    #[must_use]
    pub fn variant_name(&self, discriminant: i64) -> Option<&'static str> {
        self.variants
            .iter()
            .find(|(_, value)| *value == discriminant)
            .map(|(name, _)| *name)
    }

    /// The discriminant of the variant called `name`, if there is one.
    #[must_use]
    pub fn discriminant_of(&self, name: &str) -> Option<i64> {
        self.variants
            .iter()
            .find(|(variant, _)| *variant == name)
            .map(|(_, value)| *value)
    }
}

/// Reads an `enum:` field tag back into its representation, type name and
/// variants. Returns `None` for any other tag, or one that is malformed.
#[must_use]
pub fn parse_enum_tag(type_tag: &'static str) -> Option<EnumFieldTag> {
    let body = type_tag.strip_prefix("enum:")?;
    let (representation, rest) = body.split_once(':')?;
    let (type_name, variant_list) = rest.split_once('{')?;
    let variant_list = variant_list.strip_suffix('}')?;
    let mut variants = Vec::new();
    // An enum with no variants has an empty list; every other entry is
    // `Name=discriminant`.
    for entry in variant_list.split(',').filter(|entry| !entry.is_empty()) {
        let (name, value) = entry.split_once('=')?;
        variants.push((name, value.parse::<i64>().ok()?));
    }
    Some(EnumFieldTag {
        representation,
        type_name,
        variants,
    })
}

/// Builds the `enum:` tag for a field of the enum `type_name` laid out as
/// `layout`, in the form [`parse_enum_tag`] reads.
pub(crate) fn format_enum_tag(
    type_name: &str,
    layout: &crate::component_registry::EnumLayout,
) -> String {
    let variant_list = layout
        .variants
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "enum:{}:{type_name}{{{variant_list}}}",
        layout.representation
    )
}

/// Whether a type tag is a declared enum field's.
pub(crate) fn is_enum_tag(type_tag: &str) -> bool {
    type_tag.starts_with("enum:")
}

/// Why a generic field read or write was refused.
///
/// Every failure is all-or-nothing: nothing is written when any validation
/// fails. Each variant names the component or field it is about, so the editor
/// can show it in a console instead of guessing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentFieldError {
    /// The entity handle is not alive.
    EntityNotFound,
    /// The entity is alive but does not carry a component of that name.
    ComponentNotFound {
        /// The requested component type name.
        component: String,
    },
    /// The component was resolved through a live entity, but its column or row
    /// is missing from the archetype.
    ///
    /// This is a registration/storage desync, not a dead handle: the entity
    /// exists and it carries the component's id, so the storage the id names
    /// should be there.
    ComponentStorageMissing {
        /// The component type name.
        component: String,
    },
    /// The component is registered but has no registered field layout, so it
    /// cannot be inspected or edited generically.
    ComponentHasNoFieldLayout {
        /// The component type name.
        component: String,
    },
    /// The component's field layout does not name that field.
    FieldNotFound {
        /// The component type name.
        component: String,
        /// The requested field name.
        field: String,
    },
    /// The value variant does not match the field's registered `type_tag`.
    TypeMismatch {
        /// The field being edited.
        field: String,
        /// The tag the layout declares.
        expected: &'static str,
        /// The tag the caller supplied.
        found: &'static str,
    },
    /// A whole-array write supplied the wrong number of elements.
    ArrayLengthMismatch {
        /// The array field being edited.
        field: String,
        /// The element count the layout declares.
        expected: usize,
        /// The element count the caller supplied.
        found: usize,
    },
    /// An element write addressed an element outside the fixed array.
    ArrayIndexOutOfRange {
        /// The array field being edited.
        field: String,
        /// The requested index.
        index: usize,
        /// The element count the layout declares.
        length: usize,
    },
    /// An asset reference write named another asset type than the field's.
    AssetTypeMismatch {
        /// The field being edited.
        field: String,
        /// The asset type the field references.
        expected: String,
        /// The asset type the caller supplied.
        found: String,
    },
    /// The field's tag is outside the editable vocabulary (`struct:` and
    /// anything the derive does not emit a scalar tag for).
    UnsupportedField {
        /// The field being edited.
        field: String,
        /// Why it cannot be edited.
        reason: &'static str,
    },
}

impl ComponentFieldError {
    /// A short, stable message for the editor's console.
    pub(crate) fn summary(&self) -> String {
        match self {
            Self::EntityNotFound => "entity is not alive".to_string(),
            Self::ComponentNotFound { component } => {
                format!("component `{component}` is not on this entity")
            }
            Self::ComponentStorageMissing { component } => {
                format!("component `{component}` has no storage column or row in the archetype")
            }
            Self::ComponentHasNoFieldLayout { component } => {
                format!("component `{component}` has no registered field layout")
            }
            Self::FieldNotFound { component, field } => {
                format!("component `{component}` has no field `{field}`")
            }
            Self::TypeMismatch {
                field,
                expected,
                found,
            } => format!("field `{field}` expects `{expected}` but received `{found}`"),
            Self::ArrayLengthMismatch {
                field,
                expected,
                found,
            } => format!("field `{field}` expects {expected} elements but received {found}"),
            Self::ArrayIndexOutOfRange {
                field,
                index,
                length,
            } => format!("field `{field}` index {index} is out of range (length {length})"),
            Self::AssetTypeMismatch {
                field,
                expected,
                found,
            } => format!("field `{field}` references a `{expected}` but received a `{found}`"),
            Self::UnsupportedField { field, reason } => {
                format!("field `{field}` is not editable: {reason}")
            }
        }
    }
}

impl std::fmt::Display for ComponentFieldError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.summary())
    }
}

impl std::error::Error for ComponentFieldError {}

// =============================================================================
// Free Functions
// =============================================================================

/// Whether a field's type tag names a heap-backed container.
///
/// `vec:<tag>` and `dynbuf:<tag>` fields hold a `(pointer, length, capacity)`
/// header, and `string` holds the same three words with UTF-8 text behind
/// them. All three are read by following a pointer that the value in the row
/// owns. A descriptor row is raw bytes with no Rust value in it, so a layout
/// that claims one of these tags on such a row promises a header the bytes
/// never held.
pub(crate) fn is_container_tag(type_tag: &str) -> bool {
    type_tag == "string" || type_tag.starts_with("vec:") || type_tag.starts_with("dynbuf:")
}

/// Whether a type tag is a `Handle<T>` field's (`struct:Handle<Mesh>`, or
/// `struct:Handle` when the derive could not read the type argument).
pub(crate) fn is_handle_tag(type_tag: &str) -> bool {
    type_tag == "struct:Handle" || type_tag.starts_with("struct:Handle<")
}

/// Whether a type tag is an `Entity` field's.
pub(crate) fn is_entity_tag(type_tag: &str) -> bool {
    type_tag == "entity"
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod component_field_tests;
