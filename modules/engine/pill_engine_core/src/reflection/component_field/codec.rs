//! Scalar and container encode/decode for component field rows.
//!
//! # Responsibilities
//!
//! - Read a row's bytes as a [`FieldValue`](super::FieldValue): a scalar, an
//!   asset or entity reference, a whole fixed array, a heap container read
//!   through its real header, or an opaque copy of bytes the engine cannot
//!   interpret.
//! - Encode a value back into row bytes for every field the generic write path
//!   accepts: scalars, handles, entity references and fixed arrays of scalars.
//!   Heap containers are read-only here and every write to one is refused.
//! - Own the size facts a row is validated against - handle size, entity
//!   reference size, the array element size - so the reads and the writes
//!   agree on them.
//!
//! # Design
//!
//! The standard library does not fix the field order of a `Vec` or a `String`
//! header, so the code measures the offsets once at runtime
//! (`std_container_offsets`) instead of assuming them. The two places that turn
//! those words into a memory access explain in a `// SAFETY:` comment why the
//! access is sound. Rows are native-endian, as the module's row byte convention
//! states, so the scalar paths are plain `from_ne_bytes`/`to_ne_bytes`.

// Standard library
use std::sync::OnceLock;

// Current crate
use super::row::FieldWrite;
use super::{
    is_container_tag, is_entity_tag, is_enum_tag, is_handle_tag, parse_enum_tag,
    ComponentFieldError, FieldValue,
};
use crate::component_registry::ComponentFieldDescriptor;
use crate::entity::Entity;

// =============================================================================
// Constants
// =============================================================================

/// The byte size of a handle: its `u32` index and `u32` generation.
const HANDLE_SIZE: usize = 8;

/// The byte size of an entity reference: the `u64` slot id, the `u32`
/// generation, and the padding `#[repr(C)]` puts after them.
const ENTITY_REFERENCE_SIZE: usize = size_of::<Entity>();

// =============================================================================
// ScalarKind
// =============================================================================

/// The scalar primitives the field vocabulary names.
///
/// There is one entry per tag. The size table, the two codecs and the value
/// labels all ask this type for the tag instead of repeating the strings, so a
/// new scalar is added in one place. The derive emits its own tag text and must
/// name the same tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScalarKind {
    /// A 32-bit float.
    F32,
    /// A 64-bit float.
    F64,
    /// An 8-bit signed integer.
    I8,
    /// A 16-bit signed integer.
    I16,
    /// A 32-bit signed integer.
    I32,
    /// A 64-bit signed integer.
    I64,
    /// An 8-bit unsigned integer.
    U8,
    /// A 16-bit unsigned integer.
    U16,
    /// A 32-bit unsigned integer.
    U32,
    /// A 64-bit unsigned integer.
    U64,
    /// A boolean, one byte.
    Bool,
    /// A pointer-sized unsigned integer.
    Usize,
    /// A pointer-sized signed integer.
    Isize,
}

