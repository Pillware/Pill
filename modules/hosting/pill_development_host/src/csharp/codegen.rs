//! Host-side C# mirror codegen.
//!
//! # Responsibilities
//!
//! - Generate `generated/<module>_Components.g.cs` inside each extension that
//!   exposes a managed surface: its components, its `#[derive(PillMirror)]`
//!   value types, the object classes, enums and resource markers its type rows
//!   declare, and its mirrored free functions. Everything comes from the
//!   descriptors the module itself registered - name, size, alignment and
//!   field layout.
//! - Write the declaration into the module's own folder: nothing in the C#
//!   project is hand-written, so the file derives only from what the Rust
//!   module registered.
//!
//! # Design
//!
//! A component with a registered field layout becomes a `LayoutKind.Explicit`
//! struct whose fields sit at the descriptor's byte offsets; a component with
//! no field metadata becomes an opaque
//! `[StructLayout(LayoutKind.Sequential, Size = N)]` struct whose only field
//! is an alignment pad. Both expose the live bytes as a `Raw` span in safe
//! code. The explicit struct also adds a pad when its fields alone would not
//! give it the module's alignment. When the managed manifest is registered,
//! the host compares the mirror's size and alignment with the module binding,
//! so a generator bug cannot ship a mismatched mirror silently.
//!
//! The file is regenerated on every startup and module reload of a C# project.
//! A committed copy lets standalone C# builds (IDE, `dotnet build`, the C#
//! runtime regression tests) compile without a prior host run. Identical
//! content is left untouched, so a matching module keeps the tree clean.

// Standard library
use std::path::Path;

// External crates
use pill_csharp_bridge::{
    is_heap_owned_tag, is_opaque_container_tag, snake_to_pascal, split_array_tag,
    ModuleExposedComponent, ResolvedFieldAccessor, ResolvedMirrorMethod,
};
use pill_engine::component_registry::{ComponentFieldDescriptor, PillValueTypeDescriptor};

// Current crate
use super::mirror_codegen;

// =============================================================================
// Constants
// =============================================================================

/// C# reserved keywords that cannot name a parameter.
///
/// A Rust argument can legally carry one of these words (`event`, `string`,
/// `object`, ...), because Rust's keyword list is different. Emitting it
/// verbatim would stop the generated project from compiling, so such an
/// argument falls back to a positional `argN` name. That reads better in
/// generated code than `@`-escaping, and it keeps working when C# adds a
/// keyword.
const CSHARP_RESERVED_KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "base",
    "bool",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "checked",
    "class",
    "const",
    "continue",
    "decimal",
    "default",
    "delegate",
    "do",
    "double",
    "else",
    "enum",
    "event",
    "explicit",
    "extern",
    "false",
    "finally",
    "fixed",
    "float",
    "for",
    "foreach",
    "goto",
    "if",
    "implicit",
    "in",
    "int",
    "interface",
    "internal",
    "is",
    "lock",
    "long",
    "namespace",
    "new",
    "null",
    "object",
    "operator",
    "out",
    "override",
    "params",
    "private",
    "protected",
    "public",
    "readonly",
    "ref",
    "return",
    "sbyte",
    "sealed",
    "short",
    "sizeof",
    "stackalloc",
    "static",
    "string",
    "struct",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "uint",
    "ulong",
    "unchecked",
    "unsafe",
    "ushort",
    "using",
    "virtual",
    "void",
    "volatile",
    "while",
];

// =============================================================================
// Types
// =============================================================================

/// One nested C# struct to write out, keyed by the Rust path it came from.
struct EmittedStruct {
    /// Rust path (written with `::`) the definition was generated from.
    qualified: String,
    /// Full C# struct text, without the namespace wrapper.
    definition: String,
}

// =============================================================================
// Free Functions
// =============================================================================

/// The C# primitive used as a pad field to force a struct's alignment.
///
/// A C# struct with no fields has 1-byte alignment, whatever its `Size` says.
/// The generator therefore adds one private pad field of the primitive that
/// matches the module's alignment, so the runtime's layout check, which reads
/// alignment off the fields, reports the registered value.
fn alignment_primitive(align: usize) -> Option<&'static str> {
    match align {
        1 => Some("byte"),
        2 => Some("ushort"),
        4 => Some("uint"),
        8 => Some("ulong"),
        _ => None,
    }
}

/// Splits a C# name such as `pill_spline.Spline` into its namespace and its
/// type name.
fn split_csharp_name(csharp_name: &str) -> Option<(&str, &str)> {
    let dot = csharp_name.rfind('.')?;
    Some((&csharp_name[..dot], &csharp_name[dot + 1..]))
}

/// Returns the C# namespace of a Rust path: every segment before the last,
/// joined with dots (`pill_test::Tint` gives `pill_test`).
///
/// Returns `None` when the path has no `::` separator.
fn path_namespace(qualified: &str) -> Option<String> {
    qualified
        .rfind("::")
        .map(|separator| qualified[..separator].replace("::", "."))
}

/// Generates `<module>_Components.g.cs` in the extension's `generated`
/// directory, covering the surface the module exposes to managed code: its
/// exposed components, its `#[derive(PillMirror)]` value types, the types its
/// type rows declare, and its `#[pill_mirror_fn]` free functions.
///
/// Returns `Ok(true)` when the mirror file was written or removed, and
/// `Ok(false)` when the file on disk already matched what this call would
/// generate. The caller uses the flag to rebuild the C# project only when what
/// C# can see has changed.
///
/// # Errors
///
/// Returns a message when an exposed component's derived name has no
/// namespace; a field layout cannot be expressed in C# (an alignment above 8,
/// a field outside the type, overlapping siblings, an array whose count does
/// not divide its size, an unsupported type tag); a container field has no
/// published accessor or trampoline; two mirrored types collide on one C#
/// name; a declared value type disagrees with the field using it; a declared
/// type or mirrored member cannot be emitted; or a file operation fails.
pub(crate) fn generate_module_components_csharp(
    workspace_root: &Path,
    module_name: &str,
    exposed: &[ModuleExposedComponent],
    value_types: &[PillValueTypeDescriptor],
    methods: &[ResolvedMirrorMethod],
    accessors: &[ResolvedFieldAccessor],
) -> Result<bool, String> {
    generate_components_csharp(
        &crate::config::extension_crate_directory(workspace_root, module_name),
        module_name,
        exposed,
        value_types,
        methods,
        accessors,
    )
}

/// Writes `generated/<module_name>_Components.g.cs` inside `crate_root`.
///
/// This is the implementation behind [`generate_module_components_csharp`]. It
/// takes the directory that holds the module's `generated/` folder directly.
///
/// # Errors
///
/// Returns the same errors as [`generate_module_components_csharp`].
pub(crate) fn generate_components_csharp(
    crate_root: &Path,
    module_name: &str,
    exposed: &[ModuleExposedComponent],
    value_types: &[PillValueTypeDescriptor],
    methods: &[ResolvedMirrorMethod],
    accessors: &[ResolvedFieldAccessor],
) -> Result<bool, String> {
    let generated_dir = crate_root.join("generated");
    let output_path = generated_dir.join(format!("{module_name}_Components.g.cs"));

    // Step 1: when the module exposes nothing to managed code - no components,
    // no value types, no free functions and no declared types - delete the
    // mirror file. A surface that went away must not stay in the C# project,
    // where it would compile against a binding that no longer exists.
    let has_free_functions = methods.iter().any(|method| method.is_free_function);
    let has_declared_types = methods
        .iter()
        .any(|method| method.method_name == pill_engine::component_registry::MIRROR_TYPE_ROW);
    if exposed.is_empty() && value_types.is_empty() && !has_free_functions && !has_declared_types {
        if output_path.exists() {
            std::fs::remove_file(&output_path).map_err(|error| {
                format!("cannot remove stale {}: {error}", output_path.display())
            })?;
            return Ok(true);
        }
        return Ok(false);
    }

    std::fs::create_dir_all(&generated_dir)
        .map_err(|error| format!("cannot create {}: {error}", generated_dir.display()))?;

    // Step 2: write the whole file into `generated_file_text`: the usings, the
    // nested value-type and component namespaces, the declared types, then the
    // free functions.
    let mut generated_file_text = String::new();
    generated_file_text.push_str(
        "// AUTO-GENERATED by the Rust host from this extension's registered metadata.\n\
         // Do not edit: derived from the components, mirrored value types, declared\n\
         // types and mirrored free functions the module itself declares.\n\
         // Written into generated/<module>_Components.g.cs when a C# project\n\
         // starts and whenever one of its modules reloads.\n\
         // Components registered with compile-time field layouts are emitted as\n\
         // typed structs; everything else is an opaque ABI blob with a `Raw` span.\n\
         // Heap-owning fields become accessor members: `Vec`/`String` reach the\n\
         // Rust heap through exported trampolines, and `DynamicBuffer` mirrors\n\
         // its `(ptr, len, cap)` handle so iterating costs no calls at all.\n\
         using System;\n\
         using System.Runtime.CompilerServices;\n\
         using System.Runtime.InteropServices;\n\n",
    );

    // The namespace each value type is written into. It is decided up front so
    // that any generated signature can name a value type in full, from any
    // namespace - an object class taking a `MeshVertex` span, for example.
    let value_namespaces = value_type_namespaces(module_name, exposed, value_types);
    let scope = mirror_codegen::MirrorScope::new(methods, value_types, &|type_name| {
        value_namespaces.get(type_name).cloned()
    })?;

    // Nested `#[derive(PillMirror)]` structs, one per Rust path, added in
    // dependency order so that writing them out in order always compiles. Each
    // is emitted once per file, before the components that use it.
    let mut nested_definitions: Vec<EmittedStruct> = Vec::new();
    let mut component_blocks: Vec<(String, String)> = Vec::new();

    for component in exposed {
        let (namespace, name) = split_csharp_name(&component.csharp_name).ok_or_else(|| {
            format!(
                "derived C# name {} has no namespace separator",
                component.csharp_name
            )
        })?;
        // Components registered by the derive carry a typed field layout; all
        // others get the opaque ABI blob.
        let component_struct_text = if component.fields.is_empty() {
            emit_blob_component(name, component.size, component.align)?
        } else {
            // The C# name is the Rust path with `::` turned into `.`, so
            // turning the dots back recovers the name the accessor rows use and
            // the path that mirrored methods resolve against.
            let rust_type_name = component.csharp_name.replace('.', "::");
            emit_typed_struct(
                name,
                &component.fields,
                component.size,
                component.align,
                value_types,
                &mut nested_definitions,
                methods,
                &rust_type_name,
                accessors,
                &rust_type_name,
                &scope,
            )?
        };
        component_blocks.push((namespace.to_string(), component_struct_text));
    }

    // Write out every value type the module declared, not only the ones a
    // component references, so `#[derive(PillMirror)]` alone exposes a type to
    // C#. Types already in `nested_definitions` are skipped here.
    for value_type in value_types {
        if nested_definitions
            .iter()
            .any(|entry| entry.qualified == value_type.type_name)
        {
            continue;
        }
        let cs_name = last_path_segment(value_type.type_name);
        if let Some(existing) = nested_definitions
            .iter()
            .find(|entry| last_path_segment(&entry.qualified) == cs_name)
        {
            return Err(format!(
                "value types `{}` and `{}` both map to C# name `{cs_name}`",
                existing.qualified, value_type.type_name
            ));
        }
        let definition = emit_typed_struct(
            &cs_name,
            value_type.fields,
            value_type.size,
            value_type.align,
            value_types,
            &mut nested_definitions,
            methods,
            value_type.type_name,
            &[],
            "",
            &scope,
        )?;
        nested_definitions.push(EmittedStruct {
            qualified: value_type.type_name.to_string(),
            definition,
        });
    }

    // Each nested definition goes to the namespace chosen for it above. An
    // opaque stand-in - a type no descriptor declares - goes beside the
    // component that uses it.
    let default_namespace = default_value_namespace(module_name, exposed);
    let nested_namespaces: Vec<String> = nested_definitions
        .iter()
        .map(|definition| {
            value_namespaces
                .get(&definition.qualified)
                .cloned()
                .or_else(|| opaque_namespaces(exposed, &definition.qualified))
                .unwrap_or_else(|| default_namespace.clone())
        })
        .collect();
    for (nested, namespace) in nested_definitions.iter().zip(&nested_namespaces) {
        generated_file_text.push_str(&format!(
            "namespace {namespace} {{\n\n{}\n\n}}\n\n",
            nested.definition
        ));
    }
    // A component in another namespace sees those types through `using`, so a
    // field of one of those types compiles wherever it was declared.
    let mut nested_namespace_set: Vec<&String> = nested_namespaces.iter().collect();
    nested_namespace_set.sort();
    nested_namespace_set.dedup();
    for (namespace, component_struct_text) in &component_blocks {
        let usings: String = nested_namespace_set
            .iter()
            .filter(|nested| **nested != namespace)
            .map(|nested| format!("using {nested};\n"))
            .collect();
        let usings = if usings.is_empty() {
            usings
        } else {
            format!("{usings}\n")
        };
        generated_file_text.push_str(&format!(
            "namespace {namespace} {{\n\n{usings}{body}\n\n}}\n\n",
            body = component_struct_text
        ));
    }

    // The object classes, enums and resource markers declared by the module's
    // type rows, each in its crate's root namespace.
    for declared in &scope.declared {
        generated_file_text.push_str(&mirror_codegen::emit_declared_type(
            declared, methods, &scope,
        )?);
    }

    // `#[pill_mirror_fn]` free functions become one static class per declaring
    // module, written after the struct mirrors. Upstream code sorts the methods
    // by module path and then name, so one scan groups them.
    if has_free_functions {
        let free_functions: Vec<&ResolvedMirrorMethod> = methods
            .iter()
            .filter(|method| method.is_free_function)
            .collect();
        let mut group_start = 0;
        while group_start < free_functions.len() {
            let module_path = free_functions[group_start].type_name.as_str();
            let mut group_end = group_start + 1;
            while group_end < free_functions.len()
                && free_functions[group_end].type_name == module_path
            {
                group_end += 1;
            }
            generated_file_text.push_str(&mirror_codegen::emit_free_function_class(
                module_path,
                &free_functions[group_start..group_end],
                &scope,
            )?);
            group_start = group_end;
        }
    }

    // Step 3: write only when the file is missing or its content differs.
    // "Up to date" means the bytes match, not the timestamps: a module rebuild
    // that leaves the registered layout alone keeps the file and its
    // modification time, so the committed copy stays clean.
    match std::fs::read(&output_path) {
        Ok(existing) if existing == generated_file_text.as_bytes() => return Ok(false),
        _ => {}
    }

    std::fs::write(&output_path, generated_file_text)
        .map_err(|error| format!("cannot write {}: {error}", output_path.display()))?;
    Ok(true)
}

