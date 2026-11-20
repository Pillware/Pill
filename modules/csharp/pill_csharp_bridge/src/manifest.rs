//! Managed component manifest schema, validation, and engine type mapping.
//!
//! # Responsibilities
//!
//! - Define the manifest entries a managed assembly submits: components and
//!   resources, with their identities, sizes and field trees.
//! - Validate field trees against the blittable vocabulary the byte-level
//!   column storage requires.
//! - Map managed field types onto the engine's field descriptors, and intern
//!   the strings a manifest-declared component has no static to borrow from.
//!
//! # Design
//!
//! A manifest is the managed side's only description of a component: its
//! identity, layout and fields, sent as JSON bytes in the pending-manifest
//! payload. The storage it feeds is a raw byte buffer, so the validation here
//! is what makes that storage's `Send`/`Sync` claim true: every field must be
//! a blittable value with no ownership and nothing to release.
//!
//! The types stay `Deserialize`-only. The host parses a manifest and never
//! writes one, so there is no serializer to keep in step with.

// Standard library
use std::collections::{HashMap, HashSet};

// External crates
use pill_core::error::CSharpError;
use pill_engine::archetype::{FieldPlan, LayoutField};
use pill_engine::component_registry::{ComponentFieldDescriptor, FieldDecorator};
use pill_engine::{ComponentId, Engine};
use serde::Deserialize;

// Current crate
use super::components::{stable_component_id, StableComponentId};
use super::resources::{ManagedResourceDeclaration, ResourceFieldLayout};

// =============================================================================
// Constants
// =============================================================================

/// Maximum nesting depth accepted in a managed component field tree.
///
/// Real component layouts never exceed a handful of levels. The limit stays
/// below `serde_json`'s own parser recursion limit, so this check, not a vague
/// parser error, refuses a pathological manifest.
const MAX_FIELD_NESTING_DEPTH: usize = 32;

/// The nested-struct marker. Validation descends into a nested struct's
/// fields instead of treating it as a leaf.
const STRUCT_TYPE: &str = "struct";

/// The prefix of a `Handle<T>` field's type. What follows is the Rust type
/// path of `T`, as the C# mirror of `T` carries it.
const HANDLE_PREFIX: &str = "handle:";

/// A handle's size and alignment: two `u32`s, `index` then `generation`.
const HANDLE_SIZE: usize = 8;
const HANDLE_ALIGNMENT: usize = 4;

// =============================================================================
// Blittable Vocabulary
// =============================================================================

/// What one accepted leaf type means to the host.
///
/// Four checks read this table: blittability, natural alignment, the engine
/// type tag and default-literal parsing. A type is accepted exactly when it has
/// a row here, so the checks cannot disagree about what a type means.
struct BlittableLeaf {
    /// Canonical managed type name as the manifest writes it.
    type_name: &'static str,
    /// Alignment the managed side gives the field in its own layout.
    alignment: usize,
    /// Engine type tag, or `None` for a type the engine cannot decode. Such a
    /// field keeps its bytes in storage but is left out of the registered
    /// editor layout.
    type_tag: Option<&'static str>,
    /// How a `default:` literal for this type parses.
    default_literal: DefaultLiteral,
}

/// How a leaf type's `default:` literal turns into native bytes.
#[derive(Clone, Copy)]
enum DefaultLiteral {
    /// `true` or `false`.
    Boolean,
    /// An integer or float literal of this exact width.
    Signed8,
    Signed16,
    Signed32,
    Signed64,
    Unsigned8,
    Unsigned16,
    Unsigned32,
    Unsigned64,
    Float32,
    Float64,
    /// No supported literal form. The pointer-width types and `Entity` use
    /// this, so a manifest that declares a default for one of them is refused
    /// by name.
    Unsupported,
}