impl ScalarKind {
    /// The kind a tag names, or `None` when the tag is not a scalar.
    fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "f32" => Self::F32,
            "f64" => Self::F64,
            "i8" => Self::I8,
            "i16" => Self::I16,
            "i32" => Self::I32,
            "i64" => Self::I64,
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "bool" => Self::Bool,
            "usize" => Self::Usize,
            "isize" => Self::Isize,
            _ => return None,
        })
    }

    /// The tag this kind is named by, as the derive emits it.
    pub(super) fn tag(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::I32 => "i32",
            Self::I64 => "i64",
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::Bool => "bool",
            Self::Usize => "usize",
            Self::Isize => "isize",
        }
    }

    /// The size of one value in bytes.
    fn size(self) -> usize {
        match self {
            Self::F32 | Self::I32 | Self::U32 => 4,
            Self::F64 | Self::I64 | Self::U64 => 8,
            Self::I8 | Self::U8 | Self::Bool => 1,
            Self::I16 | Self::U16 => 2,
            // Pointer-sized: 4 bytes on wasm32, 8 on 64-bit targets.
            Self::Usize => size_of::<usize>(),
            Self::Isize => size_of::<isize>(),
        }
    }

    /// Append one value, reporting whether it is of this kind.
    fn encode(self, field_value: &FieldValue, encoded_bytes: &mut Vec<u8>) -> bool {
        match (self, field_value) {
            (Self::F32, FieldValue::F32(value)) => {
                encoded_bytes.extend_from_slice(&value.to_ne_bytes())
            }
            (Self::F64, FieldValue::F64(value)) => {
                encoded_bytes.extend_from_slice(&value.to_ne_bytes())
            }
            (Self::I8, FieldValue::I8(value)) => encoded_bytes.push(*value as u8),
            (Self::I16, FieldValue::I16(value)) => {
                encoded_bytes.extend_from_slice(&value.to_ne_bytes())
            }
            (Self::I32, FieldValue::I32(value)) => {
                encoded_bytes.extend_from_slice(&value.to_ne_bytes())
            }
            (Self::I64, FieldValue::I64(value)) => {
                encoded_bytes.extend_from_slice(&value.to_ne_bytes())
            }
            (Self::U8, FieldValue::U8(value)) => encoded_bytes.push(*value),
            (Self::U16, FieldValue::U16(value)) => {
                encoded_bytes.extend_from_slice(&value.to_ne_bytes())
            }
            (Self::U32, FieldValue::U32(value)) => {
                encoded_bytes.extend_from_slice(&value.to_ne_bytes())
            }
            (Self::U64, FieldValue::U64(value)) => {
                encoded_bytes.extend_from_slice(&value.to_ne_bytes())
            }
            (Self::Bool, FieldValue::Bool(value)) => encoded_bytes.push(u8::from(*value)),
            (Self::Usize, FieldValue::Usize(value)) => {
                encoded_bytes.extend_from_slice(&value.to_ne_bytes())
            }
            (Self::Isize, FieldValue::Isize(value)) => {
                encoded_bytes.extend_from_slice(&value.to_ne_bytes())
            }
            _ => return false,
        }
        true
    }

    /// Decode one value at `offset`, or `None` when the buffer is too short.
    fn decode(self, bytes: &[u8], offset: usize) -> Option<FieldValue> {
        fn read_bytes<const N: usize>(bytes: &[u8], offset: usize) -> Option<[u8; N]> {
            bytes.get(offset..offset + N)?.try_into().ok()
        }
        Some(match self {
            Self::F32 => FieldValue::F32(f32::from_ne_bytes(read_bytes::<4>(bytes, offset)?)),
            Self::F64 => FieldValue::F64(f64::from_ne_bytes(read_bytes::<8>(bytes, offset)?)),
            Self::I8 => FieldValue::I8(*bytes.get(offset)? as i8),
            Self::I16 => FieldValue::I16(i16::from_ne_bytes(read_bytes::<2>(bytes, offset)?)),
            Self::I32 => FieldValue::I32(i32::from_ne_bytes(read_bytes::<4>(bytes, offset)?)),
            Self::I64 => FieldValue::I64(i64::from_ne_bytes(read_bytes::<8>(bytes, offset)?)),
            Self::U8 => FieldValue::U8(*bytes.get(offset)?),
            Self::U16 => FieldValue::U16(u16::from_ne_bytes(read_bytes::<2>(bytes, offset)?)),
            Self::U32 => FieldValue::U32(u32::from_ne_bytes(read_bytes::<4>(bytes, offset)?)),
            Self::U64 => FieldValue::U64(u64::from_ne_bytes(read_bytes::<8>(bytes, offset)?)),
            Self::Bool => FieldValue::Bool(bytes.get(offset).copied()? != 0),
            Self::Usize => FieldValue::Usize(usize::from_ne_bytes(read_bytes::<
                { size_of::<usize>() },
            >(bytes, offset)?)),
            Self::Isize => FieldValue::Isize(isize::from_ne_bytes(read_bytes::<
                { size_of::<isize>() },
            >(bytes, offset)?)),
        })
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The size in bytes of one scalar whose tag is in the closed vocabulary.
pub(super) fn scalar_size(tag: &str) -> Option<usize> {
    ScalarKind::from_tag(tag).map(ScalarKind::size)
}

/// Whether a tag names one of the scalar primitives.
pub(super) fn is_scalar_tag(tag: &str) -> bool {
    ScalarKind::from_tag(tag).is_some()
}

/// Encode one scalar value into `encoded_bytes`, returning whether the tag
/// matched.
pub(super) fn encode_scalar(
    tag: &str,
    field_value: &FieldValue,
    encoded_bytes: &mut Vec<u8>,
) -> bool {
    ScalarKind::from_tag(tag).is_some_and(|kind| kind.encode(field_value, encoded_bytes))
}

/// Decode one scalar from a byte buffer at `offset`.
///
/// Returns `None` when the tag is not a scalar, or when the buffer ends before
/// the value does.
pub(super) fn decode_scalar(bytes: &[u8], offset: usize, tag: &str) -> Option<FieldValue> {
    ScalarKind::from_tag(tag)?.decode(bytes, offset)
}

/// The scalar value of an enum's integer representation that holds
/// `discriminant`, or `None` when the representation is not an integer scalar
/// or the discriminant does not fit it.
pub(super) fn enum_discriminant_value(
    representation: &str,
    discriminant: i64,
) -> Option<FieldValue> {
    Some(match ScalarKind::from_tag(representation)? {
        ScalarKind::I8 => FieldValue::I8(i8::try_from(discriminant).ok()?),
        ScalarKind::I16 => FieldValue::I16(i16::try_from(discriminant).ok()?),
        ScalarKind::I32 => FieldValue::I32(i32::try_from(discriminant).ok()?),
        ScalarKind::I64 => FieldValue::I64(discriminant),
        ScalarKind::U8 => FieldValue::U8(u8::try_from(discriminant).ok()?),
        ScalarKind::U16 => FieldValue::U16(u16::try_from(discriminant).ok()?),
        ScalarKind::U32 => FieldValue::U32(u32::try_from(discriminant).ok()?),
        ScalarKind::U64 => FieldValue::U64(u64::try_from(discriminant).ok()?),
        ScalarKind::Isize => FieldValue::Isize(isize::try_from(discriminant).ok()?),
        ScalarKind::Usize => FieldValue::Usize(usize::try_from(discriminant).ok()?),
        ScalarKind::F32 | ScalarKind::F64 | ScalarKind::Bool => return None,
    })
}

/// The discriminant an integer scalar holds, or `None` for a value that is not
/// an integer or does not fit an `i64`.
fn discriminant_of_scalar(value: &FieldValue) -> Option<i64> {
    match *value {
        FieldValue::I8(value) => Some(i64::from(value)),
        FieldValue::I16(value) => Some(i64::from(value)),
        FieldValue::I32(value) => Some(i64::from(value)),
        FieldValue::I64(value) => Some(value),
        FieldValue::U8(value) => Some(i64::from(value)),
        FieldValue::U16(value) => Some(i64::from(value)),
        FieldValue::U32(value) => Some(i64::from(value)),
        FieldValue::U64(value) => i64::try_from(value).ok(),
        FieldValue::Isize(value) => i64::try_from(value).ok(),
        FieldValue::Usize(value) => i64::try_from(value).ok(),
        _ => None,
    }
}

/// The number of bytes one fixed-array element occupies, from the descriptor.
///
/// Returns `None` when the descriptor declares no elements, so there is no
/// count to divide by.
fn array_element_size(descriptor: &ComponentFieldDescriptor) -> Option<usize> {
    if descriptor.element_count == 0 {
        return None;
    }
    Some(descriptor.size / descriptor.element_count)
}

/// Read one machine word out of a row image.
///
/// Returns `None` when the image ends before the whole word does.
fn header_word(bytes: &[u8], byte_offset: usize) -> Option<usize> {
    Some(usize::from_ne_bytes(
        bytes
            .get(byte_offset..byte_offset + size_of::<usize>())?
            .try_into()
            .ok()?,
    ))
}

/// The byte offsets of the pointer and the length inside a `Vec`/`String`
/// header.
///
/// The standard library gives `Vec`, and `String` with it, no `repr(C)`, so the
/// compiler chooses the field order and it is not pointer-first on this build.
/// The offsets are measured once from a live value rather than assumed: a
/// guessed offset would read the length as an address and dereference it, or
/// read the capacity as the number of elements.
fn std_container_offsets() -> (usize, usize) {
    static HEADER_OFFSETS: OnceLock<(usize, usize)> = OnceLock::new();
    *HEADER_OFFSETS.get_or_init(|| {
        // The pointer, the capacity and the length are all different values,
        // so each word can be identified: the string holds 12 bytes in a
        // 64-byte allocation.
        let mut text = String::with_capacity(64);
        text.push_str("header probe");
        let words = header_words_of(&text);
        let pointer_index = words
            .iter()
            .position(|&word| word == text.as_ptr() as usize)
            .expect("the pointer is one of a string's three words");
        let length_index = words
            .iter()
            .position(|&word| word == text.len())
            .expect("the length is one of a string's three words");
        assert_ne!(
            pointer_index, length_index,
            "a string's pointer and length must occupy different words"
        );
        (
            pointer_index * size_of::<usize>(),
            length_index * size_of::<usize>(),
        )
    })
}

/// The three header words of a `String`, in the order the compiler laid them
/// out.
fn header_words_of(text: &String) -> [usize; 3] {
    assert_eq!(
        size_of::<String>(),
        3 * size_of::<usize>(),
        "a string header is three words; the field layer reads it as one"
    );
    // SAFETY: the assert above confirmed that a `String` is exactly three
    // words, so three words cover the whole value. `transmute_copy` copies
    // those bits out and leaves the string in place.
    unsafe { std::mem::transmute_copy(text) }
}

/// Read the `(pointer, length)` header of a std heap container field.
///
/// `vec:` and `string` tags reach here, and only from a row copied out of a
/// live column. Fixed-array fields do not: their elements are read one by one,
/// with the element size [`array_element_size`] computes from the descriptor.
fn container_header(bytes: &[u8], offset: usize) -> Option<(*const u8, usize)> {
    let (pointer_offset, length_offset) = std_container_offsets();
    let pointer = header_word(bytes, offset + pointer_offset)? as *const u8;
    let length = header_word(bytes, offset + length_offset)?;
    Some((pointer, length))
}

/// Read the `(pointer, length)` header of an engine-owned native buffer field.
///
/// `DynamicBuffer` fixes the word order with `repr(C)`, and the C# mirror reads
/// the same three words straight out of the row. Unlike [`container_header`],
/// this function can name the offsets.
fn native_buffer_header(bytes: &[u8], offset: usize) -> Option<(*const u8, usize)> {
    let pointer = header_word(bytes, offset)? as *const u8;
    let length = header_word(bytes, offset + size_of::<usize>())?;
    Some((pointer, length))
}

/// Decode one field (scalar, whole array, heap container, or opaque) from a
/// row image.
pub(super) fn decode_field(bytes: &[u8], descriptor: &ComponentFieldDescriptor) -> FieldValue {
    // Heap-owning containers are reported, not copied out: a `vec:` or
    // `dynbuf:` field becomes its element count, and a `string` field becomes
    // its text. All three are read-only here; a write goes through the field's
    // generated accessors, which edit the live buffer in place.
    //
    // A `vec:` field and a `dynbuf:` field store their length at different
    // offsets: `vec:` is a std `Vec`, whose word order the compiler chooses
    // (see [`std_container_offsets`]), and `dynbuf:` is a `DynamicBuffer`,
    // whose `repr(C)` order the engine owns.
    if let Some(element_tag) = descriptor.type_tag.strip_prefix("vec:") {
        let element_count = container_header(bytes, descriptor.offset)
            .map(|(_, length)| length)
            .unwrap_or(0);
        return FieldValue::List {
            element_tag,
            element_count,
        };
    }
    if let Some(element_tag) = descriptor.type_tag.strip_prefix("dynbuf:") {
        let element_count = native_buffer_header(bytes, descriptor.offset)
            .map(|(_, length)| length)
            .unwrap_or(0);
        return FieldValue::List {
            element_tag,
            element_count,
        };
    }
    if descriptor.type_tag == "string" {
        let text = match container_header(bytes, descriptor.offset) {
            // An empty `String` keeps a dangling but aligned pointer, so a
            // slice built over it reads zero bytes. A null pointer, which no
            // live `String` holds, is refused here because `from_raw_parts`
            // requires a non-null pointer.
            Some((pointer, length)) if !pointer.is_null() => {
                // SAFETY: the header came from a `String` in a row copied out
                // of a live component column, so the buffer it points at is
                // allocated, holds this many initialized bytes, and is not
                // mutated during this shared borrow.
                let slice = unsafe { std::slice::from_raw_parts(pointer, length) };
                String::from_utf8_lossy(slice).into_owned()
            }
            _ => String::new(),
        };
        return FieldValue::Text(text);
    }
    // The layout's tag is `&'static` (compile-time or leaked runtime string),
    // so the stripped inner tag is `&'static` too.
    let Some(inner_tag) = descriptor.type_tag.strip_prefix("array:") else {
        return decode_tagged(
            bytes,
            descriptor.offset,
            descriptor.type_tag,
            descriptor.size,
        );
    };
    let Some(element_size) = array_element_size(descriptor) else {
        return FieldValue::Array(Vec::new());
    };
    let mut elements = Vec::with_capacity(descriptor.element_count);
    for element_index in 0..descriptor.element_count {
        let element_offset = descriptor.offset + element_index * element_size;
        elements.push(decode_tagged(
            bytes,
            element_offset,
            inner_tag,
            element_size,
        ));
    }
    FieldValue::Array(elements)
}

/// Decode one value of a tag from a row image.
///
/// A scalar tag decodes its scalar. A handle tag of [`HANDLE_SIZE`] bytes
/// decodes an asset reference, and an entity tag of [`ENTITY_REFERENCE_SIZE`]
/// bytes decodes an entity reference. Any other tag is copied as an opaque
/// payload of `field_size` bytes.
///
/// `type_tag` is `'static` because every tag an `Opaque` value can carry comes
/// from a registered field layout (compile-time static data or a `Box::leak`ed
/// runtime string).
fn decode_tagged(
    bytes: &[u8],
    offset: usize,
    type_tag: &'static str,
    field_size: usize,
) -> FieldValue {
    if let Some(value) = decode_scalar(bytes, offset, type_tag) {
        return value;
    }
    // A handle is two `u32`s. The asset type comes from the world's record of
    // the field, which the caller that knows the component fills in.
    if is_handle_tag(type_tag) && field_size == HANDLE_SIZE {
        let index = decode_scalar(bytes, offset, "u32");
        let generation = decode_scalar(bytes, offset + 4, "u32");
        if let (Some(FieldValue::U32(index)), Some(FieldValue::U32(generation))) =
            (index, generation)
        {
            return FieldValue::Asset {
                asset_type: None,
                index,
                generation,
            };
        }
    }
    // An entity reference is a slot id and then a generation, in the handle's
    // `#[repr(C)]` order. The trailing padding is not read.
    if is_entity_tag(type_tag) && field_size == ENTITY_REFERENCE_SIZE {
        let id = decode_scalar(bytes, offset, "u64");
        let generation = decode_scalar(bytes, offset + 8, "u32");
        if let (Some(FieldValue::U64(id)), Some(FieldValue::U32(generation))) = (id, generation) {
            return FieldValue::Entity { id, generation };
        }
    }
    // An enum field is its representation's integer. The value is reported
    // as read even when no variant declares it, so a reader can show what
    // the row holds.
    if let Some(enum_tag) = parse_enum_tag(type_tag) {
        if scalar_size(enum_tag.representation) == Some(field_size) {
            if let Some(discriminant) = decode_scalar(bytes, offset, enum_tag.representation)
                .as_ref()
                .and_then(discriminant_of_scalar)
            {
                return FieldValue::Enum {
                    type_tag,
                    discriminant,
                };
            }
        }
    }
    let mut raw_bytes = vec![0u8; field_size];
    if let Some(source) = bytes.get(offset..offset + field_size) {
        raw_bytes.copy_from_slice(source);
    }
    FieldValue::Opaque {
        type_tag,
        bytes: raw_bytes,
    }
}

/// Encode one field value into the bytes a row write should carry.
///
/// # Errors
///
/// Returns [`ComponentFieldError::UnsupportedField`] for a heap-backed
/// container, for a tag outside the editable vocabulary, or for a fixed array
/// whose elements are not scalars. Returns
/// [`ComponentFieldError::TypeMismatch`] when the value does not match the
/// field's tag, and [`ComponentFieldError::ArrayLengthMismatch`] when an array
/// value has a different number of elements than the descriptor declares.
pub(super) fn encode_field_write(
    descriptor: &ComponentFieldDescriptor,
    field_name: &str,
    field_value: &FieldValue,
) -> Result<FieldWrite, ComponentFieldError> {
    let type_tag = descriptor.type_tag;
    // A heap-backed container is never writable through the generic path. Its
    // buffer is reached through the field's accessors, which edit the live
    // container in place instead of replacing it, so an editor holding a stale
    // image cannot overwrite the real value here. `is_container_tag` is the
    // crate's only test for a heap-backed field, so a container tag added there
    // is refused here too.
    if is_container_tag(type_tag) {
        return Err(ComponentFieldError::UnsupportedField {
            field: field_name.to_string(),
            reason: "heap-backed fields are read-only here; edit them through the field accessors",
        });
    }
    // A captured payload is written back byte for byte when its tag and its
    // size still match the field. A restore relies on this: the editor
    // re-creating a destroyed entity, or re-attaching a removed component,
    // hands back the bytes an earlier read produced. Any other value goes
    // through the editable rules below.
    if let FieldValue::Opaque {
        type_tag: captured_type_tag,
        bytes,
    } = field_value
    {
        if *captured_type_tag == type_tag && bytes.len() == descriptor.size {
            return Ok(FieldWrite {
                row_offset: descriptor.offset,
                bytes: bytes.clone(),
            });
        }
        return Err(ComponentFieldError::TypeMismatch {
            field: field_name.to_string(),
            expected: type_tag,
            found: field_value.variant_tag(),
        });
    }
    // A handle field takes an asset value, written as its index and then its
    // generation in the row's native byte order (see the module's row byte
    // convention). The asset type itself is checked by the caller, which knows
    // the field's record (`World::write_component_field`).
    if let FieldValue::Asset {
        index, generation, ..
    } = field_value
    {
        if !is_handle_tag(type_tag) || descriptor.size != HANDLE_SIZE {
            return Err(ComponentFieldError::TypeMismatch {
                field: field_name.to_string(),
                expected: type_tag,
                found: field_value.variant_tag(),
            });
        }
        let mut encoded_bytes = Vec::with_capacity(HANDLE_SIZE);
        encode_scalar("u32", &FieldValue::U32(*index), &mut encoded_bytes);
        encode_scalar("u32", &FieldValue::U32(*generation), &mut encoded_bytes);
        return Ok(FieldWrite {
            row_offset: descriptor.offset,
            bytes: encoded_bytes,
        });
    }
    // A handle field is editable, but only by an asset value.
    if is_handle_tag(type_tag) {
        return Err(ComponentFieldError::TypeMismatch {
            field: field_name.to_string(),
            expected: type_tag,
            found: field_value.variant_tag(),
        });
    }
    // An entity reference takes an entity value. The row gets the slot id, then
    // the generation, then zeros for the padding a `#[repr(C)]` handle leaves
    // after them, so no row byte carries a stale value.
    if is_entity_tag(type_tag) {
        let FieldValue::Entity { id, generation } = field_value else {
            return Err(ComponentFieldError::TypeMismatch {
                field: field_name.to_string(),
                expected: type_tag,
                found: field_value.variant_tag(),
            });
        };
        if descriptor.size != ENTITY_REFERENCE_SIZE {
            return Err(ComponentFieldError::TypeMismatch {
                field: field_name.to_string(),
                expected: type_tag,
                found: field_value.variant_tag(),
            });
        }
        let mut encoded_bytes = Vec::with_capacity(ENTITY_REFERENCE_SIZE);
        encoded_bytes.extend_from_slice(&id.to_ne_bytes());
        encoded_bytes.extend_from_slice(&generation.to_ne_bytes());
        encoded_bytes.resize(ENTITY_REFERENCE_SIZE, 0);
        return Ok(FieldWrite {
            row_offset: descriptor.offset,
            bytes: encoded_bytes,
        });
    }
    // An enum field takes an enum value carrying the field's own tag, and only
    // a discriminant one of its variants declares: any other integer in a Rust
    // enum's bytes is undefined behaviour for the code that reads it.
    if is_enum_tag(type_tag) {
        let type_mismatch = || ComponentFieldError::TypeMismatch {
            field: field_name.to_string(),
            expected: type_tag,
            found: field_value.variant_tag(),
        };
        let FieldValue::Enum {
            type_tag: value_tag,
            discriminant,
        } = field_value
        else {
            return Err(type_mismatch());
        };
        if *value_tag != type_tag {
            return Err(type_mismatch());
        }
        let enum_tag = parse_enum_tag(type_tag).ok_or(ComponentFieldError::UnsupportedField {
            field: field_name.to_string(),
            reason: "the field's enum tag cannot be read",
        })?;
        if enum_tag.variant_name(*discriminant).is_none() {
            return Err(ComponentFieldError::UnsupportedField {
                field: field_name.to_string(),
                reason: "the value is not one of the enum's variants",
            });
        }
        let representation_value = enum_discriminant_value(enum_tag.representation, *discriminant)
            .ok_or_else(type_mismatch)?;
        let mut encoded_bytes = Vec::with_capacity(descriptor.size);
        encode_scalar(
            enum_tag.representation,
            &representation_value,
            &mut encoded_bytes,
        );
        if encoded_bytes.len() != descriptor.size {
            return Err(type_mismatch());
        }
        return Ok(FieldWrite {
            row_offset: descriptor.offset,
            bytes: encoded_bytes,
        });
    }
    // A `struct:` field, or any other tag the engine cannot interpret, is never
    // writable through the generic path.
    if !is_scalar_tag(type_tag) && !type_tag.starts_with("array:") {
        return Err(ComponentFieldError::UnsupportedField {
            field: field_name.to_string(),
            reason: "the field's type is outside the editable vocabulary",
        });
    }
    if let Some(inner_tag) = type_tag.strip_prefix("array:") {
        let FieldValue::Array(values) = field_value else {
            return Err(ComponentFieldError::TypeMismatch {
                field: field_name.to_string(),
                expected: type_tag,
                found: field_value.variant_tag(),
            });
        };
        if !is_scalar_tag(inner_tag) {
            return Err(ComponentFieldError::UnsupportedField {
                field: field_name.to_string(),
                reason: "array elements are not scalar primitives",
            });
        }
        if values.len() != descriptor.element_count {
            return Err(ComponentFieldError::ArrayLengthMismatch {
                field: field_name.to_string(),
                expected: descriptor.element_count,
                found: values.len(),
            });
        }
        let mut encoded_bytes = Vec::with_capacity(descriptor.size);
        for element in values {
            if !encode_scalar(inner_tag, element, &mut encoded_bytes) {
                return Err(ComponentFieldError::TypeMismatch {
                    field: field_name.to_string(),
                    expected: inner_tag,
                    found: element.variant_tag(),
                });
            }
        }
        return Ok(FieldWrite {
            row_offset: descriptor.offset,
            bytes: encoded_bytes,
        });
    }

    // A scalar field: the value must carry the exact scalar tag.
    let mut encoded_bytes = Vec::new();
    if encode_scalar(type_tag, field_value, &mut encoded_bytes) {
        Ok(FieldWrite {
            row_offset: descriptor.offset,
            bytes: encoded_bytes,
        })
    } else {
        Err(ComponentFieldError::TypeMismatch {
            field: field_name.to_string(),
            expected: type_tag,
            found: field_value.variant_tag(),
        })
    }
}