/// Writes the opaque ABI-blob mirror for a component with no field metadata.
///
/// # Errors
///
/// Returns a message when `align` has no matching C# primitive.
fn emit_blob_component(name: &str, size: usize, align: usize) -> Result<String, String> {
    let pad_primitive = alignment_primitive(align)
        .ok_or_else(|| format!("unsupported alignment {align} for exposed component {name}"))?;
    Ok(format!(
        "[StructLayout(LayoutKind.Sequential, Size = {size})]\n\
         public partial struct {name}\n\
         {{\n\
         \x20\x20\x20\x20/// {size}-byte, {align}-byte-aligned ABI mirror of a component\n\
         \x20\x20\x20\x20/// registered by this extension.\n\
         \x20\x20\x20\x20private readonly {align_primitive} _alignmentPad;\n\
         \n\
         \x20\x20\x20\x20/// Live ABI bytes of this component row (safe code).\n\
         \x20\x20\x20\x20public readonly Span<byte> Raw =>\n\
         \x20\x20\x20\x20\x20\x20\x20\x20MemoryMarshal.AsBytes(MemoryMarshal.CreateSpan(ref Unsafe.AsRef(in this), 1));\n\
         }}\n",
        size = size,
        align = align,
        align_primitive = pad_primitive,
    ))
}

/// Writes a typed C# struct from a compile-time field layout.
///
/// `size` and `align` come from the Rust type. An alignment above 8 is
/// rejected because no C# primitive can drive it. `struct:` fields are looked
/// up among the module's registered `#[derive(PillMirror)]` types, recursively;
/// when a type is not declared, an opaque blob of the field's size is used
/// instead. Nested definitions are added to `emitted_structs` in dependency
/// order.
///
/// `owner_type_name` is the component's Rust type name when the struct mirrors
/// a component row, which turns on the heap-field accessor members. It is empty
/// for a `PillMirror` value type. The many parameters are the descriptor fields
/// plus the recursion context; clippy's argument threshold is lower than their
/// count.
///
/// `qualified` is the Rust path that mirrored methods resolve against. A
/// component row passes its own name, so `#[pill_mirror_impl]` methods attach
/// to it just as they attach to a `PillMirror` value type.
///
/// # Errors
///
/// Returns a message when a field has no size or runs past the type's bytes;
/// an array reports zero elements or a size its count does not divide; two
/// emitted fields overlap; a type tag or element alignment has no C# mirror; a
/// container field has no published accessor or trampoline; a nested value
/// type collides on its C# name or disagrees with the field using it; a
/// mirrored member cannot be lowered; or no free range is left for the
/// alignment pad.
#[allow(clippy::too_many_arguments)]
fn emit_typed_struct(
    name: &str,
    fields: &[ComponentFieldDescriptor],
    size: usize,
    align: usize,
    value_types: &[PillValueTypeDescriptor],
    emitted_structs: &mut Vec<EmittedStruct>,
    methods: &[ResolvedMirrorMethod],
    qualified: &str,
    accessors: &[ResolvedFieldAccessor],
    owner_type_name: &str,
    scope: &mirror_codegen::MirrorScope,
) -> Result<String, String> {
    if align > 8 {
        return Err(format!(
            "type `{name}` has alignment {align} > 8; C# mirrors cannot express over-aligned layouts"
        ));
    }

    // Step 1: check the descriptor and turn each field into a C# declaration
    // with its byte offset and element size.
    let mut resolved_fields: Vec<(String, usize, usize)> = Vec::new();
    // The largest alignment the written C# fields force. A container field
    // adds no field, so it adds nothing here; the pad below makes up for that.
    let mut emitted_alignment: usize = 1;
    let mut sorted: Vec<&ComponentFieldDescriptor> = fields.iter().collect();
    sorted.sort_by_key(|field| field.offset);
    for field in &sorted {
        // A Rust-owned container (`Vec`, `String`) gets no C# field: its
        // header is private to the Rust type, and its elements are reached
        // through the accessor members written below.
        if is_opaque_container_tag(field.type_tag) {
            continue;
        }
        if field.size == 0 || field.offset + field.size > size {
            return Err(format!(
                "field `{}` of `{name}` spans bytes {}..{} outside the {size}-byte type",
                field.name,
                field.offset,
                field.offset + field.size
            ));
        }
        // An engine-owned native buffer is mirrored as its three raw words
        // instead of being skipped: `#[repr(C)]` fixes the handle at `(ptr,
        // len, cap)`, so managed code reads the live buffer out of the row and
        // iterates it without calling across the boundary.
        if field.type_tag.starts_with("dynbuf:") {
            let word_bytes = std::mem::size_of::<usize>();
            if field.size != 3 * word_bytes || field.align != word_bytes {
                return Err(format!(
                    "`DynamicBuffer` field `{}` of `{name}` is {}-byte/{}-aligned; the mirror \
                     needs the {}-byte `(ptr, len, cap)` handle of a 64-bit target",
                    field.name,
                    field.size,
                    field.align,
                    3 * word_bytes
                ));
            }
            let pascal = snake_to_pascal(field.name);
            resolved_fields.push((
                format!("private UIntPtr {pascal}Ptr"),
                field.offset,
                word_bytes,
            ));
            resolved_fields.push((
                format!("private UIntPtr {pascal}Len"),
                field.offset + word_bytes,
                word_bytes,
            ));
            resolved_fields.push((
                format!("private UIntPtr {pascal}Cap"),
                field.offset + 2 * word_bytes,
                word_bytes,
            ));
            emitted_alignment = emitted_alignment.max(field.align);
            continue;
        }
        let (base_tag, is_array) = split_array_tag(field.type_tag)?;
        let (element_size, element_align) = if is_array {
            if field.element_count == 0 {
                return Err(format!(
                    "array field `{}` of `{name}` reports zero elements",
                    field.name
                ));
            }
            if field.size % field.element_count != 0 {
                return Err(format!(
                    "array field `{}` of `{name}` has size {} not divisible by {} elements",
                    field.name, field.size, field.element_count
                ));
            }
            (field.size / field.element_count, field.align)
        } else {
            (field.size, field.align)
        };
        if element_align > 8 {
            return Err(format!(
                "field `{}` of `{name}` has element alignment {element_align} > 8; not expressible in a C# mirror",
                field.name
            ));
        }
        emitted_alignment = emitted_alignment.max(element_align);
        let typed_handle = base_tag
            .strip_prefix("struct:Handle<")
            .and_then(|asset| asset.strip_suffix('>'))
            .filter(|_| element_size == 8)
            .and_then(|asset| scope.typed_handle(asset));
        let cs_type = if let Some((cs, _, _)) = cs_primitive(base_tag) {
            cs.to_string()
        } else if base_tag == "entity" && element_size == std::mem::size_of::<pill_engine::Entity>()
        {
            // A reference to another entity. The runtime's own handle type
            // matches the Rust one - same `(id, generation)` layout - so the
            // field gets that type instead of an opaque blob.
            "Entity".to_string()
        } else if let Some(typed_handle) = typed_handle {
            // An asset handle whose asset this module mirrors takes that
            // asset's type, so a C# field holds the `Handle<Mesh>` a mirrored
            // call returned.
            typed_handle
        } else if let Some(qualified) = base_tag.strip_prefix("struct:") {
            // An asset handle whose asset nobody mirrors stays the opaque
            // `Handle`.
            let qualified = if qualified.starts_with("Handle<") {
                "Handle"
            } else {
                qualified
            };
            emit_nested_struct(
                qualified,
                element_size,
                element_align,
                value_types,
                emitted_structs,
                methods,
                scope,
            )?
        } else {
            return Err(format!(
                "unsupported type tag `{}` for field `{}` of `{name}`",
                field.type_tag, field.name
            ));
        };
        let pascal = snake_to_pascal(field.name);
        if is_array {
            for index in 0..field.element_count {
                let offset = field.offset + index * element_size;
                resolved_fields.push((
                    format!("public {cs_type} {pascal}{index}"),
                    offset,
                    element_size,
                ));
            }
        } else {
            resolved_fields.push((
                format!("public {cs_type} {pascal}"),
                field.offset,
                element_size,
            ));
        }
    }

    // Step 2: sibling fields must not overlap. Two C# fields over one byte
    // range would read and write the same storage, so the layout would be
    // wrong rather than merely odd.
    resolved_fields.sort_by_key(|(_, offset, _)| *offset);
    for pair in resolved_fields.windows(2) {
        let (_, left_offset, left_size) = &pair[0];
        let (_, right_offset, _) = &pair[1];
        if *left_offset + *left_size > *right_offset {
            return Err(format!(
                "fields of `{name}` overlap at byte {}; the derive produced an invalid layout",
                right_offset
            ));
        }
    }

    // Step 3: raise the struct's alignment when the fields cannot. C# takes a
    // struct's alignment from its fields, and a container field adds none, so
    // the mirror can fall short of the alignment the module registered - the
    // value the manifest registration checks. A pad field of the matching
    // primitive goes over bytes no field uses (a container's header area or
    // trailing padding), just as the opaque blob mirror pads its layout.
    let mut alignment_pad: Option<(usize, &'static str)> = None;
    if emitted_alignment < align {
        let pad_primitive = alignment_primitive(align).ok_or_else(|| {
            format!("type `{name}` has alignment {align} that no C# primitive can drive")
        })?;
        let mut pad_offset = 0;
        'search: loop {
            let pad_end = pad_offset + align;
            if pad_end > size {
                return Err(format!(
                    "type `{name}` needs an {align}-byte alignment pad but has no free range inside its {size} bytes"
                ));
            }
            for (_, offset, field_size) in &resolved_fields {
                if pad_offset < offset + field_size && *offset < pad_end {
                    pad_offset = offset + field_size;
                    continue 'search;
                }
            }
            break;
        }
        alignment_pad = Some((pad_offset, pad_primitive));
    }

    // Step 4: write the struct. `Size` forces the exact Rust byte size, even
    // with trailing padding; `Raw` keeps the escape hatch to the live bytes.
    let mut struct_text = String::new();
    struct_text.push_str(&format!(
        "[StructLayout(LayoutKind.Explicit, Size = {size})]\npublic partial struct {name}\n{{\n"
    ));
    for (declaration, offset, _) in &resolved_fields {
        struct_text.push_str(&format!("    [FieldOffset({offset})] {declaration};\n"));
    }
    if let Some((pad_offset, pad_primitive)) = alignment_pad {
        struct_text.push_str(&format!(
            "    [FieldOffset({pad_offset})] private readonly {pad_primitive} _alignmentPad;\n"
        ));
    }
    struct_text.push_str(
        "    /// Live ABI bytes of this component row (safe code).\n\
         \x20\x20\x20\x20public readonly Span<byte> Raw =>\n\
         \x20\x20\x20\x20\x20\x20\x20\x20MemoryMarshal.AsBytes(MemoryMarshal.CreateSpan(ref Unsafe.AsRef(in this), 1));\n",
    );
    // Mirrored methods (from `#[pill_mirror_impl]`) become typed C# members
    // that call the Rust implementation through its trampoline: instance
    // methods when they take a receiver, static ones when they do not.
    if !qualified.is_empty() {
        struct_text.push_str(&emit_value_type_methods(qualified, methods, scope)?);
    }
    // Heap-field accessors, for components only: a value type never carries a
    // container field, so an empty owner type means there is nothing to write.
    if !owner_type_name.is_empty() {
        struct_text.push_str(&emit_heap_field_accessors(
            name,
            owner_type_name,
            fields,
            value_types,
            emitted_structs,
            methods,
            accessors,
            scope,
        )?);
    }
    struct_text.push_str("}\n");
    Ok(struct_text)
}

/// Writes the typed C# struct for a `struct:<path>` field tag, once.
///
/// The path is looked up among the module's registered `#[derive(PillMirror)]`
/// types. A declared type is written from its own field layout, recursively.
/// Anything else - an enum, a plain struct that never derived `PillMirror`, a
/// foreign type - becomes an opaque blob of the field's element size and
/// alignment, so the mirror still compiles and keeps the right size.
///
/// # Errors
///
/// Returns a message when two nested types map to one C# name; when a declared
/// value type's size or alignment disagrees with the field using it; or when a
/// recursive definition or the opaque fallback cannot be written.
fn emit_nested_struct(
    qualified: &str,
    size: usize,
    align: usize,
    value_types: &[PillValueTypeDescriptor],
    emitted_structs: &mut Vec<EmittedStruct>,
    methods: &[ResolvedMirrorMethod],
    scope: &mirror_codegen::MirrorScope,
) -> Result<String, String> {
    let cs_name = last_path_segment(qualified);
    if let Some(existing) = emitted_structs
        .iter()
        .find(|entry| entry.qualified == qualified)
    {
        // Already written for an earlier field, so reuse that C# name.
        return Ok(last_path_segment(&existing.qualified));
    }
    if let Some(existing) = emitted_structs
        .iter()
        .find(|entry| last_path_segment(&entry.qualified) == cs_name)
    {
        return Err(format!(
            "nested value types `{}` and `{}` both map to C# name `{cs_name}`; rename one",
            existing.qualified, qualified
        ));
    }
    let definition = match declared_value_type(value_types, qualified) {
        Some(descriptor) => {
            if descriptor.size != size || descriptor.align != align {
                return Err(format!(
                    "value type `{qualified}` is {}-byte/{}-aligned but the field using it is {size}-byte/{align}-aligned; rebuild the module so the mirror matches",
                    descriptor.size, descriptor.align
                ));
            }
            emit_typed_struct(
                &cs_name,
                descriptor.fields,
                descriptor.size,
                descriptor.align,
                value_types,
                emitted_structs,
                methods,
                descriptor.type_name,
                &[],
                "",
                scope,
            )?
        }
        None => emit_opaque_struct(&cs_name, size, align)?,
    };
    emitted_structs.push(EmittedStruct {
        qualified: qualified.to_string(),
        definition,
    });
    Ok(cs_name)
}