/// The managed blittable field vocabulary: one row per accepted leaf type.
///
/// This table is what makes `ComponentColumn`'s `unsafe impl Send`/`Sync`
/// sound and what lets its rows be freed without drop glue. The column is a raw
/// byte buffer: rows are moved with `ptr::copy` and the buffer is freed without
/// running any destructor. Every field must therefore be a blittable value with
/// no ownership, no interior pointer, and nothing to release.
///
/// A nested value type is a shape, not a leaf: it has no row here. Validation
/// recurses into its own fields instead of looking it up.
const BLITTABLE_LEAVES: &[BlittableLeaf] = &[
    BlittableLeaf {
        type_name: "System.Byte",
        alignment: 1,
        type_tag: Some("u8"),
        default_literal: DefaultLiteral::Unsigned8,
    },
    BlittableLeaf {
        type_name: "System.SByte",
        alignment: 1,
        type_tag: Some("i8"),
        default_literal: DefaultLiteral::Signed8,
    },
    BlittableLeaf {
        type_name: "System.Int16",
        alignment: 2,
        type_tag: Some("i16"),
        default_literal: DefaultLiteral::Signed16,
    },
    BlittableLeaf {
        type_name: "System.UInt16",
        alignment: 2,
        type_tag: Some("u16"),
        default_literal: DefaultLiteral::Unsigned16,
    },
    BlittableLeaf {
        type_name: "System.Int32",
        alignment: 4,
        type_tag: Some("i32"),
        default_literal: DefaultLiteral::Signed32,
    },
    BlittableLeaf {
        type_name: "System.UInt32",
        alignment: 4,
        type_tag: Some("u32"),
        default_literal: DefaultLiteral::Unsigned32,
    },
    BlittableLeaf {
        type_name: "System.Int64",
        alignment: 8,
        type_tag: Some("i64"),
        default_literal: DefaultLiteral::Signed64,
    },
    BlittableLeaf {
        type_name: "System.UInt64",
        alignment: 8,
        type_tag: Some("u64"),
        default_literal: DefaultLiteral::Unsigned64,
    },
    // Pointer-width on the host, which is how the managed side marshals them.
    // The engine only ever stores their bytes.
    BlittableLeaf {
        type_name: "System.IntPtr",
        alignment: std::mem::size_of::<usize>(),
        type_tag: None,
        default_literal: DefaultLiteral::Unsupported,
    },
    BlittableLeaf {
        type_name: "System.UIntPtr",
        alignment: std::mem::size_of::<usize>(),
        type_tag: None,
        default_literal: DefaultLiteral::Unsupported,
    },
    BlittableLeaf {
        type_name: "System.Single",
        alignment: 4,
        type_tag: Some("f32"),
        default_literal: DefaultLiteral::Float32,
    },
    BlittableLeaf {
        type_name: "System.Double",
        alignment: 8,
        type_tag: Some("f64"),
        default_literal: DefaultLiteral::Float64,
    },
    BlittableLeaf {
        type_name: "System.Boolean",
        alignment: 1,
        type_tag: Some("bool"),
        default_literal: DefaultLiteral::Boolean,
    },
    // A blittable UTF-16 code unit, the same size as the engine's `u16`.
    // Exposing it as `u16` keeps the field editable.
    BlittableLeaf {
        type_name: "System.Char",
        alignment: 2,
        type_tag: Some("u16"),
        default_literal: DefaultLiteral::Unsigned16,
    },
    // A reference to another entity, mirrored from the runtime's `Entity`
    // handle: a `u64` slot id and a `u32` generation. It is eight-byte aligned
    // because of the `u64`, and it has no default: an unset reference is the
    // world's null handle.
    BlittableLeaf {
        type_name: "Entity",
        alignment: 8,
        type_tag: Some("entity"),
        default_literal: DefaultLiteral::Unsupported,
    },
];

/// The Rust type path of a handle field's asset type, when the field is one.
pub(super) fn handle_asset_path(primitive_type: &str) -> Option<&str> {
    primitive_type
        .strip_prefix(HANDLE_PREFIX)
        .filter(|path| !path.is_empty())
}

/// The engine tag a handle field is recorded under, such as
/// `struct:Handle<Mesh>`. It uses the last segment of the asset type's path,
/// as the derive writes it.
fn handle_tag(asset_path: &str) -> &'static str {
    let short = asset_path.rsplit("::").next().unwrap_or(asset_path);
    intern(&format!("struct:Handle<{short}>"))
}

/// The vocabulary row for one managed type, when it is an accepted leaf.
fn blittable_leaf(primitive_type: &str) -> Option<&'static BlittableLeaf> {
    BLITTABLE_LEAVES
        .iter()
        .find(|leaf| leaf.type_name == primitive_type)
}

/// Whether a manifest may declare a field of this type. That is an accepted
/// leaf, or a nested value struct, whose own fields are validated against this
/// same vocabulary.
fn is_blittable(primitive_type: &str) -> bool {
    primitive_type == STRUCT_TYPE
        || blittable_leaf(primitive_type).is_some()
        || handle_asset_path(primitive_type).is_some()
}

// =============================================================================
// Manifest Schema
// =============================================================================

