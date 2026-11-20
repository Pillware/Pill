//! The naming rules a generated C# mirror follows for its fields. Both the
//! mirror generator and the binding check use them, so the two agree about
//! what a mirror should contain.
//!
//! # Responsibilities
//!
//! - Turn Rust field names and type tags into the C# declarations a generated
//!   mirror carries: a PascalCase name for each field, one numbered field per
//!   array element, and no public field for a heap container.
//! - Describe a mirror's public fields as one signature text that both sides
//!   can build: the generator from the native descriptors, the bridge from the
//!   managed manifest.
//!
//! # Design
//!
//! This module is compiled into every build. Only a development host generates
//! mirrors, but a shipping host still binds them, and it still has to refuse a
//! managed struct whose fields disagree with the native component. So the
//! naming rules cannot live in the generator alone.

// External crates
use pill_engine::component_registry::ComponentFieldDescriptor;

// =============================================================================
// Free Functions
// =============================================================================

/// Describes the public fields a generated mirror declares for `fields` as the
/// text `Name@offset:size|...`, ordered by offset. Returns `None` when the
/// fields cannot be written in that form.
///
/// This text lets a managed mirror be checked field by field against the
/// native component it binds to. The names and offsets are exactly the ones
/// the generator in `pill_development_host` (`emit_typed_struct`) writes:
/// PascalCase, with one numbered field per array element. A component with no
/// fields, or with a heap-owned field, has no such list: the generator skips a
/// `vec:`/`string` field and gives a `dynbuf:` field three private header
/// words instead of a public field. Such a component is checked by size and
/// alignment only.
pub(super) fn generated_field_signature(fields: &[ComponentFieldDescriptor]) -> Option<String> {
    // Step 1: refuse a component whose fields have no public C# form.
    if fields.is_empty() || fields.iter().any(|field| is_heap_owned_tag(field.type_tag)) {
        return None;
    }
    // Step 2: expand each field into the entries the mirror declares, one per
    // array element, with that element's byte offset.
    let mut declared_fields: Vec<(usize, String, usize)> = Vec::new();
    for field in fields {
        let (_, is_array) = split_array_tag(field.type_tag).ok()?;
        let pascal_case_name = snake_to_pascal(field.name);
        if is_array {
            if field.element_count == 0 || field.size % field.element_count != 0 {
                return None;
            }
            let element_size = field.size / field.element_count;
            for index in 0..field.element_count {
                declared_fields.push((
                    field.offset + index * element_size,
                    format!("{pascal_case_name}{index}"),
                    element_size,
                ));
            }
        } else {
            declared_fields.push((field.offset, pascal_case_name, field.size));
        }
    }
    // Step 3: join the entries into the one text both sides compare.
    Some(field_signature_text(declared_fields))
}

/// Turns `(offset, name, size)` triples into the signature text
/// `Name@offset:size|...`, sorted by offset so both sides produce the same
/// string even when they list the fields in different orders.
pub(super) fn field_signature_text(mut declared_fields: Vec<(usize, String, usize)>) -> String {
    declared_fields.sort();
    declared_fields
        .into_iter()
        .map(|(offset, name, size)| format!("{name}@{offset}:{size}"))
        .collect::<Vec<_>>()
        .join("|")
}

/// Tells whether a type tag names a Rust-owned container field. Such a field
/// gets no C# field of its own, because its pointer must never reach managed
/// code. Managed code reads its elements through the accessor members the
/// generator writes instead.
///
/// This is narrower than [`is_heap_owned_tag`] on purpose. An engine-owned
/// `dynbuf:` buffer is mirrored as three private header words, so the generator
/// must not skip it the way it skips a `vec:`, while managed code still reads
/// its contents through accessors.
pub fn is_opaque_container_tag(type_tag: &str) -> bool {
    type_tag.starts_with("vec:") || type_tag == "string"
}

/// Tells whether a type tag names a field whose bytes are a header for a
/// buffer allocated elsewhere: a `vec:`/`string` container owned by the row,
/// or an engine-owned `dynbuf:` native buffer.
///
/// This repeats the engine's own container rule (`is_container_tag` in
/// `pill_engine_core::component_field`), which is private to that crate. The
/// two copies must agree. [`generated_field_signature`] uses this rule to
/// answer `None` for a component whose public fields cannot be listed at all,
/// and the generator uses it to pick the fields that get accessor members.
pub fn is_heap_owned_tag(type_tag: &str) -> bool {
    type_tag == "string" || type_tag.starts_with("vec:") || type_tag.starts_with("dynbuf:")
}

/// Splits an `array:<inner>` type tag into its base tag and whether the field
/// is an array. A tag without the `array:` prefix comes back unchanged with
/// `false`.
///
/// It never returns `Err`. A tag without the prefix is still a valid answer,
/// and the `Result` lets callers use `?` beside the other type-tag errors,
/// which are `String` too.
pub fn split_array_tag(type_tag: &str) -> Result<(&str, bool), String> {
    match type_tag.strip_prefix("array:") {
        Some(inner) => Ok((inner, true)),
        None => Ok((type_tag, false)),
    }
}

/// Converts a snake_case Rust identifier to the PascalCase name a C# mirror
/// uses: each word starts with a capital letter and the underscores are gone.
pub fn snake_to_pascal(snake_case_name: &str) -> String {
    let mut pascal_case_name = String::with_capacity(snake_case_name.len());
    let mut capitalize_next = true;
    for character in snake_case_name.chars() {
        if character == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            pascal_case_name.extend(character.to_uppercase());
            capitalize_next = false;
        } else {
            pascal_case_name.push(character);
        }
    }
    pascal_case_name
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor(
        name: &'static str,
        type_tag: &'static str,
        offset: usize,
    ) -> ComponentFieldDescriptor {
        ComponentFieldDescriptor {
            name,
            type_tag,
            offset,
            size: 24,
            align: 8,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }
    }

    /// A component with an engine-owned buffer field has no field signature,
    /// so it is checked by size and alignment only. The generator writes three
    /// private header words for that field, so no list of public field names
    /// describes the struct, and a signature built from one would refuse every
    /// such mirror at bind time.
    #[test]
    fn a_native_buffer_field_leaves_no_field_signature() {
        let fields = [
            descriptor("intensity", "f32", 0),
            descriptor("points", "dynbuf:f32", 8),
        ];
        assert_eq!(generated_field_signature(&fields), None);
    }

    #[test]
    fn a_plain_component_still_has_a_field_signature() {
        let fields = [
            descriptor("intensity", "f32", 0),
            descriptor("name_id", "u32", 4),
        ];
        assert_eq!(
            generated_field_signature(&fields).as_deref(),
            Some("Intensity@0:24|NameId@4:24")
        );
    }
}