/// Writes an opaque blob struct for a nested type with no `PillMirror`
/// descriptor.
///
/// # Errors
///
/// Returns a message when `align` has no matching C# primitive.
fn emit_opaque_struct(name: &str, size: usize, align: usize) -> Result<String, String> {
    let pad_primitive = alignment_primitive(align)
        .ok_or_else(|| format!("unsupported alignment {align} for nested value type `{name}`"))?;
    Ok(format!(
        "[StructLayout(LayoutKind.Sequential, Size = {size})]\n\
         public partial struct {name}\n\
         {{\n\
         \x20\x20\x20\x20private readonly {align_primitive} _alignmentPad;\n\
         \n\
         \x20\x20\x20\x20/// Live ABI bytes of this value type; it was not declared with\n\
         \x20\x20\x20\x20/// `#[derive(PillMirror)]`, so its fields stay opaque.\n\
         \x20\x20\x20\x20public readonly Span<byte> Raw =>\n\
         \x20\x20\x20\x20\x20\x20\x20\x20MemoryMarshal.AsBytes(MemoryMarshal.CreateSpan(ref Unsafe.AsRef(in this), 1));\n\
         }}\n",
        size = size,
        align_primitive = pad_primitive,
    ))
}

/// Returns the parameter name the generated C# uses for one mirrored-method
/// argument.
///
/// Uses the Rust parameter name the macro captured, or a positional `argN` when
/// that name is blank or is a C# reserved keyword.
pub(super) fn csharp_parameter_name(argument_index: usize, rust_name: &str) -> String {
    if !rust_name.is_empty() && !CSHARP_RESERVED_KEYWORDS.contains(&rust_name) {
        rust_name.to_string()
    } else {
        format!("arg{argument_index}")
    }
}

/// Writes the C# members that mirror a value type's (or a component row's)
/// `#[pill_mirror_impl]` functions.
///
/// A `&self` or `&mut self` method becomes an instance member and hands the
/// trampoline the receiver's live address - a component row's column slot, or
/// the local the call was made on - so a `&mut self` method writes through to
/// it. A function with no receiver becomes a static member. Every member calls
/// its trampoline through `TracyLive.MirrorCall`, which does the pointer work,
/// so the generated body compiles without `AllowUnsafeBlocks`.
///
/// Returns an empty string when the type has no mirrored functions.
///
/// # Errors
///
/// Returns a message when one of the type's members cannot be lowered to C#.
fn emit_value_type_methods(
    qualified: &str,
    methods: &[ResolvedMirrorMethod],
    scope: &mirror_codegen::MirrorScope,
) -> Result<String, String> {
    let mut members = String::new();
    if scope.declared_kind(qualified).is_some() {
        // An object's, enum's or resource's functions are written on its own
        // declaration, never on a struct mirror.
        return Ok(members);
    }
    for method in mirror_codegen::members_of(methods, qualified) {
        members.push_str(&mirror_codegen::emit_member(
            method,
            &mirror_codegen::MemberHost::ValueStruct,
            scope,
        )?);
    }
    Ok(members)
}

/// Returns the namespace for a value type that no component references: the
/// first exposed component's namespace, or `pill_<module_name>` when the module
/// exposes no components.
fn default_value_namespace(module_name: &str, exposed: &[ModuleExposedComponent]) -> String {
    exposed
        .first()
        .and_then(|component| split_csharp_name(&component.csharp_name))
        .map(|(namespace, _)| namespace.to_string())
        .unwrap_or_else(|| format!("pill_{module_name}"))
}

/// Returns the namespace of the first component whose fields name `qualified`.
///
/// Used to place an opaque stand-in that no descriptor declares.
fn opaque_namespaces(exposed: &[ModuleExposedComponent], qualified: &str) -> Option<String> {
    exposed
        .iter()
        .find(|component| {
            component.fields.iter().any(|field| {
                split_array_tag(field.type_tag)
                    .ok()
                    .and_then(|(base, _)| base.strip_prefix("struct:"))
                    .is_some_and(|path| {
                        path == qualified || (qualified == "Handle" && path.starts_with("Handle<"))
                    })
            })
        })
        .and_then(|component| split_csharp_name(&component.csharp_name))
        .map(|(namespace, _)| namespace.to_string())
}

/// The declared value type a `struct:<path>` field tag names, or `None` when
/// the module declares no such type.
///
/// The tag is the path as written at the field site: a full path, or a short
/// name an imported `#[derive(PillMirror)]` type brought into scope. The lookup
/// goes through `resolve_unique_name`, the same rule the world uses to expand
/// the field, so a tag that still carries a module path a type used to have
/// finds the type too.
fn declared_value_type<'a>(
    value_types: &'a [PillValueTypeDescriptor],
    path: &str,
) -> Option<&'a PillValueTypeDescriptor> {
    let resolved = pill_engine::component_registry::resolve_unique_name(
        path,
        value_types.iter().map(|descriptor| descriptor.type_name),
    )?;
    value_types
        .iter()
        .find(|descriptor| descriptor.type_name == resolved)
}

/// Works out the namespace of every value type before anything is written.
///
/// A value type goes into the namespace of the first component that references
/// it, directly or through another value type, so C# sees `pill_spline.Vector3f`
/// beside `pill_spline.Spline`. A value type no component references goes into
/// the first component's namespace. When the module exposes no components at
/// all, it goes into the namespace of its own declaring module, so
/// `#[derive(PillMirror)]` alone exposes it where a C# project can `using` it.
fn value_type_namespaces(
    module_name: &str,
    exposed: &[ModuleExposedComponent],
    value_types: &[PillValueTypeDescriptor],
) -> std::collections::HashMap<String, String> {
    fn visit(
        fields: &[ComponentFieldDescriptor],
        namespace: &str,
        value_types: &[PillValueTypeDescriptor],
        assigned: &mut std::collections::HashMap<String, String>,
    ) {
        for field in fields {
            let Ok((base, _)) = split_array_tag(field.type_tag) else {
                continue;
            };
            let Some(path) = base.strip_prefix("struct:") else {
                continue;
            };
            let Some(descriptor) = declared_value_type(value_types, path) else {
                continue;
            };
            if assigned.contains_key(descriptor.type_name) {
                continue;
            }
            assigned.insert(descriptor.type_name.to_string(), namespace.to_string());
            visit(descriptor.fields, namespace, value_types, assigned);
        }
    }

    let mut namespaces_by_type_name = std::collections::HashMap::new();
    for component in exposed {
        if let Some((namespace, _)) = split_csharp_name(&component.csharp_name) {
            visit(
                &component.fields,
                namespace,
                value_types,
                &mut namespaces_by_type_name,
            );
        }
    }
    let default_namespace = default_value_namespace(module_name, exposed);
    for value_type in value_types {
        if namespaces_by_type_name.contains_key(value_type.type_name) {
            continue;
        }
        let namespace = if exposed.is_empty() {
            path_namespace(value_type.type_name).unwrap_or_else(|| default_namespace.clone())
        } else {
            default_namespace.clone()
        };
        namespaces_by_type_name.insert(value_type.type_name.to_string(), namespace);
        visit(
            value_type.fields,
            &namespaces_by_type_name[value_type.type_name].clone(),
            value_types,
            &mut namespaces_by_type_name,
        );
    }
    namespaces_by_type_name
}