/// One entry deserialized from the manifest a managed assembly submits.
#[derive(Deserialize)]
pub(super) struct ManagedManifestEntry {
    /// Low 64 bits of the stable component identity.
    pub(super) stable_id_low: u64,
    /// High 64 bits of the stable component identity.
    pub(super) stable_id_high: u64,
    /// Canonical full name used to recompute and verify the identity.
    pub(super) full_name: String,
    /// Total byte size of the component layout.
    pub(super) size: usize,
    /// Required byte alignment of the component layout.
    pub(super) alignment: usize,
    /// Hash of the managed field schema used to match native mirrors.
    pub(super) schema_hash: u64,
    /// Whether the managed side expects a native engine binding.
    pub(super) shared: bool,
    /// What the entry declares: a component, or a resource.
    ///
    /// Defaulted so a payload that omits the tag parses as a list of
    /// components, which keeps the manifest of an older generation readable
    /// during a reload.
    #[serde(default)]
    pub(super) kind: ManifestEntryKind,
    /// The names of live registrations this entry renames onto itself.
    ///
    /// A rename is declared here, not as a new entry: the host finds the
    /// registration that answered to each alias and moves its rows onto this
    /// entry, so a rename keeps its data, where a removal plus an add would
    /// lose it. Defaulted for the same reason `kind` is.
    #[serde(default)]
    pub(super) aliases: Vec<String>,
    /// Top-level field descriptions of the component layout.
    pub(super) fields: Vec<ManagedFieldManifest>,
}

/// What one managed manifest entry declares.
///
/// Resources ride in the same array as components rather than a second
/// document, so one transfer covers a whole generation's declaration and a
/// manifest that fails validation registers nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ManifestEntryKind {
    /// Per-entity storage, registered as a column.
    #[default]
    Component,
    /// One value for the whole world, registered as a foreign resource.
    Resource,
}

/// Deserialized field entry within a managed component manifest.
#[derive(Deserialize)]
pub(super) struct ManagedFieldManifest {
    /// Field name as it appears in the managed schema.
    pub(super) name: String,
    /// Byte offset of the field within its containing struct.
    pub(super) offset: usize,
    /// Byte size of the field.
    pub(super) size: usize,
    /// Managed type name of the field: a leaf type name as the manifest writes
    /// it, `struct` for a nested value type, or `handle:` plus the asset
    /// type's Rust path.
    pub(super) primitive_type: String,
    /// Literal a newly added or reset field starts from, instead of zero.
    #[serde(default)]
    pub(super) default: Option<String>,
    /// Presentation decorators the field declares, in declaration order.
    ///
    /// Defaulted so a payload that omits them parses as an undecorated field.
    #[serde(default)]
    pub(super) decorations: Vec<ManagedFieldDecorator>,
    /// Nested field descriptions when this field is a struct.
    pub(super) fields: Vec<ManagedFieldManifest>,
}

/// One presentation decorator a managed field declares.
///
/// Mirrors the editor's [`FieldDecorator`](pill_engine::component_registry::FieldDecorator)
/// vocabulary. The managed side writes one flat record for the whole
/// vocabulary, so unused payload members arrive as `null` and each variant
/// reads only the members its kind carries. [`validate_decorations`] refuses a
/// payload that cannot mean what it says; nothing here affects storage.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ManagedFieldDecorator {
    /// Unity's `[HideInInspector]`: the Inspector draws no row for the field.
    HideInInspector {},
    /// A separator line with text, drawn above the field.
    Header {
        /// Text the line shows.
        text: String,
    },
    /// A vertical gap above the field.
    Space {
        /// Height of the gap, in pixels.
        number: f32,
    },
    /// Hover text on the field's label.
    Tooltip {
        /// Text shown while the pointer rests on the label.
        text: String,
    },
    /// A slider over the range.
    Range {
        /// Lowest value the slider offers.
        min: f32,
        /// Highest value the slider offers.
        max: f32,
    },
    /// A multi-line text box.
    TextArea {
        /// Rows the Inspector asks the box to show.
        min_lines: usize,
        /// Declared upper row count; the Inspector sizes the box from
        /// `min_lines` alone.
        max_lines: usize,
    },
    /// A fixed-height multi-line text box.
    Multiline {
        /// Rows the box shows.
        lines: usize,
    },
    /// A drag value that never goes below the minimum.
    Min {
        /// Lowest value the drag value accepts.
        number: f32,
    },
}

// =============================================================================
// Validation
// =============================================================================

/// Reject sibling fields that share any byte range.
///
/// Two fields that share bytes would overwrite each other's values with no
/// error, so an overlap and a duplicated offset are both invalid layouts.
///
/// # Errors
///
/// Returns an error naming the first pair of sibling fields that overlap.
pub(super) fn validate_sibling_non_overlap(
    fields: &[ManagedFieldManifest],
    parent_name: &str,
) -> Result<(), String> {
    for (index, left) in fields.iter().enumerate() {
        let left_end = left.offset.saturating_add(left.size);
        for right in &fields[index + 1..] {
            let right_end = right.offset.saturating_add(right.size);
            if left.offset < right_end && right.offset < left_end {
                return Err(format!(
                    "managed fields {} and {} overlap inside {parent_name}",
                    left.name, right.name
                ));
            }
        }
    }
    Ok(())
}

/// Check that one field and all of its nested fields fit inside the struct
/// that contains them, and that sibling fields never overlap.
///
/// The field tree is walked with an explicit worklist, so deeply nested input
/// uses heap memory instead of the stack. The depth limit refuses a
/// pathological manifest before it costs any real work.
///
/// # Errors
///
/// Returns an error when a field overflows its containing struct, names an
/// empty field or type, or declares a type outside the [`BLITTABLE_LEAVES`]
/// vocabulary. It also fails when a leaf sits at an offset that is not a
/// multiple of its natural alignment, when a handle is not exactly
/// [`HANDLE_SIZE`] bytes at a [`HANDLE_ALIGNMENT`]-byte boundary or declares a
/// default, when a default does not parse or does not fill the field exactly,
/// when a decorator is out of range, when sibling fields overlap, or when the
/// nesting limit is exceeded.
pub(super) fn validate_field_manifest(
    field: &ManagedFieldManifest,
    parent_size: usize,
) -> Result<(), String> {
    // Each entry carries the field to inspect, the size of the struct that
    // directly contains it, and that branch's current nesting depth.
    let mut worklist = vec![(field, parent_size, 0_usize)];
    while let Some((field, parent_size, depth)) = worklist.pop() {
        let end = field
            .offset
            .checked_add(field.size)
            .ok_or("managed field range overflow")?;
        if field.name.is_empty() || field.primitive_type.is_empty() || end > parent_size {
            return Err("managed field lies outside its component layout".into());
        }
        // Reject anything that is not a blittable value type. `ComponentColumn`
        // copies rows as raw bytes and frees its buffer without running drop
        // glue. A field that owned a resource would therefore be copied on
        // move and leaked on free, and sharing such a column across threads,
        // which the engine does, would be unsound.
        if !is_blittable(&field.primitive_type) {
            return Err(format!(
                "managed field {} has non-blittable type {}; descriptor components must contain only unmanaged value types",
                field.name, field.primitive_type
            ));
        }
        // A field placed off its natural alignment describes a packed or
        // hand-written layout. The analyzer refuses `StructLayout.Pack` on the
        // managed side (PILL0402), but the analyzer is a compile-time lint on
        // one project's source while this manifest is the runtime trust
        // boundary - a project built without the analyzer reference reaches the
        // host with nothing in between.
        //
        // Each field offset is measured from the struct that directly contains
        // it, so a nested leaf is checked against its own struct. A struct
        // field declares no alignment of its own, so where that struct starts
        // inside the component is not checked here.

        // A handle must be exactly the engine's `Handle<T>`: two `u32`s,
        // aligned to four, with no default (an unset reference is the invalid
        // handle).
        if handle_asset_path(&field.primitive_type).is_some()
            && (field.size != HANDLE_SIZE
                || field.offset % HANDLE_ALIGNMENT != 0
                || field.default.is_some())
        {
            return Err(format!(
                "managed handle field {} must be {HANDLE_SIZE} bytes at a {HANDLE_ALIGNMENT}-byte boundary with no default",
                field.name
            ));
        }
        if let Some(leaf) = blittable_leaf(&field.primitive_type) {
            if field.offset % leaf.alignment != 0 {
                return Err(format!(
                    "managed field {} of type {} sits at offset {}, which is not a multiple of its {}-byte alignment",
                    field.name, field.primitive_type, field.offset, leaf.alignment
                ));
            }
        }
        // Defaults are for leaves only. A struct has no literal form, and a
        // nested default would be a second layout language to validate. The
        // literal is checked against the field's own type here, so a mismatch
        // is a named refusal instead of a surprise at marshalling time.
        if let Some(literal) = &field.default {
            if !field.fields.is_empty() {
                return Err(format!(
                    "managed field {} declares a default, but defaults are only supported on primitive fields",
                    field.name
                ));
            }
            let bytes = parse_default_bytes(&field.primitive_type, literal)?;
            if bytes.len() != field.size {
                return Err(format!(
                    "managed field {} declares a default of {} byte(s) for a {}-byte field",
                    field.name,
                    bytes.len(),
                    field.size
                ));
            }
        }
        // The depth check runs after the field validates so the error always
        // names a well-formed field.
        validate_decorations(&field.decorations, &field.name)?;
        if depth >= MAX_FIELD_NESTING_DEPTH {
            return Err(format!(
                "managed field {} exceeds the maximum nesting depth of {MAX_FIELD_NESTING_DEPTH}",
                field.name
            ));
        }
        validate_sibling_non_overlap(&field.fields, &field.name)?;
        for nested in &field.fields {
            worklist.push((nested, field.size, depth + 1));
        }
    }
    Ok(())
}