/// Writes the C# members that reach a component's heap-owning fields.
///
/// A `Vec<E>` field becomes a count, a read-only span, a writable span and a
/// resize method. A `DynamicBuffer<E>` field becomes a count and two spans
/// that read straight out of the row, plus a resize method. A `Vec<String>`
/// field becomes a count, element get and set, an append and a resize - one
/// boundary call per element, never a span. A `String` field becomes a getter
/// and a setter.
///
/// A member that calls native code passes the row's address to the module's
/// trampoline through a `MirrorMethods.Invoke*` helper, so the buffer is read
/// and written where it lives: one boundary call per member use, none per
/// element. Those calls go through raw C-ABI function pointers, with no
/// delegate stub and no marshalling layer. The trampoline addresses come from
/// a per-struct `EnsureAccessorsBound()` cache keyed on
/// `MirrorMethods.Generation`: a reload maps the declaring module at a fresh
/// base address, so the addresses from the previous bind would dangle. All of
/// it stays safe C#, so the reloadable project assembly needs no
/// `AllowUnsafeBlocks`.
///
/// Read-only members stay on the value type; the ones that call native code
/// address it with `MirrorMethods.AddressOf(ref Unsafe.AsRef(in this))`.
/// Reading through a copy reads the same container header, so a copy is
/// harmless there.
///
/// The members that MUTATE a container header - every `Resize`, `Set` and
/// `Push`, each of which reallocates through the address it is handed - are
/// written on a nested `ref struct {Name}RowRef` instead, bound once to the
/// live row with `Bind(ref row)`. A C# struct is copied on assignment, so a
/// mutator on the value type would reallocate through the copy and leave the
/// live row pointing at the freed block. On the row-ref type that call does
/// not compile, and copying the handle copies only a pointer.
///
/// Every span is a loan, not ownership: resizing or replacing the container
/// makes it invalid, as growing a Rust `Vec` makes its earlier slices invalid.
///
/// The parameters are the descriptor fields plus the recursion context, as in
/// [`emit_typed_struct`]; clippy's threshold is below the (internal)
/// signature's size.
///
/// # Errors
///
/// Returns a message when a container field has no published accessor; when the
/// accessor's kind or element tag disagrees with the field; when a trampoline
/// the generated members call was not published; or when a span's element type
/// cannot be resolved.
#[allow(clippy::too_many_arguments)]
fn emit_heap_field_accessors(
    cs_name: &str,
    type_name: &str,
    fields: &[ComponentFieldDescriptor],
    value_types: &[PillValueTypeDescriptor],
    emitted_structs: &mut Vec<EmittedStruct>,
    methods: &[ResolvedMirrorMethod],
    accessors: &[ResolvedFieldAccessor],
    scope: &mirror_codegen::MirrorScope,
) -> Result<String, String> {
    // Step 1: collect the heap-owning fields, in layout order like the typed
    // fields, so a regenerated mirror keeps the same member order.
    let mut containers: Vec<&ComponentFieldDescriptor> = fields
        .iter()
        .filter(|field| is_heap_owned_tag(field.type_tag))
        .collect();
    if containers.is_empty() {
        return Ok(String::new());
    }
    containers.sort_by_key(|field| field.offset);

    // Trampoline addresses gathered while the fields are written, turned into
    // one `EnsureAccessorsBound()` per struct at the end.
    let mut bound_declarations: Vec<String> = Vec::new();
    let mut bound_assignments: Vec<String> = Vec::new();

    // The first container's PascalCase name, used in the row-ref type's doc
    // example.
    let example_pascal = snake_to_pascal(containers[0].name);

    let mut members = String::new();
    // Header-mutating members, written on the nested row-ref type at the end.
    let mut mutators = String::new();
    // Step 2: write each container's read-only members and record the
    // trampoline slots they call.
    for field in containers {
        let Some(accessor) = accessors
            .iter()
            .find(|accessor| accessor.type_name == type_name && accessor.field_name == field.name)
        else {
            return Err(format!(
                "component `{cs_name}` declares container field `{}` but its module published \
                 no accessor for it; rebuild the module so the derive's trampolines are exported",
                field.name
            ));
        };
        let pascal = snake_to_pascal(field.name);
        let camel = snake_to_camel(field.name);
        let view_operation = crate::csharp::accessor_operation_name(&accessor.field_name, "view");
        // The generated members call these trampolines. A missing one would
        // fail when C# calls it instead of when the project builds, so refuse
        // here.
        let require = |operation: &str, present: bool| -> Result<(), String> {
            if present {
                Ok(())
            } else {
                Err(format!(
                    "field `{}` of `{cs_name}` needs a `{operation}` trampoline but its module \
                     published none; rebuild the module so the derive's accessors are exported",
                    field.name
                ))
            }
        };

        if field.type_tag == "vec:string" {
            if accessor.kind != "vecstring" {
                return Err(format!(
                    "field `{}` of `{cs_name}` is a `Vec<String>` but its module published a `{}` accessor",
                    field.name, accessor.kind
                ));
            }
            require("view", accessor.view_address.is_some())?;
            require("item", accessor.item_address.is_some())?;
            require("set_item", accessor.set_item_address.is_some())?;
            require("push", accessor.push_address.is_some())?;
            require("resize", accessor.resize_address.is_some())?;
            let item_operation =
                crate::csharp::accessor_operation_name(&accessor.field_name, "item");
            let set_item_operation =
                crate::csharp::accessor_operation_name(&accessor.field_name, "set_item");
            let push_operation =
                crate::csharp::accessor_operation_name(&accessor.field_name, "push");
            let resize_operation =
                crate::csharp::accessor_operation_name(&accessor.field_name, "resize");
            let view_address = format!("_{camel}ViewAddress");
            let item_address = format!("_{camel}ItemAddress");
            let set_item_address = format!("_{camel}SetItemAddress");
            let push_address = format!("_{camel}PushAddress");
            let resize_address = format!("_{camel}ResizeAddress");
            bound_declarations.push(format!(
                "    private static global::TracyLive.MirrorTrampoline {view_address};"
            ));
            bound_declarations.push(format!(
                "    private static global::TracyLive.MirrorTrampoline {item_address};"
            ));
            bound_declarations.push(format!(
                "    private static global::TracyLive.MirrorTrampoline {set_item_address};"
            ));
            bound_declarations.push(format!(
                "    private static global::TracyLive.MirrorTrampoline {push_address};"
            ));
            bound_declarations.push(format!(
                "    private static global::TracyLive.MirrorTrampoline {resize_address};"
            ));
            bound_assignments.push(format!(
                "        {view_address} = global::TracyLive.MirrorMethods.Address(\"{type_name}\", \"{view_operation}\");"
            ));
            bound_assignments.push(format!(
                "        {item_address} = global::TracyLive.MirrorMethods.Address(\"{type_name}\", \"{item_operation}\");"
            ));
            bound_assignments.push(format!(
                "        {set_item_address} = global::TracyLive.MirrorMethods.Address(\"{type_name}\", \"{set_item_operation}\");"
            ));
            bound_assignments.push(format!(
                "        {push_address} = global::TracyLive.MirrorMethods.Address(\"{type_name}\", \"{push_operation}\");"
            ));
            bound_assignments.push(format!(
                "        {resize_address} = global::TracyLive.MirrorMethods.Address(\"{type_name}\", \"{resize_operation}\");"
            ));
            members.push_str(&format!(
                r#"
    /// Number of strings in `{field}` (Rust `Vec<String>`).
    public readonly int {pascal}Count
    {{
        get
        {{
            EnsureAccessorsBound();
            global::TracyLive.MirrorMethods.InvokeView({view_address}, global::TracyLive.MirrorMethods.AddressOf(ref Unsafe.AsRef(in this)), out global::TracyLive.BufferView view);
            return view.Length;
        }}
    }}

    /// Element `index` of `{field}`, decoded as UTF-8. Each element is its own
    /// boundary call; an unreachable index throws rather than returning an empty string.
    public readonly string Get{pascal}(int index)
    {{
        EnsureAccessorsBound();
        byte status = global::TracyLive.MirrorMethods.InvokeItem({item_address}, global::TracyLive.MirrorMethods.AddressOf(ref Unsafe.AsRef(in this)), index, out global::TracyLive.BufferView view);
        if (status != 0)
            throw new global::System.ArgumentOutOfRangeException(nameof(index), "the element is not reachable (the row is dead or the index is out of range)");
        return global::TracyLive.ComponentViews.ToUtf8String(view);
    }}
"#,
                field = field.name,
                pascal = pascal,
                view_address = view_address,
                item_address = item_address,
            ));
            mutators.push_str(&format!(
                r#"
        /// Replace element `index` of `{field}` with `value`, encoded as UTF-8.
        /// Mutating member: reach it through `{cs_name}RowRef.Bind`.
        public void Set{pascal}(int index, string value)
        {{
            byte[] utf8 = global::System.Text.Encoding.UTF8.GetBytes(value);
            byte status = global::TracyLive.MirrorMethods.InvokeSetItem({set_item_address}, _row, index, utf8);
            if (status != 0)
                throw new global::System.ArgumentOutOfRangeException(nameof(index), "the element is not reachable (the row is dead or the index is out of range)");
        }}

        /// Append `value` to `{field}` as a new last element.
        /// Mutating member: reach it through `{cs_name}RowRef.Bind`.
        public void Push{pascal}(string value)
        {{
            byte[] utf8 = global::System.Text.Encoding.UTF8.GetBytes(value);
            global::TracyLive.MirrorMethods.InvokeUtf8Write({push_address}, _row, utf8);
        }}

        /// Resize `{field}` to `count` elements; new elements are empty strings.
        /// Mutating member: reach it through `{cs_name}RowRef.Bind`.
        public void Resize{pascal}(int count)
        {{
            global::TracyLive.MirrorMethods.InvokeResize({resize_address}, _row, count);
        }}
"#,
                field = field.name,
                pascal = pascal,
                cs_name = cs_name,
                set_item_address = set_item_address,
                push_address = push_address,
                resize_address = resize_address,
            ));
            continue;
        }

        if let Some(element_tag) = field.type_tag.strip_prefix("dynbuf:") {
            if accessor.kind != "dynbuf" {
                return Err(format!(
                    "field `{}` of `{cs_name}` is a `DynamicBuffer` but its module published a `{}` accessor",
                    field.name, accessor.kind
                ));
            }
            if accessor.element_tag != element_tag {
                return Err(format!(
                    "field `{}` of `{cs_name}` is tagged `{element_tag}` but its module published \
                     element `{}`; rebuild the module so the mirror matches",
                    field.name, accessor.element_tag
                ));
            }
            require("resize", accessor.resize_address.is_some())?;
            let element_type = resolve_span_element_type(
                element_tag,
                cs_name,
                field.name,
                value_types,
                emitted_structs,
                methods,
                scope,
            )?;
            let resize_operation =
                crate::csharp::accessor_operation_name(&accessor.field_name, "resize");
            let resize_address = format!("_{camel}ResizeAddress");
            bound_declarations.push(format!(
                "    private static global::TracyLive.MirrorTrampoline {resize_address};"
            ));
            bound_assignments.push(format!(
                "        {resize_address} = global::TracyLive.MirrorMethods.Address(\"{type_name}\", \"{resize_operation}\");"
            ));
            members.push_str(&format!(
                r#"
    /// Number of live elements in `{field}` (engine-owned `DynamicBuffer<{element_tag}>`);
    /// read straight out of the row, with no boundary call.
    public readonly int {pascal}Count => checked((int){pascal}Len);

    /// Read-only lease over the engine-owned `{field}` buffer; a resize retires the block.
    public readonly ReadOnlySpan<{element_type}> {pascal} =>
        global::TracyLive.ComponentViews.OverDynamicBufferReadOnly<{element_type}>({pascal}Ptr, {pascal}Count);

    /// Writable lease over `{field}`: write elements in place, then resize to grow or shrink.
    public Span<{element_type}> {pascal}Mut =>
        global::TracyLive.ComponentViews.OverDynamicBuffer<{element_type}>({pascal}Ptr, {pascal}Count);
"#,
                field = field.name,
                pascal = pascal,
                element_tag = element_tag,
                element_type = element_type,
            ));
            mutators.push_str(&format!(
                r#"
        /// Resize `{field}` to `count` elements; new elements take the element type's `default`.
        /// Native code reallocates - the elements live in engine-owned memory.
        /// Mutating member: reach it through `{cs_name}RowRef.Bind`.
        public void Resize{pascal}(int count)
        {{
            global::TracyLive.MirrorMethods.InvokeResize({resize_address}, _row, count);
        }}
"#,
                field = field.name,
                pascal = pascal,
                cs_name = cs_name,
                resize_address = resize_address,
            ));
            continue;
        }

        if let Some(element_tag) = field.type_tag.strip_prefix("vec:") {
            if accessor.kind != "vec" {
                return Err(format!(
                    "field `{}` of `{cs_name}` is a `Vec` but its module published a `{}` accessor",
                    field.name, accessor.kind
                ));
            }
            // The layout tag and the accessor's element tag both come from the
            // same derive, so a mismatch means the module and its registered
            // layout disagree. Refuse rather than write a span whose size is a
            // guess.
            if accessor.element_tag != element_tag {
                return Err(format!(
                    "field `{}` of `{cs_name}` is tagged `{element_tag}` but its module published \
                     element `{}`; rebuild the module so the mirror matches",
                    field.name, accessor.element_tag
                ));
            }
            // The span's element type is a primitive, or a
            // `#[derive(PillMirror)]` struct whose descriptor gives both the
            // Rust size and the C# struct that `Span<T>` needs.
            require("view", accessor.view_address.is_some())?;
            require("resize", accessor.resize_address.is_some())?;
            let element_type = resolve_span_element_type(
                element_tag,
                cs_name,
                field.name,
                value_types,
                emitted_structs,
                methods,
                scope,
            )?;
            let resize_operation =
                crate::csharp::accessor_operation_name(&accessor.field_name, "resize");
            let view_address = format!("_{camel}ViewAddress");
            let resize_address = format!("_{camel}ResizeAddress");
            bound_declarations.push(format!(
                "    private static global::TracyLive.MirrorTrampoline {view_address};"
            ));
            bound_declarations.push(format!(
                "    private static global::TracyLive.MirrorTrampoline {resize_address};"
            ));
            bound_assignments.push(format!(
                "        {view_address} = global::TracyLive.MirrorMethods.Address(\"{type_name}\", \"{view_operation}\");"
            ));
            bound_assignments.push(format!(
                "        {resize_address} = global::TracyLive.MirrorMethods.Address(\"{type_name}\", \"{resize_operation}\");"
            ));
            members.push_str(&format!(
                r#"
    /// Number of live elements in `{field}` (Rust `Vec<{element_tag}>`).
    public readonly int {pascal}Count
    {{
        get
        {{
            EnsureAccessorsBound();
            global::TracyLive.MirrorMethods.InvokeView({view_address}, global::TracyLive.MirrorMethods.AddressOf(ref Unsafe.AsRef(in this)), out global::TracyLive.BufferView view);
            return view.Length;
        }}
    }}

    /// Read-only lease over `{field}`; a resize or any structural change invalidates it.
    public readonly ReadOnlySpan<{element_type}> {pascal}
    {{
        get
        {{
            EnsureAccessorsBound();
            global::TracyLive.MirrorMethods.InvokeView({view_address}, global::TracyLive.MirrorMethods.AddressOf(ref Unsafe.AsRef(in this)), out global::TracyLive.BufferView view);
            return global::TracyLive.ComponentViews.AsReadOnlySpan<{element_type}>(view);
        }}
    }}

    /// Writable lease over `{field}`: write elements in place, then resize to grow or shrink.
    public Span<{element_type}> {pascal}Mut
    {{
        get
        {{
            EnsureAccessorsBound();
            global::TracyLive.MirrorMethods.InvokeView({view_address}, global::TracyLive.MirrorMethods.AddressOf(ref Unsafe.AsRef(in this)), out global::TracyLive.BufferView view);
            return global::TracyLive.ComponentViews.AsSpan<{element_type}>(view);
        }}
    }}
"#,
                field = field.name,
                pascal = pascal,
                element_tag = element_tag,
                element_type = element_type,
                view_address = view_address,
            ));
            mutators.push_str(&format!(
                r#"
        /// Resize `{field}` to `count` elements; new elements take the element type's `default`.
        /// Mutating member: reach it through `{cs_name}RowRef.Bind`.
        public void Resize{pascal}(int count)
        {{
            global::TracyLive.MirrorMethods.InvokeResize({resize_address}, _row, count);
        }}
"#,
                field = field.name,
                pascal = pascal,
                cs_name = cs_name,
                resize_address = resize_address,
            ));
            continue;
        }

        if accessor.kind != "string" {
            return Err(format!(
                "field `{}` of `{cs_name}` is a `String` but its module published a `{}` accessor",
                field.name, accessor.kind
            ));
        }
        require("view", accessor.view_address.is_some())?;
        require("set", accessor.set_address.is_some())?;
        let set_operation = crate::csharp::accessor_operation_name(&accessor.field_name, "set");
        let view_address = format!("_{camel}ViewAddress");
        let set_address = format!("_{camel}SetAddress");
        bound_declarations.push(format!(
            "    private static global::TracyLive.MirrorTrampoline {view_address};"
        ));
        bound_declarations.push(format!(
            "    private static global::TracyLive.MirrorTrampoline {set_address};"
        ));
        bound_assignments.push(format!(
            "        {view_address} = global::TracyLive.MirrorMethods.Address(\"{type_name}\", \"{view_operation}\");"
        ));
        bound_assignments.push(format!(
            "        {set_address} = global::TracyLive.MirrorMethods.Address(\"{type_name}\", \"{set_operation}\");"
        ));
        members.push_str(&format!(
            r#"
    /// Decoded UTF-8 text of `{field}` (Rust `String`).
    public readonly string Get{pascal}()
    {{
        EnsureAccessorsBound();
        global::TracyLive.MirrorMethods.InvokeView({view_address}, global::TracyLive.MirrorMethods.AddressOf(ref Unsafe.AsRef(in this)), out global::TracyLive.BufferView view);
        return global::TracyLive.ComponentViews.ToUtf8String(view);
    }}
"#,
            field = field.name,
            pascal = pascal,
            view_address = view_address,
        ));
        mutators.push_str(&format!(
            r#"
        /// Replace `{field}` with `value`, encoded as UTF-8.
        /// Mutating member: reach it through `{cs_name}RowRef.Bind`.
        public void Set{pascal}(string value)
        {{
            byte[] utf8 = global::System.Text.Encoding.UTF8.GetBytes(value);
            global::TracyLive.MirrorMethods.InvokeUtf8Write({set_address}, _row, utf8);
        }}
"#,
            field = field.name,
            pascal = pascal,
            cs_name = cs_name,
            set_address = set_address,
        ));
    }
    // Step 3: write one resolver per struct for all the recorded slots.
    if !bound_assignments.is_empty() {
        members.push_str(&format!(
            r#"
    private static int _accessorsBoundGeneration = -1;
{declarations}

    /// Re-resolve this struct's heap-field trampoline addresses after a host
    /// (re)bind and remember which bind they came from: one int compare per
    /// member use, and no lookup at all while the host stays bound. Shared
    /// with `{cs_name}RowRef`, which checks it once in `Bind`.
    internal static void EnsureAccessorsBound()
    {{
        int generation = global::TracyLive.MirrorMethods.Generation;
        if (_accessorsBoundGeneration == generation)
            return;
{assignments}
        _accessorsBoundGeneration = generation;
    }}
"#,
            cs_name = cs_name,
            declarations = bound_declarations.join("\n"),
            assignments = bound_assignments.join("\n"),
        ));
    }
    // Step 4: write the header-mutating members on a handle that can only
    // name a live row.
    if !mutators.is_empty() {
        members.push_str(&format!(
            r#"
    /// Borrowed handle to one live `{cs_name}` row.
    ///
    /// A C# struct is copied on assignment, so a mutating member on the value
    /// type reallocates through the copy and leaves the live row pointing at
    /// the freed block - `var copy = row.Write<{cs_name}>(); copy.Resize{pascal}(8);`
    /// was exactly that. The mutating members live here instead and address
    /// the row `Bind` captured, and copying this handle copies only the
    /// pointer.
    ///
    /// ```csharp
    /// {cs_name}.{cs_name}RowRef live = {cs_name}.{cs_name}RowRef.Bind(ref row.Write<{cs_name}>());
    /// live.Resize{pascal}(8);
    /// ```
    public ref struct {cs_name}RowRef
    {{
        /// Address of the live row this handle was bound to.
        private global::TracyLive.RowPointer _row;

        /// Bind to the live row `row` refers to.
        ///
        /// `row` must be a `ref` to the row itself, which `row.Write<T>()`
        /// returns; the compiler refuses a copy here. The bind check runs once,
        /// because every member below addresses this same row in this same
        /// invocation.
        public static {cs_name}RowRef Bind(ref {cs_name} row)
        {{
            {cs_name}.EnsureAccessorsBound();
            return new {cs_name}RowRef {{ _row = global::TracyLive.MirrorMethods.AddressOf(ref row) }};
        }}
{mutators}    }}
"#,
            cs_name = cs_name,
            pascal = example_pascal,
            mutators = mutators,
        ));
    }
    Ok(members)
}

/// Returns the C# element type for a generated span to iterate.
///
/// A primitive maps straight across. A `struct:<path>` element must be declared
/// with `#[derive(PillMirror)]`, because that descriptor supplies both the C#
/// struct and the size a span of it needs.
///
/// # Errors
///
/// Returns a message when a `struct:` element carries no `PillMirror`
/// descriptor, when the tag is neither a primitive nor a struct, or when the
/// element's own definition cannot be written.
fn resolve_span_element_type(
    element_tag: &str,
    cs_name: &str,
    field_name: &str,
    value_types: &[PillValueTypeDescriptor],
    emitted: &mut Vec<EmittedStruct>,
    methods: &[ResolvedMirrorMethod],
    scope: &mirror_codegen::MirrorScope,
) -> Result<String, String> {
    if let Some((cs_type, _, _)) = cs_primitive(element_tag) {
        return Ok(cs_type.to_string());
    }
    if let Some(qualified) = element_tag.strip_prefix("struct:") {
        let descriptor = declared_value_type(value_types, qualified).ok_or_else(|| {
            format!(
                "container field `{field_name}` of `{cs_name}` has element type `{qualified}`, \
                 which must derive `PillMirror` so the generated span has a sized C# struct"
            )
        })?;
        return emit_nested_struct(
            qualified,
            descriptor.size,
            descriptor.align,
            value_types,
            emitted,
            methods,
            scope,
        );
    }
    Err(format!(
        "container field `{field_name}` of `{cs_name}` has unsupported element tag `{element_tag}`"
    ))
}

/// Returns the C# type, byte size and alignment for a primitive type tag, or
/// `None` for a tag with no primitive mirror.
fn cs_primitive(type_tag: &str) -> Option<(&'static str, usize, usize)> {
    match type_tag {
        "f32" => Some(("float", 4, 4)),
        "f64" => Some(("double", 8, 8)),
        "i8" => Some(("sbyte", 1, 1)),
        "u8" => Some(("byte", 1, 1)),
        "i16" => Some(("short", 2, 2)),
        "u16" => Some(("ushort", 2, 2)),
        "i32" => Some(("int", 4, 4)),
        "u32" => Some(("uint", 4, 4)),
        "i64" => Some(("long", 8, 8)),
        "u64" => Some(("ulong", 8, 8)),
        // Rust `bool` takes one byte, but C# gives `bool` an ambiguous
        // marshalled size, so the mirror exposes a plain byte.
        "bool" => Some(("byte", 1, 1)),
        "usize" => Some((
            "UIntPtr",
            std::mem::size_of::<usize>(),
            std::mem::align_of::<usize>(),
        )),
        "isize" => Some((
            "IntPtr",
            std::mem::size_of::<isize>(),
            std::mem::align_of::<isize>(),
        )),
        _ => None,
    }
}