/// Reject a presentation decorator that cannot mean what it says.
///
/// A decorator changes only how the Inspector draws a field, but a slider whose
/// minimum is above its maximum, or a text box with zero lines, is a mistake
/// nobody sees: Unity silently ignores bad ranges, and a decorator that is
/// silently ignored is worse than a manifest that is refused.
///
/// # Errors
///
/// Returns an error naming the field and the decorator that is out of range.
pub(super) fn validate_decorations(
    decorations: &[ManagedFieldDecorator],
    field_name: &str,
) -> Result<(), String> {
    for decoration in decorations {
        match decoration {
            ManagedFieldDecorator::HideInInspector {} | ManagedFieldDecorator::Header { .. } => {}
            ManagedFieldDecorator::Tooltip { .. } => {}
            ManagedFieldDecorator::Space { number } if !number.is_finite() || *number < 0.0 => {
                return Err(format!(
                    "managed field {field_name} declares a space of {number}, which is not a finite, non-negative height"
                ));
            }
            ManagedFieldDecorator::Range { min, max }
                if !min.is_finite() || !max.is_finite() || min > max =>
            {
                return Err(format!(
                    "managed field {field_name} declares the range {min}..={max}, which needs finite bounds with the minimum first"
                ));
            }
            ManagedFieldDecorator::TextArea {
                min_lines,
                max_lines,
            } if *min_lines == 0 || min_lines > max_lines => {
                return Err(format!(
                    "managed field {field_name} declares a text area of {min_lines}..{max_lines} lines, which needs at least one line with the minimum first"
                ));
            }
            ManagedFieldDecorator::Multiline { lines } if *lines == 0 => {
                return Err(format!(
                    "managed field {field_name} declares a multiline box of zero lines"
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

// =============================================================================
// Type Mapping
// =============================================================================

/// The editor decorators a managed field declares, in the form the layout
/// carries them.
///
/// Text payloads are owned here: a managed declaration's text comes from the
/// parsed JSON, not from a static, so the descriptor must own it. An empty list
/// returns the borrowed empty slice that every undecorated field shares.
pub(super) fn field_decorations(
    decorations: &[ManagedFieldDecorator],
) -> std::borrow::Cow<'static, [FieldDecorator]> {
    if decorations.is_empty() {
        return std::borrow::Cow::Borrowed(&[]);
    }
    std::borrow::Cow::Owned(
        decorations
            .iter()
            .map(|decoration| match decoration {
                ManagedFieldDecorator::HideInInspector {} => FieldDecorator::HideInInspector,
                ManagedFieldDecorator::Header { text } => {
                    FieldDecorator::Header(text.clone().into())
                }
                ManagedFieldDecorator::Space { number } => FieldDecorator::Space(*number),
                ManagedFieldDecorator::Tooltip { text } => {
                    FieldDecorator::Tooltip(text.clone().into())
                }
                ManagedFieldDecorator::Range { min, max } => FieldDecorator::Range {
                    min: *min,
                    max: *max,
                },
                ManagedFieldDecorator::TextArea {
                    min_lines,
                    max_lines,
                } => FieldDecorator::TextArea {
                    min_lines: *min_lines,
                    max_lines: *max_lines,
                },
                ManagedFieldDecorator::Multiline { lines } => FieldDecorator::Multiline(*lines),
                ManagedFieldDecorator::Min { number } => FieldDecorator::Min(*number),
            })
            .collect(),
    )
}

/// The engine-vocabulary tag a field takes in a migration plan.
///
/// For a leaf it is the tag [`managed_field_layout`] records, and for a handle
/// its `struct:Handle<...>` tag. A nested struct is `struct`, which is also
/// what the plan folds the registry's longer tag to, so both sides compare like
/// with like. A leaf the engine cannot decode (the pointer-width types) carries
/// no tag and reads `unsupported`.
pub(super) fn manifest_field_tag(field: &ManagedFieldManifest) -> &'static str {
    if field.primitive_type == STRUCT_TYPE {
        return STRUCT_TYPE;
    }
    if let Some(asset_path) = handle_asset_path(&field.primitive_type) {
        return handle_tag(asset_path);
    }
    blittable_leaf(&field.primitive_type)
        .and_then(|leaf| leaf.type_tag)
        .unwrap_or("unsupported")
}

/// Parse one field default literal into native-endian bytes.
///
/// Only literals are accepted. A build that compiles the C# ahead of time
/// cannot evaluate an expression, so the expression would be a default that
/// exists in the manifest and not in the running world. The literal is parsed
/// as the field's own declared type, so a mismatch is a named validation error
/// instead of a surprise at marshalling time.
///
/// # Errors
///
/// Returns an error when the type has no supported literal form (a
/// pointer-width type or `Entity`), or when the literal does not parse as that
/// type.
pub(super) fn parse_default_bytes(primitive_type: &str, literal: &str) -> Result<Vec<u8>, String> {
    let trimmed = literal.trim();
    let invalid = || format!("field default `{literal}` is not a valid {primitive_type} literal");
    let Some(leaf) = blittable_leaf(primitive_type) else {
        return Err(format!(
            "field default `{literal}` names a type with no supported literal form ({primitive_type})"
        ));
    };
    match leaf.default_literal {
        DefaultLiteral::Boolean => match trimmed {
            "true" => Ok(vec![1]),
            "false" => Ok(vec![0]),
            _ => Err(invalid()),
        },
        DefaultLiteral::Signed8 => trimmed
            .parse::<i8>()
            .map(|value| value.to_ne_bytes().to_vec())
            .map_err(|_| invalid()),
        DefaultLiteral::Signed16 => trimmed
            .parse::<i16>()
            .map(|value| value.to_ne_bytes().to_vec())
            .map_err(|_| invalid()),
        DefaultLiteral::Signed32 => trimmed
            .parse::<i32>()
            .map(|value| value.to_ne_bytes().to_vec())
            .map_err(|_| invalid()),
        DefaultLiteral::Signed64 => trimmed
            .parse::<i64>()
            .map(|value| value.to_ne_bytes().to_vec())
            .map_err(|_| invalid()),
        DefaultLiteral::Unsigned8 => trimmed
            .parse::<u8>()
            .map(|value| value.to_ne_bytes().to_vec())
            .map_err(|_| invalid()),
        // UTF-16 code units parse as the engine's `u16` alongside the real
        // unsigned 16-bit type.
        DefaultLiteral::Unsigned16 => trimmed
            .parse::<u16>()
            .map(|value| value.to_ne_bytes().to_vec())
            .map_err(|_| invalid()),
        DefaultLiteral::Unsigned32 => trimmed
            .parse::<u32>()
            .map(|value| value.to_ne_bytes().to_vec())
            .map_err(|_| invalid()),
        DefaultLiteral::Unsigned64 => trimmed
            .parse::<u64>()
            .map(|value| value.to_ne_bytes().to_vec())
            .map_err(|_| invalid()),
        DefaultLiteral::Float32 => trimmed
            .parse::<f32>()
            .map(|value| value.to_ne_bytes().to_vec())
            .map_err(|_| invalid()),
        DefaultLiteral::Float64 => trimmed
            .parse::<f64>()
            .map(|value| value.to_ne_bytes().to_vec())
            .map_err(|_| invalid()),
        DefaultLiteral::Unsupported => Err(format!(
            "field default `{literal}` names a type with no supported literal form ({primitive_type})"
        )),
    }
}

/// Reduce a field's type tag to what the migration plan compares.
///
/// For the editor, the registry records a nested struct as
/// `struct:<owner>::<field>`, while a manifest only knows that the field is a
/// struct. Comparing those tags as they are would mark every nested field as
/// retyped, and again on every component rename, because the owner is part of
/// the tag. Both sides are folded to `struct`, so the plan asks the question it
/// means: is this field still a struct?
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) fn plan_tag(type_tag: &str) -> &str {
    if type_tag.starts_with("struct:") {
        "struct"
    } else {
        type_tag
    }
}

/// Strings handed to the engine as `&'static str`, stored once per value.
///
/// `ComponentFieldDescriptor` holds `&'static str` because the derive builds it
/// in an artifact's static data. A manifest-declared component has no statics
/// to borrow from, so its strings have to be given that lifetime by hand.
///
/// Leaking a fresh `Box` for every registration would grow with the reload
/// count, because the C# project re-registers its components on every reload.
/// Interning limits the cost to the project's type set: a name that survives a
/// hundred reloads is stored once.
static INTERNED_FIELD_STRINGS: std::sync::Mutex<Option<HashSet<&'static str>>> =
    std::sync::Mutex::new(None);

/// Return a `&'static str` equal to `text`. Only the first call for a given
/// text allocates.
pub(super) fn intern(text: &str) -> &'static str {
    let mut guard = INTERNED_FIELD_STRINGS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let table = guard.get_or_insert_with(HashSet::new);
    if let Some(existing) = table.get(text) {
        return existing;
    }
    // The first call for a value is the only one that allocates.
    let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
    table.insert(leaked);
    leaked
}

/// Convert the top-level fields of a managed component into engine descriptors.
///
/// Primitive leaves map onto the engine's type-tag vocabulary so the editor
/// can decode and edit them. A nested struct stays opaque: it gets one
/// `struct:` tag and the engine does not walk into it, so its bytes are visible
/// but not interpretable. Field names and struct tags are interned, so a
/// component that is re-registered on every reload pays for its strings once.
pub(super) fn managed_field_layout(
    component_name: &str,
    fields: &[ManagedFieldManifest],
) -> Vec<ComponentFieldDescriptor> {
    let mut field_layout = Vec::new();
    for field in fields {
        // A handle is a leaf the engine decodes as an asset reference. Its
        // asset type is recorded in the world for the component by
        // `record_managed_asset_fields`.
        if let Some(asset_path) = handle_asset_path(&field.primitive_type) {
            field_layout.push(ComponentFieldDescriptor {
                name: intern(&field.name),
                type_tag: handle_tag(asset_path),
                offset: field.offset,
                size: field.size,
                align: HANDLE_ALIGNMENT,
                element_count: 0,
                decorators: field_decorations(&field.decorations),
            });
            continue;
        }
        if field.primitive_type == STRUCT_TYPE {
            field_layout.push(ComponentFieldDescriptor {
                name: intern(&field.name),
                type_tag: intern(&format!("struct:{component_name}::{}", field.name)),
                offset: field.offset,
                size: field.size,
                align: 1,
                element_count: 0,
                decorators: field_decorations(&field.decorations),
            });
            continue;
        }
        // A leaf the engine cannot decode keeps its bytes in storage but is
        // omitted from the registered layout.
        let Some(type_tag) = blittable_leaf(&field.primitive_type).and_then(|leaf| leaf.type_tag)
        else {
            continue;
        };
        field_layout.push(ComponentFieldDescriptor {
            name: intern(&field.name),
            type_tag,
            offset: field.offset,
            size: field.size,
            align: 1,
            element_count: 0,
            decorators: field_decorations(&field.decorations),
        });
    }
    field_layout
}

/// Render a registered field layout as one stable, parseable line so
/// integration suites can check that the managed manifest reached the engine
/// intact. Each field is `name@offset:size:type_tag`, and the entries are
/// joined with `|`.
pub(super) fn format_field_layout_line(field_layout: &[ComponentFieldDescriptor]) -> String {
    field_layout
        .iter()
        .map(|field| {
            format!(
                "{}@{}:{}:{}",
                field.name, field.offset, field.size, field.type_tag
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

// =============================================================================
// Manifest Parsing and Plans
// =============================================================================

/// Parse a managed manifest and check every entry against its own identity.
///
/// Every check here is a property of the manifest alone: identity, uniqueness
/// and the shape of each layout. The startup path and the reload path both run
/// it before they touch the world, so both validate exactly the same thing.
///
/// # Errors
///
/// Returns a [`CSharpError`] when the bytes do not parse as a manifest, when an
/// entry's identity does not match its full name, when two entries declare one
/// identity, or when an alias is empty, names a live declaration or is declared
/// by two entries. It also fails on an invalid layout (a size of zero or above
/// `u32::MAX`, a zero or non-power-of-two alignment, or a layout the allocator
/// cannot express), on overlapping sibling fields, on any error
/// [`validate_field_manifest`] reports, and on a resource marked `shared`.
pub(super) fn parse_and_validate_manifest(
    bytes: &[u8],
) -> Result<Vec<ManagedManifestEntry>, CSharpError> {
    // Step 1: Parse the payload.
    let manifest: Vec<ManagedManifestEntry> = serde_json::from_slice(bytes)?;
    let mut seen = HashSet::new();
    let mut alias_owners: HashMap<&str, &str> = HashMap::new();
    for component in &manifest {
        // Step 2: Check each entry's identity and its aliases.
        let stable_id =
            StableComponentId::from_halves(component.stable_id_low, component.stable_id_high);
        if stable_component_id(&component.full_name) != stable_id {
            return Err(format!(
                "managed component {} has an ID that does not match its canonical full name",
                component.full_name
            )
            .into());
        }
        if !seen.insert(stable_id) {
            // Components and resources share one identity space, so this also
            // catches a resource whose declared name collides with a component's
            // type name. Without the check, two entries would register under one
            // slot.
            return Err(format!(
                "duplicate declaration {} in managed manifest",
                component.full_name
            )
            .into());
        }
        for alias in &component.aliases {
            // An alias is resolved in one step, against live names only. An
            // alias that is itself a declared name, or another entry's alias,
            // therefore has no unambiguous predecessor. It is refused here
            // instead of being resolved by precedence, because the wrong choice
            // would move a live registration's rows onto the wrong entry.
            if alias.trim().is_empty() {
                return Err(format!(
                    "managed component {} declares an empty alias",
                    component.full_name
                )
                .into());
            }
            if manifest.iter().any(|other| other.full_name == *alias) {
                return Err(format!(
                    "managed component {} declares alias `{alias}`, which is a live declaration",
                    component.full_name
                )
                .into());
            }
            if let Some(owner) = alias_owners.insert(alias.as_str(), &component.full_name) {
                return Err(format!(
                    "alias `{alias}` is declared by both {owner} and {}",
                    component.full_name
                )
                .into());
            }
        }
        // Step 3: Check the bounds of the declared layout and its field tree.
        if component.size == 0
            || u32::try_from(component.size).is_err()
            || component.alignment == 0
            || !component.alignment.is_power_of_two()
            || std::alloc::Layout::from_size_align(component.size, component.alignment).is_err()
        {
            return Err(format!(
                "invalid layout for managed component {}",
                component.full_name
            )
            .into());
        }

        // Sibling fields of the component itself must not overlap either.
        validate_sibling_non_overlap(&component.fields, &component.full_name)?;
        for field in &component.fields {
            validate_field_manifest(field, component.size)?;
        }

        // Step 4: Refuse a resource that claims a native binding.
        //
        // A resource is a singleton, so a native binding has nothing to mean
        // for one: there is no column to bind and no native mirror to validate
        // against. The entry is refused where it is declared instead of being
        // silently ignored, so a mis-marked struct becomes a named error.
        if component.kind == ManifestEntryKind::Resource && component.shared {
            return Err(format!(
                "managed resource {} is marked shared; resources have no native binding",
                component.full_name
            )
            .into());
        }
    }
    Ok(manifest)
}

/// Split one parsed manifest into its component and resource halves.
///
/// Both halves went through the same identity, layout and field validation
/// above; only what they are registered as differs.
///
/// # Panics
///
/// Panics when a declared default does not parse. Both callers pass a manifest
/// that [`parse_and_validate_manifest`] already validated, and that function
/// refuses such a default, so this cannot fire for them.
pub(super) fn split_manifest_kinds(
    manifest: Vec<ManagedManifestEntry>,
) -> (Vec<ManagedManifestEntry>, Vec<ManagedResourceDeclaration>) {
    let mut components = Vec::new();
    let mut resources = Vec::new();
    for entry in manifest {
        match entry.kind {
            ManifestEntryKind::Component => components.push(entry),
            ManifestEntryKind::Resource => {
                // Both sides go through the same tag function the component
                // plan uses, so a resource migration compares one vocabulary
                // instead of marking every nested field retyped.
                let fields = entry
                    .fields
                    .iter()
                    .map(|field| ResourceFieldLayout {
                        name: field.name.clone(),
                        type_tag: manifest_field_tag(field).to_owned(),
                        offset: field.offset,
                        size: field.size,
                        default: field.default.as_ref().map(|literal| {
                            parse_default_bytes(&field.primitive_type, literal)
                                .expect("the field default was validated during parsing")
                        }),
                        decorations: field_decorations(&field.decorations).into_owned(),
                    })
                    .collect();
                resources.push(ManagedResourceDeclaration {
                    full_name: entry.full_name,
                    size: entry.size,
                    align: entry.alignment,
                    schema_hash: entry.schema_hash,
                    aliases: entry.aliases,
                    fields,
                });
            }
        }
    }
    (components, resources)
}

/// Build the byte plan from the field layout a component has now to the one a
/// manifest asks for.
///
/// The earlier side comes from the world, not from the previous manifest: the
/// layout registered with a descriptor component is the layout its columns
/// actually use, and that is the only thing a byte copy can be measured
/// against.
///
/// # Panics
///
/// Panics when a declared default does not parse or does not fit its field.
/// [`parse_and_validate_manifest`] refuses both, so a manifest that came
/// through it cannot panic here.
#[cfg_attr(not(feature = "hot_reload"), allow(dead_code))]
pub(super) fn build_field_plan(
    engine: &Engine,
    component_id: ComponentId,
    fields: &[ManagedFieldManifest],
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
    // The manifest names a managed type. Mapping it with the same tag function
    // `managed_field_layout` uses is what makes the two sides comparable;
    // without it, every field would look retyped.
    let manifest_layout_fields: Vec<LayoutField<'_>> = fields
        .iter()
        .map(|field| LayoutField {
            name: field.name.as_str(),
            type_tag: manifest_field_tag(field),
            offset: field.offset,
            size: field.size,
        })
        .collect();
    let mut plan = FieldPlan::between(&live_layout_fields, &manifest_layout_fields);
    // Declared defaults fill the fields the diff leaves empty - an added field
    // or one whose type changed. `set_default` is a no-op on a copied field,
    // which is what keeps a changed default from masking a carried value.
    for field in fields {
        let Some(literal) = &field.default else {
            continue;
        };
        let bytes = parse_default_bytes(&field.primitive_type, literal)
            .expect("the manifest default was validated before the plan was built");
        plan.set_default(field.offset, &bytes)
            .expect("the manifest default was validated against the planned field");
    }
    plan
}