/// Returns a snake_case Rust identifier in the camelCase form used for
/// private C# members: `entry_count` becomes `entryCount`.
fn snake_to_camel(snake_case_name: &str) -> String {
    let pascal_name = snake_to_pascal(snake_case_name);
    let mut characters = pascal_name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

/// Returns the last segment of a fully-qualified Rust type name.
fn last_path_segment(qualified_type_name: &str) -> String {
    qualified_type_name
        .rsplit("::")
        .next()
        .unwrap_or(qualified_type_name)
        .to_string()
}

// =============================================================================
// Tests
// =============================================================================

/// Regression tests for the host-side C# mirror generation.
///
/// Each test runs [`generate_module_components_csharp`] against a fresh
/// temporary workspace shaped like the real repository
/// (`<root>/extensions/<module>/<module>/generated/`). They cover the
/// generated text and its namespaces, the alignment pads, mirrored methods
/// and heap-field accessors, the rewrite and removal conditions, per-module
/// file isolation, deterministic output, and the error paths.
#[cfg(test)]
mod tests {
    use super::*;
    use pill_engine::ComponentId;
    use std::path::{Path, PathBuf};

    /// Runs the codegen through the module entry point; tests that exercise
    /// no mirrored methods or accessors pass empty tables.
    fn call_codegen(
        workspace: &Path,
        module_name: &str,
        components: &[ModuleExposedComponent],
        value_types: &[PillValueTypeDescriptor],
    ) -> Result<(), String> {
        generate_module_components_csharp(workspace, module_name, components, value_types, &[], &[])
            .map(|_changed| ())
    }

    /// One resolved mirrored method for codegen tests.
    fn mirrored_method(
        type_name: &str,
        method_name: &str,
        return_tag: &str,
        arg_tags: &[&str],
        arg_names: &[&str],
    ) -> ResolvedMirrorMethod {
        ResolvedMirrorMethod {
            type_name: type_name.to_string(),
            method_name: method_name.to_string(),
            return_tag: return_tag.to_string(),
            arg_tags: arg_tags.iter().map(|tag| tag.to_string()).collect(),
            arg_names: arg_names.iter().map(|name| name.to_string()).collect(),
            address: 0x1234,
            is_free_function: false,
            crate_name: String::new(),
            receiver: "ref".to_string(),
            owner_kind: String::new(),
        }
    }

    /// One resolved mirrored free function (`#[pill_mirror_fn]`) for codegen
    /// tests; `module_path` is the declaring module, not a type.
    fn mirrored_free_function(
        module_path: &str,
        method_name: &str,
        return_tag: &str,
        arg_tags: &[&str],
        arg_names: &[&str],
    ) -> ResolvedMirrorMethod {
        ResolvedMirrorMethod {
            is_free_function: true,
            receiver: String::new(),
            owner_kind: "module".to_string(),
            ..mirrored_method(module_path, method_name, return_tag, arg_tags, arg_names)
        }
    }

    /// Creates a fresh temporary workspace and the module's runtime crate
    /// directory inside it, with the manifest the extension index reads.
    fn temp_workspace(test_name: &str, module_name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("pill_codegen_{}_{}", test_name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let crate_directory = root
            .join("extensions")
            .join(module_name)
            .join("modules")
            .join("runtime");
        std::fs::create_dir_all(&crate_directory).expect("create temp module dir");
        std::fs::write(
            crate_directory.join("Cargo.toml"),
            format!("[package]\nname = \"{module_name}\"\n"),
        )
        .expect("write temp module manifest");
        root
    }

    /// One exposed component with a synthetic stable identity.
    fn exposed(csharp_name: &str, size: usize, align: usize) -> ModuleExposedComponent {
        ModuleExposedComponent {
            csharp_name: csharp_name.to_string(),
            component_id: ComponentId::descriptor(0xABCD_0001_0000_0001),
            size,
            align,
            fields: Vec::new(),
        }
    }

    /// One exposed component carrying a compile-time field layout.
    fn exposed_typed(
        csharp_name: &str,
        size: usize,
        align: usize,
        fields: Vec<ComponentFieldDescriptor>,
    ) -> ModuleExposedComponent {
        ModuleExposedComponent {
            csharp_name: csharp_name.to_string(),
            component_id: ComponentId::descriptor(0xABCD_0001_0000_0001),
            size,
            align,
            fields,
        }
    }

    /// One compile-time field descriptor for a typed-mirror test.
    fn field(
        field_name: &'static str,
        type_tag: &'static str,
        offset: usize,
        size: usize,
        align: usize,
    ) -> ComponentFieldDescriptor {
        ComponentFieldDescriptor {
            name: field_name,
            type_tag,
            offset,
            size,
            align,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }
    }

    /// One compile-time array-field descriptor for a typed-mirror test.
    fn array_field(
        field_name: &'static str,
        type_tag: &'static str,
        offset: usize,
        size: usize,
        align: usize,
        element_count: usize,
    ) -> ComponentFieldDescriptor {
        ComponentFieldDescriptor {
            name: field_name,
            type_tag,
            offset,
            size,
            align,
            element_count,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }
    }

    /// Returns the path of a module's mirror file inside a temporary workspace
    /// shaped like the real repository:
    /// `<root>/extensions/<module>/modules/runtime/generated/`.
    fn generated_path(workspace: &Path, module_name: &str) -> PathBuf {
        workspace
            .join("extensions")
            .join(module_name)
            .join("modules")
            .join("runtime")
            .join("generated")
            .join(format!("{module_name}_Components.g.cs"))
    }

    /// Field layout of a synthetic `pill_core::math::Vector2f` value type.
    static VECTOR2F_FIELDS: &[ComponentFieldDescriptor] = &[
        ComponentFieldDescriptor {
            name: "x",
            type_tag: "f32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "y",
            type_tag: "f32",
            offset: 4,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ];

    /// Returns the text of a module's generated mirror file for assertions.
    fn read_generated(workspace: &Path, module_name: &str) -> String {
        std::fs::read_to_string(generated_path(workspace, module_name))
            .expect("generated file is readable")
    }

    /// The generated mirror must sit inside the extension's own
    /// `generated` directory, not in the C# project.
    #[test]
    fn generated_file_lives_inside_the_module_directory() {
        let workspace = temp_workspace("location", "pill_spline");
        call_codegen(
            &workspace,
            "pill_spline",
            &[exposed("pill_spline.Spline", 196, 4)],
            &[],
        )
        .unwrap();
        assert!(generated_path(&workspace, "pill_spline").exists());
        assert!(generated_path(&workspace, "pill_spline").starts_with(
            workspace
                .join("extensions")
                .join("pill_spline")
                .join("modules")
                .join("runtime")
        ));
    }

    /// A single component produces a struct with the right namespace, name,
    /// size, alignment pad, and the safe raw-bytes accessor.
    #[test]
    fn generates_mirror_for_a_single_component() {
        let workspace = temp_workspace("single", "pill_spline");
        call_codegen(
            &workspace,
            "pill_spline",
            &[exposed("pill_spline.Spline", 196, 4)],
            &[],
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("namespace pill_spline {"));
        assert!(content.contains("public partial struct Spline"));
        assert!(content.contains("Size = 196"));
        assert!(content.contains("private readonly uint _alignmentPad;"));
        assert!(content.contains("public readonly Span<byte> Raw =>"));
        assert!(content.contains("MemoryMarshal.CreateSpan(ref Unsafe.AsRef(in this), 1)"));
    }

    /// Several components from one module go into one file, each in its own
    /// namespace, with its own size and alignment.
    #[test]
    fn generates_multiple_components_in_multiple_namespaces() {
        let workspace = temp_workspace("multi", "pill_spline");
        let components = [
            exposed("pill_spline.Spline", 196, 4),
            exposed("pill_spline.Path", 12, 4),
            exposed("tracy_nav.Segment", 16, 8),
        ];
        call_codegen(&workspace, "pill_spline", &components, &[]).unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("namespace pill_spline {"));
        assert!(content.contains("namespace tracy_nav {"));
        assert!(content.contains("public partial struct Spline"));
        assert!(content.contains("public partial struct Path"));
        assert!(content.contains("public partial struct Segment"));
        assert!(content.contains("Size = 196"));
        assert!(content.contains("Size = 12"));
        assert!(content.contains("Size = 16"));
        // align 8 uses a ulong pad.
        assert!(content.contains("private readonly ulong _alignmentPad;"));
    }

    /// The alignment pad primitive must match each supported alignment, since
    /// C# struct alignment is driven by fields.
    #[test]
    fn alignment_pad_matches_each_supported_alignment() {
        for (align, expected_pad) in [
            (1usize, "byte"),
            (2usize, "ushort"),
            (4usize, "uint"),
            (8usize, "ulong"),
        ] {
            let workspace = temp_workspace(&format!("align_{align}"), "pill_spline");
            call_codegen(
                &workspace,
                "pill_spline",
                &[exposed("pill_spline.Spline", 16, align)],
                &[],
            )
            .unwrap();
            let content = read_generated(&workspace, "pill_spline");
            assert!(
                content.contains(&format!("private readonly {expected_pad} _alignmentPad;")),
                "alignment {align} must use a {expected_pad} pad field"
            );
        }
    }

    /// An alignment with no C# primitive is rejected and writes nothing.
    #[test]
    fn unsupported_alignment_is_rejected() {
        let workspace = temp_workspace("bad_align", "pill_spline");
        let codegen_result = call_codegen(
            &workspace,
            "pill_spline",
            &[exposed("pill_spline.Spline", 16, 3)],
            &[],
        );
        let error = codegen_result.expect_err("alignment 3 is unsupported");
        assert!(error.contains("unsupported alignment"));
        assert!(
            !generated_path(&workspace, "pill_spline").exists(),
            "a rejected generation must not write a file"
        );
    }

    /// A derived C# name without a namespace separator is rejected.
    #[test]
    fn name_without_namespace_is_rejected() {
        let workspace = temp_workspace("no_namespace", "pill_spline");
        let codegen_result =
            call_codegen(&workspace, "pill_spline", &[exposed("Spline", 4, 4)], &[]);
        let error = codegen_result.expect_err("a name without '.' cannot be namespaced");
        assert!(error.contains("no namespace separator"));
    }

    /// A typed component's plain fields each become one C# field at their byte
    /// offset, inside an explicit-layout struct with the Raw accessor.
    #[test]
    fn typed_mirror_emits_explicit_fields() {
        let workspace = temp_workspace("typed", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Physics",
            24,
            4,
            vec![
                field("position_x", "f32", 0, 4, 4),
                field("position_y", "f32", 4, 4, 4),
                field("active", "bool", 8, 1, 1),
                field("score", "u64", 16, 8, 8),
            ],
        );
        call_codegen(&workspace, "pill_spline", &[component], &[]).unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("LayoutKind.Explicit, Size = 24"));
        assert!(content.contains("[FieldOffset(0)] public float PositionX;"));
        assert!(content.contains("[FieldOffset(4)] public float PositionY;"));
        // bool maps to a raw byte in the ABI mirror.
        assert!(content.contains("[FieldOffset(8)] public byte Active;"));
        assert!(content.contains("[FieldOffset(16)] public ulong Score;"));
        assert!(content.contains("public readonly Span<byte> Raw =>"));
    }

    /// An array field is flattened into one C# field per element, with each
    /// element's own byte offset.
    #[test]
    fn typed_mirror_flattens_arrays() {
        let workspace = temp_workspace("array", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Probe",
            12,
            4,
            vec![array_field("samples", "array:f32", 0, 12, 4, 3)],
        );
        call_codegen(&workspace, "pill_spline", &[component], &[]).unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("[FieldOffset(0)] public float Samples0;"));
        assert!(content.contains("[FieldOffset(4)] public float Samples1;"));
        assert!(content.contains("[FieldOffset(8)] public float Samples2;"));
    }

    /// A `struct:` field whose type carries a `PillMirror` descriptor emits a
    /// typed nested struct (recursively) before the component.
    #[test]
    fn typed_mirror_emits_nested_value_types() {
        let workspace = temp_workspace("nested", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Path",
            24,
            4,
            vec![
                field("start", "struct:pill_core::math::Vector2f", 0, 8, 4),
                field("end", "struct:pill_core::math::Vector2f", 8, 8, 4),
                field("kind", "u32", 20, 4, 4),
            ],
        );
        let value_types = [PillValueTypeDescriptor {
            type_name: "pill_core::math::Vector2f",
            crate_name: "pill_core",
            size: 8,
            align: 4,
            fields: VECTOR2F_FIELDS,
        }];
        call_codegen(&workspace, "pill_spline", &[component], &value_types).unwrap();

        let content = read_generated(&workspace, "pill_spline");
        // The nested struct is emitted once, before the component.
        let nested_index = content
            .find("public partial struct Vector2f")
            .expect("nested struct");
        let component_index = content
            .find("public partial struct Path")
            .expect("component struct");
        assert!(
            nested_index < component_index,
            "nested type must precede its user"
        );
        assert!(content.contains("[FieldOffset(0)] public float X;"));
        assert!(content.contains("[FieldOffset(4)] public float Y;"));
        assert!(content.contains("[FieldOffset(0)] public Vector2f Start;"));
        assert!(content.contains("[FieldOffset(8)] public Vector2f End;"));
        assert!(content.contains("[FieldOffset(20)] public uint Kind;"));
    }

    /// A file spanning two namespaces puts a nested type beside the component
    /// that uses it, not beside whichever component came first, and lets the
    /// other namespace see it through `using`.
    #[test]
    fn a_nested_type_lives_in_the_namespace_of_its_first_user() {
        let workspace = temp_workspace("namespaces", "pill_spline");
        let first_component = exposed_typed(
            "pill_engine.common_components.TransformComponent",
            40,
            4,
            vec![field("translation", "f32", 0, 12, 4)],
        );
        let second_component = exposed_typed(
            "pill_demo.component.Path",
            16,
            4,
            vec![
                field("start", "struct:pill_core::math::Vector2f", 0, 8, 4),
                field("end", "struct:pill_core::math::Vector2f", 8, 8, 4),
            ],
        );
        let value_types = [PillValueTypeDescriptor {
            type_name: "pill_core::math::Vector2f",
            crate_name: "pill_core",
            size: 8,
            align: 4,
            fields: VECTOR2F_FIELDS,
        }];
        call_codegen(
            &workspace,
            "pill_spline",
            &[first_component, second_component],
            &value_types,
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains(
            "namespace pill_demo.component {\n\n[StructLayout(LayoutKind.Explicit, Size = 8)]\npublic partial struct Vector2f"
        ));
        assert!(content
            .contains("namespace pill_engine.common_components {\n\nusing pill_demo.component;\n"));
        assert!(!content.contains("namespace pill_demo.component {\n\nusing"));
    }

    /// A `struct:` field with no `PillMirror` descriptor - a foreign type, an
    /// enum, or a struct that never derived the trait - becomes an opaque blob
    /// of the field's size, so the mirror still compiles with the right size.
    #[test]
    fn typed_mirror_falls_back_to_opaque_nested() {
        let workspace = temp_workspace("opaque", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Blob",
            12,
            4,
            vec![field("foreign", "struct:glam::vec3::Vec3", 0, 12, 4)],
        );
        call_codegen(&workspace, "pill_spline", &[component], &[]).unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("public partial struct Vec3"));
        assert!(content.contains("Size = 12"));
        assert!(content.contains("[FieldOffset(0)] public Vec3 Foreign;"));
    }

    /// An array of nested structs resolves each element through the registered
    /// value types and checks the element size against the descriptor.
    #[test]
    fn typed_mirror_handles_arrays_of_nested_structs() {
        let workspace = temp_workspace("array_nested", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Track",
            24,
            4,
            vec![array_field(
                "points",
                "array:struct:pill_core::math::Vector2f",
                0,
                24,
                4,
                3,
            )],
        );
        let value_types = [PillValueTypeDescriptor {
            type_name: "pill_core::math::Vector2f",
            crate_name: "pill_core",
            size: 8,
            align: 4,
            fields: VECTOR2F_FIELDS,
        }];
        call_codegen(&workspace, "pill_spline", &[component], &value_types).unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("[FieldOffset(0)] public Vector2f Points0;"));
        assert!(content.contains("[FieldOffset(8)] public Vector2f Points1;"));
        assert!(content.contains("[FieldOffset(16)] public Vector2f Points2;"));
    }

    /// A nested descriptor whose size disagrees with the referencing field is
    /// a drift bug and must fail loudly rather than emit a wrong mirror.
    #[test]
    fn typed_mirror_rejects_nested_size_mismatch() {
        let workspace = temp_workspace("mismatch", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Path",
            8,
            4,
            vec![field("start", "struct:pill_core::math::Vector2f", 0, 8, 4)],
        );
        let value_types = [PillValueTypeDescriptor {
            type_name: "pill_core::math::Vector2f",
            crate_name: "pill_core",
            size: 12,
            align: 4,
            fields: VECTOR2F_FIELDS,
        }];
        let codegen_result = call_codegen(&workspace, "pill_spline", &[component], &value_types);
        let error = codegen_result.expect_err("a size mismatch must be rejected");
        assert!(error.contains("rebuild the module"));
    }

    /// Value types declared by the module are mirrored even when no component
    /// references them, so `#[derive(PillMirror)]` alone exposes a type to C#.
    #[test]
    fn mirrors_value_types_not_referenced_by_components() {
        let workspace = temp_workspace("standalone_value", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Spline",
            200,
            4,
            vec![field("count", "u32", 0, 4, 4), field("elo", "f32", 4, 4, 4)],
        );
        let value_types = [PillValueTypeDescriptor {
            type_name: "pill_spline::OmoMO",
            crate_name: "pill_spline",
            size: 16,
            align: 8,
            fields: &[
                ComponentFieldDescriptor {
                    name: "x",
                    type_tag: "u64",
                    offset: 0,
                    size: 8,
                    align: 8,
                    element_count: 0,
                    decorators: std::borrow::Cow::Borrowed(&[]),
                },
                ComponentFieldDescriptor {
                    name: "y",
                    type_tag: "u64",
                    offset: 8,
                    size: 8,
                    align: 8,
                    element_count: 0,
                    decorators: std::borrow::Cow::Borrowed(&[]),
                },
            ],
        }];
        call_codegen(&workspace, "pill_spline", &[component], &value_types).unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("public partial struct OmoMO"));
        assert!(content.contains("[FieldOffset(0)] public ulong X;"));
        assert!(content.contains("[FieldOffset(8)] public ulong Y;"));
        assert!(content.contains("public partial struct Spline"));
    }

    /// Overlapping sibling fields (a malformed descriptor) are rejected before
    /// any C# file is written.
    #[test]
    fn typed_mirror_rejects_overlapping_fields() {
        let workspace = temp_workspace("overlap", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Bad",
            8,
            4,
            vec![field("a", "u64", 0, 8, 8), field("b", "u32", 4, 4, 4)],
        );
        let codegen_result = call_codegen(&workspace, "pill_spline", &[component], &[]);
        let error = codegen_result.expect_err("overlapping fields must be rejected");
        assert!(error.contains("overlap"));
    }

    /// A field that runs past the end of the type is a malformed descriptor.
    #[test]
    fn typed_mirror_rejects_out_of_bounds_field() {
        let workspace = temp_workspace("bounds", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Bad",
            4,
            4,
            vec![field("a", "u32", 0, 4, 4), field("b", "u32", 4, 4, 4)],
        );
        let codegen_result = call_codegen(&workspace, "pill_spline", &[component], &[]);
        let error = codegen_result.expect_err("an out-of-bounds field must be rejected");
        assert!(error.contains("outside the"));
    }

    /// Over-aligned layouts cannot be expressed in C# and are rejected with a
    /// message naming the type.
    #[test]
    fn typed_mirror_rejects_over_alignment() {
        let workspace = temp_workspace("over_align", "pill_spline");
        let component = exposed_typed("pill_spline.Big", 32, 16, vec![field("a", "u32", 0, 4, 4)]);
        let codegen_result = call_codegen(&workspace, "pill_spline", &[component], &[]);
        let error = codegen_result.expect_err("alignment 16 is not expressible");
        assert!(error.contains("alignment"));
    }

    /// Regenerating with identical content must not touch the file or its
    /// modification time, so the committed mirror file stays clean.
    #[test]
    fn skips_rewrite_when_content_is_identical() {
        let workspace = temp_workspace("up_to_date", "pill_spline");
        let components = [exposed("pill_spline.Spline", 196, 4)];
        call_codegen(&workspace, "pill_spline", &components, &[]).unwrap();

        let before = std::fs::metadata(generated_path(&workspace, "pill_spline"))
            .unwrap()
            .modified()
            .unwrap();
        call_codegen(&workspace, "pill_spline", &components, &[]).unwrap();
        let after = std::fs::metadata(generated_path(&workspace, "pill_spline"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            before, after,
            "identical content must not rewrite the generated file"
        );
    }

    /// A missing file is regenerated.
    #[test]
    fn writes_when_file_is_missing() {
        let workspace = temp_workspace("missing", "pill_spline");
        let components = [exposed("pill_spline.Spline", 196, 4)];
        call_codegen(&workspace, "pill_spline", &components, &[]).unwrap();
        std::fs::remove_file(generated_path(&workspace, "pill_spline")).unwrap();

        call_codegen(&workspace, "pill_spline", &components, &[]).unwrap();
        assert!(generated_path(&workspace, "pill_spline").exists());
    }

    /// A layout change (size/align) triggers a rewrite with the new values.
    #[test]
    fn rewrites_when_layout_changed() {
        let workspace = temp_workspace("layout_changed", "pill_spline");
        call_codegen(
            &workspace,
            "pill_spline",
            &[exposed("pill_spline.Spline", 196, 4)],
            &[],
        )
        .unwrap();
        assert!(read_generated(&workspace, "pill_spline").contains("Size = 196"));

        call_codegen(
            &workspace,
            "pill_spline",
            &[exposed("pill_spline.Spline", 200, 8)],
            &[],
        )
        .unwrap();
        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("Size = 200"));
        assert!(content.contains("private readonly ulong _alignmentPad;"));
    }

    /// Different modules write separate files, and one module's mirror never
    /// leaks into another's.
    #[test]
    fn writes_separate_files_per_module() {
        let workspace = temp_workspace("per_module", "pill_spline");
        let other_directory = workspace
            .join("extensions")
            .join("other_module")
            .join("modules")
            .join("runtime");
        std::fs::create_dir_all(&other_directory).unwrap();
        std::fs::write(
            other_directory.join("Cargo.toml"),
            "[package]\nname = \"other_module\"\n",
        )
        .unwrap();
        call_codegen(
            &workspace,
            "pill_spline",
            &[exposed("pill_spline.Spline", 196, 4)],
            &[],
        )
        .unwrap();
        call_codegen(
            &workspace,
            "other_module",
            &[exposed("other_module.Gadget", 8, 4)],
            &[],
        )
        .unwrap();

        let spline_file = read_generated(&workspace, "pill_spline");
        let gadget_file = read_generated(&workspace, "other_module");
        assert!(spline_file.contains("Spline"));
        assert!(!spline_file.contains("Gadget"));
        assert!(gadget_file.contains("Gadget"));
        assert!(!gadget_file.contains("Spline"));
    }

    /// A module that stops exposing components removes its stale mirror file
    /// so a vanished binding cannot linger in the C# project.
    #[test]
    fn empty_exposure_removes_stale_file() {
        let workspace = temp_workspace("emptied", "pill_spline");
        call_codegen(
            &workspace,
            "pill_spline",
            &[exposed("pill_spline.Spline", 196, 4)],
            &[],
        )
        .unwrap();
        assert!(generated_path(&workspace, "pill_spline").exists());

        call_codegen(&workspace, "pill_spline", &[], &[]).unwrap();
        assert!(
            !generated_path(&workspace, "pill_spline").exists(),
            "a module exposing nothing must not keep a stale mirror"
        );
    }

    /// The same input produces byte-identical output on two fresh workspaces,
    /// so regenerating never changes the committed mirror file.
    #[test]
    fn output_is_deterministic() {
        let first_workspace = temp_workspace("deterministic_a", "pill_spline");
        let second_workspace = temp_workspace("deterministic_b", "pill_spline");
        let components = [
            exposed("pill_spline.Spline", 196, 4),
            exposed("pill_spline.Path", 12, 4),
        ];
        call_codegen(&first_workspace, "pill_spline", &components, &[]).unwrap();
        call_codegen(&second_workspace, "pill_spline", &components, &[]).unwrap();

        assert_eq!(
            read_generated(&first_workspace, "pill_spline"),
            read_generated(&second_workspace, "pill_spline")
        );
    }

    /// Fields of the `Tint` value type used by the value-type-only test (three
    /// f32 channels).
    static TINT_FIELDS: &[ComponentFieldDescriptor] = &[
        ComponentFieldDescriptor {
            name: "r",
            type_tag: "f32",
            offset: 0,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "g",
            type_tag: "f32",
            offset: 4,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
        ComponentFieldDescriptor {
            name: "b",
            type_tag: "f32",
            offset: 8,
            size: 4,
            align: 4,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        },
    ];

    /// A module that declares only `#[derive(PillMirror)]` value types gets a
    /// mirror file for them, each in its declaring crate's namespace, even
    /// though it exposes no component.
    #[test]
    fn a_module_with_only_value_types_emits_them_in_their_crate_namespace() {
        let workspace = temp_workspace("value_only", "pill_test");
        let value_types = [PillValueTypeDescriptor {
            type_name: "pill_test::Tint",
            crate_name: "pill_test",
            size: 12,
            align: 4,
            fields: TINT_FIELDS,
        }];

        generate_module_components_csharp(&workspace, "pill_test", &[], &value_types, &[], &[])
            .unwrap();

        let content = read_generated(&workspace, "pill_test");
        assert!(content.contains("namespace pill_test {"));
        assert!(content.contains("public partial struct Tint"));
        assert!(content.contains("Size = 12"));
        assert!(content.contains("[FieldOffset(0)] public float R;"));
        assert!(content.contains("[FieldOffset(8)] public float B;"));
    }

    /// `#[pill_mirror_fn]` free functions become one static class named after
    /// the module that declares them, in that module's namespace. No component
    /// or value type has to exist for the file to be generated.
    #[test]
    fn free_functions_emit_a_static_class_in_the_module_namespace() {
        let workspace = temp_workspace("free_fn", "pill_test");
        let methods = [
            mirrored_free_function("pill_test", "tint_alpha", "f32", &[], &[]),
            mirrored_free_function(
                "pill_test",
                "blend",
                "f32",
                &["f32", "f32"],
                &["alpha", "beta"],
            ),
        ];

        generate_module_components_csharp(&workspace, "pill_test", &[], &[], &methods, &[])
            .unwrap();

        let content = read_generated(&workspace, "pill_test");
        assert!(content.contains("namespace pill_test {"));
        assert!(content.contains("public static class PillTest"));
        assert!(content.contains("public static float TintAlpha()"));
        assert!(content.contains(
            "var __call = global::TracyLive.MirrorCall.Begin(\"pill_test\", \"tint_alpha\");"
        ));
        assert!(content.contains("return __call.Result<float>();"));
        // Argument names survive, and every argument is pushed in order; no
        // receiver is pushed for a free function.
        assert!(content.contains("public static float Blend(float alpha, float beta)"));
        assert!(content.contains("__call.Push(alpha);\n        __call.Push(beta);"));
        assert!(!content.contains("PushAddress"));
    }

    /// A free function declared in a submodule gets its own class in the
    /// crate's namespace.
    #[test]
    fn a_submodule_free_function_names_its_class_after_the_submodule() {
        let workspace = temp_workspace("free_fn_nested", "pill_test");
        let methods = [mirrored_free_function(
            "pill_test::utils",
            "grade",
            "u8",
            &[],
            &[],
        )];

        generate_module_components_csharp(&workspace, "pill_test", &[], &[], &methods, &[])
            .unwrap();

        let content = read_generated(&workspace, "pill_test");
        assert!(content.contains("namespace pill_test {"));
        assert!(content.contains("public static class Utils"));
        assert!(content.contains("public static byte Grade()"));
    }

    /// A mirrored method with a receiver becomes a typed C# instance method
    /// that calls the module's trampoline through `MirrorCall` and hands it the
    /// receiver's live address. A type with no mirrored methods gets no method
    /// block.
    #[test]
    fn mirrored_methods_are_emitted_on_value_types() {
        static FIELDS: &[ComponentFieldDescriptor] = &[
            ComponentFieldDescriptor {
                name: "x",
                type_tag: "u64",
                offset: 0,
                size: 8,
                align: 8,
                element_count: 0,
                decorators: std::borrow::Cow::Borrowed(&[]),
            },
            ComponentFieldDescriptor {
                name: "y",
                type_tag: "u64",
                offset: 8,
                size: 8,
                align: 8,
                element_count: 0,
                decorators: std::borrow::Cow::Borrowed(&[]),
            },
        ];
        let workspace = temp_workspace("mirror_methods", "pill_spline");
        let value_types = [PillValueTypeDescriptor {
            type_name: "pill_spline::OmoMO",
            crate_name: "pill_spline",
            size: 16,
            align: 8,
            fields: FIELDS,
        }];
        let methods = [
            mirrored_method("pill_spline::OmoMO", "get_sum", "u64", &[], &[]),
            mirrored_method(
                "pill_spline::OmoMO",
                "add",
                "u32",
                &["u32", "f32"],
                &["amount", "scale"],
            ),
            mirrored_method("pill_spline::Other", "unused", "", &[], &[]),
        ];
        generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[exposed("pill_spline.Spline", 200, 4)],
            &value_types,
            &methods,
            &[],
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        // Instance method, PascalCased, calling the trampoline by Rust names.
        assert!(content.contains("public ulong GetSum()"));
        assert!(content.contains(
            "var __call = global::TracyLive.MirrorCall.Begin(\"pill_spline::OmoMO\", \"get_sum\");"
        ));
        // Argument-carrying method: the Rust parameter names survive, the
        // receiver goes first as its live address, then each argument.
        assert!(content.contains("public uint Add(uint amount, float scale)"));
        assert!(content.contains(
            "__call.PushAddress(ref global::System.Runtime.CompilerServices.Unsafe.AsRef(in this));\n        __call.Push(amount);\n        __call.Push(scale);"
        ));
        assert!(content.contains("return __call.Result<uint>();"));
        // The receiver is addressed through the runtime helper and nothing is
        // boxed or pinned, so a mirrored call allocates nothing.
        assert!(!content.contains("boxed"));
        assert!(!content.contains("GCHandle"));
        // Methods of a type that is never emitted must not appear.
        assert!(!content.contains("Unused"));
    }

    /// Mirrored methods on a component row are written the same way as
    /// value-type methods, so a C# project can call a module's math on the
    /// component instead of reimplementing it.
    #[test]
    fn mirrored_methods_are_emitted_on_component_rows() {
        let workspace = temp_workspace("mirror_component", "pill_spline");
        let methods = [mirrored_method(
            "pill_spline::Spline",
            "get_location_x",
            "f32",
            &["f32"],
            &["t"],
        )];
        generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[exposed_typed(
                "pill_spline.Spline",
                4,
                4,
                vec![field("elo", "f32", 0, 4, 4)],
            )],
            &[],
            &methods,
            &[],
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("public float GetLocationX(float t)"));
        assert!(content.contains(
            "__call.PushAddress(ref global::System.Runtime.CompilerServices.Unsafe.AsRef(in this));\n        __call.Push(t);"
        ));
    }

    /// A void-returning mirrored method emits a `void` member whose call
    /// reads no result.
    #[test]
    fn mirrored_method_void_return_is_emitted() {
        static FIELDS: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
            name: "x",
            type_tag: "u64",
            offset: 0,
            size: 8,
            align: 8,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }];
        let workspace = temp_workspace("mirror_method_void", "pill_spline");
        let value_types = [PillValueTypeDescriptor {
            type_name: "pill_spline::OmoMO",
            crate_name: "pill_spline",
            size: 16,
            align: 8,
            fields: FIELDS,
        }];
        let methods = [mirrored_method(
            "pill_spline::OmoMO",
            "reset",
            "",
            &["u64"],
            &["countdown"],
        )];
        generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[exposed("pill_spline.Spline", 200, 4)],
            &value_types,
            &methods,
            &[],
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("public void Reset(ulong countdown)"));
        assert!(content.contains("__call.Push(countdown);\n        __call.Invoke();\n    }"));
    }

    /// A Rust parameter name that is a C# reserved keyword (or is absent) must
    /// not be emitted verbatim - the generated project would not compile.
    #[test]
    fn csharp_parameter_names_are_keyword_safe() {
        assert_eq!(csharp_parameter_name(0, "alpha"), "alpha");
        assert_eq!(csharp_parameter_name(2, "event"), "arg2");
        assert_eq!(csharp_parameter_name(1, "string"), "arg1");
        assert_eq!(csharp_parameter_name(0, "ref"), "arg0");
        assert_eq!(csharp_parameter_name(1, ""), "arg1");
    }

    /// The codegen reports whether the mirror file on disk changed, so the
    /// reload path queues a C# rebuild only when what C# can see changed. A
    /// matching re-run reports no change; adding a method or running on a
    /// fresh workspace reports a change.
    #[test]
    fn codegen_reports_whether_the_mirror_changed() {
        static FIELDS: &[ComponentFieldDescriptor] = &[ComponentFieldDescriptor {
            name: "x",
            type_tag: "u64",
            offset: 0,
            size: 8,
            align: 8,
            element_count: 0,
            decorators: std::borrow::Cow::Borrowed(&[]),
        }];
        let workspace = temp_workspace("mirror_changed_flag", "pill_spline");
        let component = exposed("pill_spline.OmoMO", 16, 8);
        let value_types = [PillValueTypeDescriptor {
            type_name: "pill_spline::OmoMO",
            crate_name: "pill_spline",
            size: 16,
            align: 8,
            fields: FIELDS,
        }];
        let methods_with_one_member = [mirrored_method(
            "pill_spline::OmoMO",
            "get_sum",
            "u64",
            &[],
            &[],
        )];
        let methods_with_two_members = [
            mirrored_method("pill_spline::OmoMO", "get_sum", "u64", &[], &[]),
            mirrored_method("pill_spline::OmoMO", "get_b", "u64", &[], &[]),
        ];

        // First write on a fresh workspace always reports a change.
        assert!(generate_module_components_csharp(
            &workspace,
            "pill_spline",
            std::slice::from_ref(&component),
            &value_types,
            &methods_with_one_member,
            &[],
        )
        .unwrap());
        // Identical regeneration reports no change.
        assert!(!generate_module_components_csharp(
            &workspace,
            "pill_spline",
            std::slice::from_ref(&component),
            &value_types,
            &methods_with_one_member,
            &[],
        )
        .unwrap());
        // Adding a mirrored method changes the content, so it reports a change.
        assert!(generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[component],
            &value_types,
            &methods_with_two_members,
            &[],
        )
        .unwrap());
        assert!(read_generated(&workspace, "pill_spline").contains("public ulong GetB()"));
    }

    /// One resolved heap-field accessor for codegen tests; every kind
    /// publishes a view, and the remaining trampolines are present exactly
    /// where the kind has them.
    fn resolved_accessor(
        type_name: &str,
        field_name: &str,
        kind: &str,
        element_tag: &str,
    ) -> ResolvedFieldAccessor {
        ResolvedFieldAccessor {
            type_name: type_name.to_string(),
            field_name: field_name.to_string(),
            kind: kind.to_string(),
            element_tag: element_tag.to_string(),
            view_address: Some(0x2000),
            resize_address: (kind == "vec" || kind == "dynbuf" || kind == "vecstring")
                .then_some(0x2001),
            set_address: (kind == "string").then_some(0x2002),
            item_address: (kind == "vecstring").then_some(0x2003),
            set_item_address: (kind == "vecstring").then_some(0x2004),
            push_address: (kind == "vecstring").then_some(0x2005),
        }
    }

    /// A `vec:f32` field emits count, two spans and a resize member. Each one
    /// resolves the module's accessor trampoline and hands it the live row
    /// address. The container header itself never becomes a C# field.
    #[test]
    fn heap_vec_field_emits_span_accessors() {
        let workspace = temp_workspace("heap_vec", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Trail",
            32,
            8,
            vec![
                field("points", "vec:f32", 0, 24, 8),
                field("kind", "u32", 24, 4, 4),
            ],
        );
        let accessors = [resolved_accessor(
            "pill_spline::Trail",
            "points",
            "vec",
            "f32",
        )];
        generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[component],
            &[],
            &[],
            &accessors,
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        // The heap field occupies no C# field; the blittable ones still do.
        assert!(!content.contains("public float Points"));
        assert!(content.contains("[FieldOffset(24)] public uint Kind;"));
        // A container contributes no C# field, so the pad keeps the struct's
        // reflected alignment at the module's declared 8 bytes.
        assert!(content.contains("[FieldOffset(0)] private readonly ulong _alignmentPad;"));
        assert!(content.contains("public readonly int PointsCount"));
        assert!(content.contains("public readonly ReadOnlySpan<float> Points"));
        assert!(content.contains("public Span<float> PointsMut"));
        assert!(content.contains("public void ResizePoints(int count)"));
        // The addresses come from the shared table under the operation names
        // the host registers, resolved once per bind; the row address still
        // comes from the live struct.
        assert!(content.contains(
            "_pointsViewAddress = global::TracyLive.MirrorMethods.Address(\"pill_spline::Trail\", \"points_view\");"
        ));
        assert!(content.contains(
            "_pointsResizeAddress = global::TracyLive.MirrorMethods.Address(\"pill_spline::Trail\", \"points_resize\");"
        ));
        assert!(content.contains(
            "global::TracyLive.MirrorMethods.InvokeView(_pointsViewAddress, global::TracyLive.MirrorMethods.AddressOf(ref Unsafe.AsRef(in this)), out global::TracyLive.BufferView view);"
        ));
        // The member path uses raw function pointers, so there are no delegate
        // types and no marshalling layer, and a bind check costs one int compare.
        assert!(!content.contains("UnmanagedFunctionPointer"));
        assert!(!content.contains("Resolve<"));
        assert!(content.contains("internal static void EnsureAccessorsBound()"));
        assert!(content.contains("if (_accessorsBoundGeneration == generation)"));
        assert!(content
            .contains("global::TracyLive.MirrorMethods.AddressOf(ref Unsafe.AsRef(in this))"));
        assert!(content.contains("global::TracyLive.ComponentViews.AsSpan<float>(view)"));
        // The header-mutating member lives on the row-ref handle and addresses
        // the bound row, never the struct the caller is holding.
        assert!(content.contains("public ref struct TrailRowRef"));
        assert!(content.contains("public static TrailRowRef Bind(ref Trail row)"));
        assert!(content.contains(
            "global::TracyLive.MirrorMethods.InvokeResize(_pointsResizeAddress, _row, count);"
        ));
        assert!(
            !content.contains(
                "InvokeResize(_pointsResizeAddress, global::TracyLive.MirrorMethods.AddressOf"
            ),
            "a resize must never reallocate through a struct the caller may have copied"
        );
    }

    /// A struct with several container fields resolves every trampoline
    /// address in one bind check. The generated resolver is a single method
    /// per struct, not one per field.
    #[test]
    fn heap_fields_share_one_bind_resolver_per_struct() {
        let workspace = temp_workspace("heap_multi_field", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Loadout",
            64,
            8,
            vec![
                field("points", "vec:f32", 0, 24, 8),
                field("name", "string", 24, 24, 8),
                field("weight", "f32", 48, 4, 4),
            ],
        );
        let accessors = [
            resolved_accessor("pill_spline::Loadout", "points", "vec", "f32"),
            resolved_accessor("pill_spline::Loadout", "name", "string", ""),
        ];
        generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[component],
            &[],
            &[],
            &accessors,
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert_eq!(
            content
                .matches("internal static void EnsureAccessorsBound()")
                .count(),
            1
        );
        // Both container kinds' mutators share the single `Bind` of the one
        // row-ref handle the struct emits.
        assert_eq!(
            content
                .matches("public static LoadoutRowRef Bind(ref Loadout row)")
                .count(),
            1
        );
        // Each field keeps its own address slots, so the shared resolver
        // cannot mix up two containers of the same kind.
        assert!(content
            .contains("private static global::TracyLive.MirrorTrampoline _pointsViewAddress;"));
        assert!(
            content.contains("private static global::TracyLive.MirrorTrampoline _nameViewAddress;")
        );
        assert!(
            content.contains("private static global::TracyLive.MirrorTrampoline _nameSetAddress;")
        );
        assert!(content
            .contains("private static global::TracyLive.MirrorTrampoline _pointsResizeAddress;"));
    }

    /// A `string` field emits a UTF-8 getter and setter; the setter hands
    /// its encoded bytes across as a span rather than pinning them itself.
    #[test]
    fn heap_string_field_emits_text_accessors() {
        let workspace = temp_workspace("heap_string", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Label",
            32,
            8,
            vec![
                field("name", "string", 0, 24, 8),
                field("weight", "f32", 24, 4, 4),
            ],
        );
        let accessors = [resolved_accessor(
            "pill_spline::Label",
            "name",
            "string",
            "",
        )];
        generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[component],
            &[],
            &[],
            &accessors,
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("[FieldOffset(0)] private readonly ulong _alignmentPad;"));
        assert!(content.contains("public readonly string GetName()"));
        assert!(content.contains("public void SetName(string value)"));
        assert!(content.contains("global::TracyLive.ComponentViews.ToUtf8String(view)"));
        // The payload crosses as a span; the runtime helper pins it with
        // `fixed` for the call, so no generated GCHandle remains to leak.
        assert!(!content.contains("GCHandle"));
        assert!(content.contains(
            "global::TracyLive.MirrorMethods.InvokeUtf8Write(_nameSetAddress, _row, utf8);"
        ));
        assert!(content.contains(
            "_nameSetAddress = global::TracyLive.MirrorMethods.Address(\"pill_spline::Label\", \"name_set\");"
        ));
        assert!(
            content.contains("global::TracyLive.MirrorMethods.InvokeUtf8Write(_nameSetAddress,")
        );
        // The setter changes the container header, so it lives on the row-ref
        // handle and writes through the bound row's address, not the caller's
        // struct.
        assert!(content.contains("public ref struct LabelRowRef"));
        assert!(content.contains("public static LabelRowRef Bind(ref Label row)"));
        assert!(content
            .contains("global::TracyLive.MirrorMethods.InvokeUtf8Write(_nameSetAddress, _row,"));
        assert!(
            !content.contains(
                "InvokeUtf8Write(_nameSetAddress, global::TracyLive.MirrorMethods.AddressOf"
            ),
            "a setter must never write through a struct the caller may have copied"
        );
    }

    /// Header-mutating members exist only on the row-ref handle, and the
    /// handle binds to a live row with `Bind(ref ...)`.
    ///
    /// A mutator on the value type would reallocate through whatever struct
    /// the caller holds - `var copy = row.Write<Trail>(); copy.ResizePoints(8);`
    /// and leave the live row pointing at the freed block. `Bind` takes a
    /// `ref`, so the compiler refuses a copy, and copying the handle copies
    /// only a pointer.
    #[test]
    fn heap_mutators_require_a_row_ref() {
        let workspace = temp_workspace("heap_row_ref", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Trail",
            32,
            8,
            vec![
                field("points", "vec:f32", 0, 24, 8),
                field("kind", "u32", 24, 4, 4),
            ],
        );
        let accessors = [resolved_accessor(
            "pill_spline::Trail",
            "points",
            "vec",
            "f32",
        )];
        generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[component],
            &[],
            &[],
            &accessors,
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("public ref struct TrailRowRef"));
        assert!(content.contains("private global::TracyLive.RowPointer _row;"));
        assert!(content.contains("public static TrailRowRef Bind(ref Trail row)"));
        assert!(content.contains(
            "return new TrailRowRef { _row = global::TracyLive.MirrorMethods.AddressOf(ref row) };"
        ));
        // The bind checks the trampoline addresses once for the whole handle.
        assert!(content.contains("Trail.EnsureAccessorsBound();"));
        // Read-only members stay on the value type, and the mutating member's
        // documentation points at the handle.
        assert!(content.contains("public readonly int PointsCount"));
        assert!(content.contains("reach it through `TrailRowRef.Bind`"));
        assert!(content.contains("live.ResizePoints(8);"));
    }

    /// A `vec:string` field emits per-element accessors: count, get, set,
    /// push and resize. Each element read or write is one boundary call and
    /// never a span, because every string is allocated separately on the Rust
    /// side.
    #[test]
    fn heap_vec_string_field_emits_element_accessors() {
        let workspace = temp_workspace("heap_vec_string", "pill_spline");
        let component = exposed_typed(
            "pill_spline.NameList",
            32,
            8,
            vec![
                field("names", "vec:string", 0, 24, 8),
                field("kind", "u32", 24, 4, 4),
            ],
        );
        let accessors = [resolved_accessor(
            "pill_spline::NameList",
            "names",
            "vecstring",
            "string",
        )];
        generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[component],
            &[],
            &[],
            &accessors,
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        // No span can exist over separately allocated elements, and no
        // container header becomes a C# field.
        assert!(!content.contains("ReadOnlySpan<string>"));
        assert!(!content.contains("NamesMut"));
        assert!(!content.contains("UIntPtr NamesPtr"));
        assert!(content.contains("[FieldOffset(0)] private readonly ulong _alignmentPad;"));
        // The five per-element members.
        assert!(content.contains("public readonly int NamesCount"));
        assert!(content.contains("public readonly string GetNames(int index)"));
        assert!(content.contains("public void SetNames(int index, string value)"));
        assert!(content.contains("public void PushNames(string value)"));
        assert!(content.contains("public void ResizeNames(int count)"));
        assert!(content.contains(
            "_namesItemAddress = global::TracyLive.MirrorMethods.Address(\"pill_spline::NameList\", \"names_item\");"
        ));
        assert!(content.contains(
            "_namesSetItemAddress = global::TracyLive.MirrorMethods.Address(\"pill_spline::NameList\", \"names_set_item\");"
        ));
        assert!(content.contains(
            "_namesPushAddress = global::TracyLive.MirrorMethods.Address(\"pill_spline::NameList\", \"names_push\");"
        ));
        assert!(content.contains("global::TracyLive.MirrorMethods.InvokeItem(_namesItemAddress,"));
        assert!(
            content.contains("global::TracyLive.MirrorMethods.InvokeSetItem(_namesSetItemAddress,")
        );
        assert!(
            content.contains("global::TracyLive.MirrorMethods.InvokeUtf8Write(_namesPushAddress,")
        );
        // Element reads decode UTF-8, and an unreachable element throws
        // instead of silently reading as an empty string.
        assert!(content.contains("global::TracyLive.ComponentViews.ToUtf8String(view)"));
        assert!(
            content.contains("throw new global::System.ArgumentOutOfRangeException(nameof(index)")
        );
    }

    /// A `vec:string` field whose module published only some of the element
    /// trampolines is refused: the generated members would otherwise fail at
    /// C# call time instead of at build time.
    #[test]
    fn heap_vec_string_without_element_trampolines_is_refused() {
        let workspace = temp_workspace("heap_vec_string_missing", "pill_spline");
        let component = exposed_typed(
            "pill_spline.NameList",
            32,
            8,
            vec![
                field("names", "vec:string", 0, 24, 8),
                field("kind", "u32", 24, 4, 4),
            ],
        );
        let mut accessor =
            resolved_accessor("pill_spline::NameList", "names", "vecstring", "string");
        accessor.item_address = None;
        let codegen_result = generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[component],
            &[],
            &[],
            &[accessor],
        );
        let error = codegen_result.expect_err("a missing element trampoline must be refused");
        assert!(error.contains("`item` trampoline"));
    }

    /// A `Vec` of a struct needs `#[derive(PillMirror)]` on the element type:
    /// that descriptor is where the span's C# struct and size come from. With
    /// one declared, the nested struct is emitted before the component.
    #[test]
    fn heap_vec_of_struct_requires_a_declared_value_type() {
        let workspace = temp_workspace("heap_vec_struct", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Path",
            32,
            8,
            vec![
                field("nodes", "vec:struct:pill_core::math::Vector2f", 0, 24, 8),
                field("kind", "u32", 24, 4, 4),
            ],
        );
        let accessors = [resolved_accessor(
            "pill_spline::Path",
            "nodes",
            "vec",
            "struct:pill_core::math::Vector2f",
        )];
        let codegen_result = generate_module_components_csharp(
            &workspace,
            "pill_spline",
            std::slice::from_ref(&component),
            &[],
            &[],
            &accessors,
        );
        let error = codegen_result.expect_err("an undeclared element type must be refused");
        assert!(error.contains("PillMirror"));

        let value_types = [PillValueTypeDescriptor {
            type_name: "pill_core::math::Vector2f",
            crate_name: "pill_core",
            size: 8,
            align: 4,
            fields: VECTOR2F_FIELDS,
        }];
        generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[component],
            &value_types,
            &[],
            &accessors,
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        assert!(content.contains("public Span<Vector2f> NodesMut"));
        assert!(content.contains("[FieldOffset(0)] private readonly ulong _alignmentPad;"));
        let nested_index = content
            .find("public partial struct Vector2f")
            .expect("nested struct");
        let component_index = content
            .find("public partial struct Path")
            .expect("component struct");
        assert!(nested_index < component_index);
    }

    /// A container field whose module published no accessor means the module
    /// and its mirror disagree about what exists. Codegen stops with an error
    /// rather than writing a mirror that cannot compile.
    #[test]
    fn heap_field_without_a_published_accessor_is_refused() {
        let workspace = temp_workspace("heap_missing_accessor", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Trail",
            32,
            8,
            vec![field("points", "vec:f32", 0, 24, 8)],
        );
        let codegen_result = generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[component],
            &[],
            &[],
            &[],
        );
        let error =
            codegen_result.expect_err("a container field without an accessor must be refused");
        assert!(error.contains("no accessor"));
    }

    /// An engine-owned `DynamicBuffer` field is mirrored as its three raw
    /// words plus accessor members that need no calls. The count and the
    /// spans read the row directly; only resizing resolves a trampoline.
    #[test]
    fn dynamic_buffer_field_emits_zero_call_members() {
        let workspace = temp_workspace("dynbuf", "pill_spline");
        let component = exposed_typed(
            "pill_spline.Trail",
            32,
            8,
            vec![
                field("points", "dynbuf:f32", 0, 24, 8),
                field("kind", "u32", 24, 4, 4),
            ],
        );
        let accessors = [resolved_accessor(
            "pill_spline::Trail",
            "points",
            "dynbuf",
            "f32",
        )];
        generate_module_components_csharp(
            &workspace,
            "pill_spline",
            &[component],
            &[],
            &[],
            &accessors,
        )
        .unwrap();

        let content = read_generated(&workspace, "pill_spline");
        // The handle's words are real fields, so the struct stays blittable
        // and no alignment pad is needed.
        assert!(content.contains("[FieldOffset(0)] private UIntPtr PointsPtr;"));
        assert!(content.contains("[FieldOffset(8)] private UIntPtr PointsLen;"));
        assert!(content.contains("[FieldOffset(16)] private UIntPtr PointsCap;"));
        assert!(!content.contains("_alignmentPad"));
        // Count and spans read straight from the row: no trampoline, no call.
        assert!(content.contains("public readonly int PointsCount => checked((int)PointsLen);"));
        assert!(content.contains("public readonly ReadOnlySpan<float> Points =>"));
        assert!(content.contains("public Span<float> PointsMut =>"));
        assert!(!content.contains("PointsViewDelegate"));
        // Only the resize goes through the module's trampoline.
        assert!(content.contains(
            "_pointsResizeAddress = global::TracyLive.MirrorMethods.Address(\"pill_spline::Trail\", \"points_resize\");"
        ));
        assert!(
            content.contains("global::TracyLive.MirrorMethods.InvokeResize(_pointsResizeAddress,")
        );
        assert!(content.contains("public void ResizePoints(int count)"));
        // The blittable field after the handle is still emitted at its offset.
        assert!(content.contains("[FieldOffset(24)] public uint Kind;"));
    }
}
