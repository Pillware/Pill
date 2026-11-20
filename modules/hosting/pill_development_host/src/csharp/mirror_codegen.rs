//! Builds the C# text that mirrors a module's Rust types into the generated
//! project: the classes, enums, resource markers and their members.
//!
//! # Responsibilities
//!
//! - Find the C# type behind a tag's name (`val:Mesh`, `mut:AssetManager`)
//!   through [`MirrorScope`]: an object class, an enum, a resource marker,
//!   a value-type struct, or one of the runtime's own types.
//! - Write one C# member for each mirrored function with [`emit_member`],
//!   wherever the function lives: on a value type, an object class, a
//!   resource (as extension methods on `Res<T>` / `ResMut<T>`), or a module
//!   (as a static class).
//! - Write the object classes, enums and resource markers the type rows
//!   declare, through [`emit_declared_type`].
//!
//! # Design
//!
//! Every generated member has the same shape. It starts a
//! `TracyLive.MirrorCall` for the function, pushes the receiver and each
//! argument as the tag says, invokes the call, then reads the result as the
//! result tag says. All pointers stay inside the C# runtime, so the
//! generated code is plain safe C# and the project assembly that gets
//! reloaded needs no `AllowUnsafeBlocks`. One function-pointer type serves
//! every trampoline, so NativeAOT generates nothing per signature. The byte
//! layout that `Push*` and `Result*` calls write is the contract with
//! `pill_engine::mirror` on the Rust side.

// Standard library
use std::collections::BTreeMap;

// External crates
use pill_csharp_bridge::{snake_to_pascal, ResolvedMirrorMethod};
use pill_engine::component_registry::{PillValueTypeDescriptor, MIRROR_TYPE_ROW};

// =============================================================================
// Constants
// =============================================================================

/// The runtime's own resource marker for the engine's asset store.
const ASSET_MANAGER: &str = "global::TracyLive.AssetManager";

// =============================================================================
// Types
// =============================================================================

/// What a mirrored Rust type becomes on the C# side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum MirrorKind {
    /// A `#[pill_mirror_object]` class.
    Object,
    /// A `#[derive(PillMirror)]` enum.
    Enum,
    /// A `#[pill_mirror_resource]` marker (or the runtime's `AssetManager`).
    Resource,
    /// A `#[derive(PillMirror)]` value type.
    Value,
}

/// One type a module's signatures can name, with the C# declaration it maps
/// to.
#[derive(Clone, Debug)]
pub(super) struct MirrorType {
    /// Which C# declaration it becomes.
    pub(super) kind: MirrorKind,
    /// The C# name to write: `global::ns.Name`, or the simple name alone when
    /// the value type has no namespace.
    pub(super) csharp_name: String,
}

/// One object type, enum or resource a module declares through a type row.
#[derive(Clone, Debug)]
pub(super) struct DeclaredType {
    /// The Rust qualified name that its functions are stored under.
    pub(super) type_name: String,
    /// The C# simple name.
    pub(super) csharp_name: String,
    /// The C# namespace.
    pub(super) namespace: String,
    /// The kind of declaration the type row asked for.
    pub(super) kind: MirrorKind,
    /// The type row it came from: its flags, variants, representation or
    /// shared name.
    pub(super) row: ResolvedMirrorMethod,
}

/// Every type a module's signatures can name.
pub(super) struct MirrorScope {
    /// Keyed by the simple Rust name a tag carries.
    types_by_name: BTreeMap<String, MirrorType>,
    /// The object types, enums and resources the module declares, in a
    /// deterministic order.
    pub(super) declared: Vec<DeclaredType>,
}

impl MirrorScope {
    /// Builds the lookup table from a module's mirrored methods and value
    /// types.
    ///
    /// `value_namespace` gives the namespace to put a value type in, or
    /// `None` to write its simple name with no namespace.
    ///
    /// # Errors
    ///
    /// Returns an error when a type row names an owner kind this file does not
    /// know, or when two declared types would get the same C# simple name.
    pub(super) fn new(
        methods: &[ResolvedMirrorMethod],
        value_types: &[PillValueTypeDescriptor],
        value_namespace: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        // Step 1: add the runtime's own types first. `AssetManager` is a
        // resource marker the runtime defines, so any module may name it.
        let mut types_by_name = BTreeMap::new();
        types_by_name.insert(
            "AssetManager".to_string(),
            MirrorType {
                kind: MirrorKind::Resource,
                csharp_name: ASSET_MANAGER.to_string(),
            },
        );

        // Step 2: read the type rows the module declares.
        let mut declared = Vec::new();
        for row in methods
            .iter()
            .filter(|method| method.method_name == MIRROR_TYPE_ROW)
        {
            let kind = match row.owner_kind.as_str() {
                pill_engine::component_registry::OWNER_KIND_OBJECT => MirrorKind::Object,
                pill_engine::component_registry::OWNER_KIND_ENUM => MirrorKind::Enum,
                pill_engine::component_registry::OWNER_KIND_RESOURCE => MirrorKind::Resource,
                other => {
                    return Err(format!(
                        "type row of `{}` has unknown kind `{other}`",
                        row.type_name
                    ))
                }
            };
            let name = last_segment(&row.type_name).to_string();
            let namespace = crate_namespace(&row.type_name);
            // Tags carry only a simple name, so two Rust types whose paths
            // end in the same segment cannot both be found.
            if let Some(existing) = declared
                .iter()
                .find(|existing: &&DeclaredType| existing.csharp_name == name)
            {
                return Err(format!(
                    "mirrored types `{}` and `{}` both map to C# name `{name}`; rename one",
                    existing.type_name, row.type_name
                ));
            }
            types_by_name.insert(
                name.clone(),
                MirrorType {
                    kind: kind.clone(),
                    csharp_name: format!("global::{namespace}.{name}"),
                },
            );
            declared.push(DeclaredType {
                type_name: row.type_name.clone(),
                csharp_name: name,
                namespace,
                kind,
                row: row.clone(),
            });
        }

        // Step 3: sort the declared types so the generated file comes out the
        // same on every run.
        declared.sort_by(|left, right| left.type_name.cmp(&right.type_name));

        // Step 4: add the value types last; they have no type row of their
        // own. A declared type wins over a value type with the same simple
        // name.
        for value_type in value_types {
            let name = last_segment(value_type.type_name).to_string();
            if types_by_name.contains_key(&name) {
                continue;
            }
            // Without a namespace the type is named by its simple name alone.
            let csharp_name = match value_namespace(value_type.type_name) {
                Some(namespace) => format!("global::{namespace}.{name}"),
                None => name.clone(),
            };
            types_by_name.insert(
                name,
                MirrorType {
                    kind: MirrorKind::Value,
                    csharp_name,
                },
            );
        }
        Ok(Self {
            types_by_name,
            declared,
        })
    }

    /// The type a tag's name refers to.
    ///
    /// # Errors
    ///
    /// Returns an error that names the tag when this module does not mirror
    /// that name.
    pub(super) fn resolve(&self, type_name: &str) -> Result<&MirrorType, String> {
        self.types_by_name.get(type_name).ok_or_else(|| {
            format!(
                "`{type_name}` is not a type this module mirrors; declare it with \
                 `#[pill_mirror_object]`, `#[pill_mirror_resource]` or `#[derive(PillMirror)]`"
            )
        })
    }

    /// The kind of a declared type, looked up by its Rust qualified name.
    pub(super) fn declared_kind(&self, type_name: &str) -> Option<&MirrorKind> {
        self.declared
            .iter()
            .find(|declared| declared.type_name == type_name)
            .map(|declared| &declared.kind)
    }

    /// The C# type to give a component field tagged `struct:Handle<Name>`,
    /// when `Name` is an object this module mirrors.
    pub(super) fn typed_handle(&self, asset_type_name: &str) -> Option<String> {
        self.types_by_name
            .get(asset_type_name)
            .filter(|resolved| resolved.kind == MirrorKind::Object)
            .map(|resolved| format!("global::TracyLive.Handle<{}>", resolved.csharp_name))
    }
}

/// One function argument, translated to C#.
struct CsArgument {
    /// The parameter declaration (`ReadOnlySpan<byte> bytes`).
    declaration: String,
    /// The statements that push it onto the call.
    push_statements: String,
    /// The expression that forwards the parameter to another member
    /// (`in value`).
    forward_expression: String,
}

/// One function result, translated to C#.
struct CsResult {
    /// The C# return type.
    csharp_type: String,
    /// The number of bytes the return buffer must hold, written as a C#
    /// expression. Empty means the default buffer.
    buffer_size_expression: String,
    /// The statement that returns the value after the call.
    read_statement: String,
}

/// The place a generated member belongs to.
pub(super) enum MemberHost<'a> {
    /// An instance or static member of a value-type struct.
    ValueStruct,
    /// An instance or static member of an object class.
    Object {
        /// The class's simple name. An associated `new` that returns it also
        /// becomes a constructor.
        name: &'a str,
    },
    /// An extension method on `Res<T>` / `ResMut<T>` of a resource marker.
    Resource {
        /// The marker's fully qualified C# name.
        marker: &'a str,
    },
    /// A static member of a module's static class.
    Module,
}

// =============================================================================
// Path Helpers
// =============================================================================

/// The namespace a declared type is written into: the root of its crate, so
/// one `using` reaches every object, enum and resource a module mirrors.
fn crate_namespace(type_name: &str) -> String {
    type_name
        .split("::")
        .next()
        .unwrap_or(type_name)
        .to_string()
}

/// The last `::` segment of a Rust path.
fn last_segment(type_name: &str) -> &str {
    type_name.rsplit("::").next().unwrap_or(type_name)
}

// =============================================================================
// Primitive Vocabulary
// =============================================================================

/// The C# type and width of a primitive tag.
fn primitive(tag: &str) -> Option<(&'static str, usize)> {
    Some(match tag {
        "u8" => ("byte", 1),
        "u16" => ("ushort", 2),
        "u32" => ("uint", 4),
        "u64" => ("ulong", 8),
        "i8" => ("sbyte", 1),
        "i16" => ("short", 2),
        "i32" => ("int", 4),
        "i64" => ("long", 8),
        "f32" => ("float", 4),
        "f64" => ("double", 8),
        "bool" => ("bool", 1),
        "usize" => ("nuint", 8),
        "isize" => ("nint", 8),
        _ => return None,
    })
}

/// The byte offset and C# type of each element in a tuple of primitives,
/// packed the same way `pill_engine_macros`'s `tuple_layout` packs them.
///
/// # Errors
///
/// Returns an error when an element is not a primitive tag.
fn tuple_layout(elements: &[&str]) -> Result<Vec<(usize, &'static str)>, String> {
    let mut cursor = 0usize;
    let mut layout = Vec::new();
    for element in elements {
        let (csharp, width) = primitive(element)
            .ok_or_else(|| format!("tuple element `{element}` is not a primitive"))?;
        cursor = cursor.div_ceil(width) * width;
        layout.push((cursor, csharp));
        cursor += width;
    }
    Ok(layout)
}

/// The C# type for an `array:<element>:<length>` tag.
///
/// # Errors
///
/// Returns an error for any array the runtime has no vector type for. Only
/// `[f32; 2..=4]` maps to one.
fn array_type(tag: &str) -> Result<String, String> {
    let mut parts = tag.splitn(2, ':');
    let element = parts.next().unwrap_or_default();
    let length = parts.next().unwrap_or_default();
    match (element, length) {
        ("f32", "2") => Ok("global::System.Numerics.Vector2".to_string()),
        ("f32", "3") => Ok("global::System.Numerics.Vector3".to_string()),
        ("f32", "4") => Ok("global::System.Numerics.Vector4".to_string()),
        _ => Err(format!(
            "an `[{element}; {length}]` cannot be mirrored to C#; use `[f32; 2..=4]` or a tuple"
        )),
    }
}

// =============================================================================
// Arguments
// =============================================================================

/// Turns one argument tag into its C# parameter declaration, push statement
/// and forward expression.
///
/// # Errors
///
/// Returns an error for anything this file cannot translate: an unknown tag,
/// a slice element or optional payload it does not support, a tuple element
/// that is not a primitive, an array the runtime has no vector type for, a
/// type name the module does not mirror, or a resource taken by value, inside
/// a slice or inside an option.
fn lower_argument(tag: &str, name: &str, scope: &MirrorScope) -> Result<CsArgument, String> {
    // A parameter declared `in` or `ref` is forwarded with the same modifier,
    // so code that delegates hands it on unchanged.
    let simple = |declaration: String, push: String| {
        let forward = if declaration.starts_with("in ") {
            format!("in {name}")
        } else if declaration.starts_with("ref ") {
            format!("ref {name}")
        } else {
            name.to_string()
        };
        Ok(CsArgument {
            declaration,
            push_statements: push,
            forward_expression: forward,
        })
    };
    if let Some((csharp, _)) = primitive(tag) {
        return simple(format!("{csharp} {name}"), format!("__call.Push({name});"));
    }
    if tag == "str" {
        return simple(
            format!("string {name}"),
            format!("__call.PushString({name});"),
        );
    }
    if tag == "loader" {
        return simple(
            format!("global::TracyLive.AssetLoader {name}"),
            format!("__call.PushLoader({name});"),
        );
    }
    if let Some(asset) = tag.strip_prefix("handle:") {
        let asset = scope.resolve(asset)?;
        return simple(
            format!("global::TracyLive.Handle<{}> {name}", asset.csharp_name),
            format!("__call.Push({name});"),
        );
    }
    if let Some(element) = tag.strip_prefix("slice:") {
        if let Some((csharp, _)) = primitive(element) {
            return simple(
                format!("global::System.ReadOnlySpan<{csharp}> {name}"),
                format!("__call.PushSpan({name});"),
            );
        }
        if element == "str" {
            return simple(
                format!("global::System.ReadOnlySpan<string> {name}"),
                format!("__call.PushStrings({name});"),
            );
        }
        if let Some(asset) = element.strip_prefix("handle:") {
            let asset = scope.resolve(asset)?;
            return simple(
                format!(
                    "global::System.ReadOnlySpan<global::TracyLive.Handle<{}>> {name}",
                    asset.csharp_name
                ),
                format!("__call.PushSpan({name});"),
            );
        }
        if let Some(named) = element.strip_prefix("val:") {
            let resolved = scope.resolve(named)?;
            let push = match resolved.kind {
                MirrorKind::Object => format!("__call.PushObjects({name});"),
                MirrorKind::Value | MirrorKind::Enum => format!("__call.PushSpan({name});"),
                MirrorKind::Resource => {
                    return Err(format!("a slice of resource `{named}` cannot be mirrored"))
                }
            };
            return simple(
                format!(
                    "global::System.ReadOnlySpan<{}> {name}",
                    resolved.csharp_name
                ),
                push,
            );
        }
        return Err(format!("unsupported slice element tag `{element}`"));
    }
    if let Some(payload_tag) = tag.strip_prefix("option:") {
        if let Some((csharp, _)) = primitive(payload_tag) {
            return simple(
                format!("{csharp}? {name}"),
                format!("__call.PushOptional({name});"),
            );
        }
        if let Some(asset) = payload_tag.strip_prefix("handle:") {
            let asset = scope.resolve(asset)?;
            return simple(
                format!("global::TracyLive.Handle<{}>? {name}", asset.csharp_name),
                format!("__call.PushOptional({name});"),
            );
        }
        if let Some(named) = payload_tag.strip_prefix("val:") {
            let resolved = scope.resolve(named)?;
            return match resolved.kind {
                MirrorKind::Object => simple(
                    format!("{}? {name}", resolved.csharp_name),
                    format!("__call.PushOptionalObject({name});"),
                ),
                MirrorKind::Enum => simple(
                    format!("{}? {name}", resolved.csharp_name),
                    format!("__call.PushOptional({name});"),
                ),
                _ => Err(format!("an optional `{named}` cannot be mirrored")),
            };
        }
        return Err(format!("unsupported optional tag `{payload_tag}`"));
    }
    if let Some(elements) = tag.strip_prefix("tuple:") {
        let elements: Vec<&str> = elements.split(',').collect();
        let layout = tuple_layout(&elements)?;
        let types: Vec<&str> = layout.iter().map(|(_, csharp)| *csharp).collect();
        let pushes: Vec<String> = layout
            .iter()
            .enumerate()
            .map(|(index, (offset, _))| {
                format!("__call.PushField({offset}, {name}.Item{});", index + 1)
            })
            .collect();
        return simple(
            format!("({}) {name}", types.join(", ")),
            format!("{} __call.EndSlot();", pushes.join(" ")),
        );
    }
    if let Some(array) = tag.strip_prefix("array:") {
        return simple(
            format!("{} {name}", array_type(array)?),
            format!("__call.Push({name});"),
        );
    }
    if let Some((mutable, named)) = tag
        .strip_prefix("ref:")
        .map(|named| (false, named))
        .or_else(|| tag.strip_prefix("mut:").map(|named| (true, named)))
    {
        let resolved = scope.resolve(named)?;
        return match resolved.kind {
            MirrorKind::Object => simple(
                format!("{} {name}", resolved.csharp_name),
                format!("__call.PushBorrowed({name});"),
            ),
            MirrorKind::Resource => {
                let (parameter, access) = if mutable {
                    ("ResMut", "Write")
                } else {
                    ("Res", "Read")
                };
                simple(
                    format!(
                        "global::TracyLive.{parameter}<{}> {name}",
                        resolved.csharp_name
                    ),
                    format!(
                        "__call.PushResource<{}>(global::TracyLive.QueryAccess.{access});",
                        resolved.csharp_name
                    ),
                )
            }
            MirrorKind::Value => {
                if mutable {
                    simple(
                        format!("ref {} {name}", resolved.csharp_name),
                        format!("__call.PushAddress(ref {name});"),
                    )
                } else {
                    simple(
                        format!("in {} {name}", resolved.csharp_name),
                        format!(
                            "__call.PushAddress(ref global::System.Runtime.CompilerServices.Unsafe.AsRef(in {name}));"
                        ),
                    )
                }
            }
            MirrorKind::Enum => simple(
                format!("{} {name}", resolved.csharp_name),
                format!("__call.PushCopy({name});"),
            ),
        };
    }
    if let Some(named) = tag.strip_prefix("val:") {
        let resolved = scope.resolve(named)?;
        return match resolved.kind {
            MirrorKind::Object => simple(
                format!("{} {name}", resolved.csharp_name),
                format!("__call.PushMoved({name});"),
            ),
            MirrorKind::Value => simple(
                format!("in {} {name}", resolved.csharp_name),
                format!("__call.PushCopy({name});"),
            ),
            MirrorKind::Enum => simple(
                format!("{} {name}", resolved.csharp_name),
                format!("__call.Push({name});"),
            ),
            MirrorKind::Resource => Err(format!(
                "resource `{named}` cannot be taken by value; take `&{named}` or `&mut {named}`"
            )),
        };
    }
    Err(format!("unsupported argument tag `{tag}`"))
}

// =============================================================================
// Results
// =============================================================================

/// Turns one result tag into the C# return type, buffer size and read
/// statement.
///
/// # Errors
///
/// Returns an error for anything this file cannot translate: an unsupported
/// tag or optional payload, a tuple element that is not a primitive, an array
/// the runtime has no vector type for, a type name the module does not mirror,
/// or a resource returned by value.
fn lower_result(tag: &str, scope: &MirrorScope) -> Result<CsResult, String> {
    // A return tag may or may not start with `result:`. What follows that
    // prefix is the value tag.
    let tag = tag.strip_prefix("result:").unwrap_or(tag);
    // A plain result reads from the default return buffer, so it gives no size.
    let plain = |csharp: String, read: String| {
        Ok(CsResult {
            csharp_type: csharp,
            buffer_size_expression: String::new(),
            read_statement: read,
        })
    };
    if tag.is_empty() {
        return plain("void".to_string(), String::new());
    }
    if let Some((csharp, _)) = primitive(tag) {
        return plain(
            csharp.to_string(),
            format!("return __call.Result<{csharp}>();"),
        );
    }
    if tag == "str" {
        return plain(
            "string".to_string(),
            "return __call.ResultString();".to_string(),
        );
    }
    if let Some(asset) = tag.strip_prefix("handle:") {
        let csharp = format!(
            "global::TracyLive.Handle<{}>",
            scope.resolve(asset)?.csharp_name
        );
        return plain(csharp.clone(), format!("return __call.Result<{csharp}>();"));
    }
    if let Some(elements) = tag.strip_prefix("tuple:") {
        let elements: Vec<&str> = elements.split(',').collect();
        let layout = tuple_layout(&elements)?;
        let types: Vec<&str> = layout.iter().map(|(_, csharp)| *csharp).collect();
        let reads: Vec<String> = layout
            .iter()
            .map(|(offset, csharp)| format!("__call.ResultAt<{csharp}>({offset})"))
            .collect();
        return plain(
            format!("({})", types.join(", ")),
            format!("return ({});", reads.join(", ")),
        );
    }
    if let Some(array) = tag.strip_prefix("array:") {
        let csharp = array_type(array)?;
        return plain(csharp.clone(), format!("return __call.Result<{csharp}>();"));
    }
    if let Some(named) = tag.strip_prefix("val:") {
        let resolved = scope.resolve(named)?;
        let csharp = resolved.csharp_name.clone();
        return match resolved.kind {
            MirrorKind::Object => plain(
                csharp.clone(),
                format!("return new {csharp}(__call.ResultObject(0));"),
            ),
            MirrorKind::Enum => plain(csharp.clone(), format!("return __call.Result<{csharp}>();")),
            MirrorKind::Value => Ok(CsResult {
                buffer_size_expression: format!(
                    "global::System.Runtime.CompilerServices.Unsafe.SizeOf<{csharp}>()"
                ),
                read_statement: format!("return __call.Result<{csharp}>();"),
                csharp_type: csharp,
            }),
            MirrorKind::Resource => Err(format!("resource `{named}` cannot be returned")),
        };
    }
    if let Some(payload_tag) = tag.strip_prefix("option:") {
        // An optional result keeps its payload in the second 16-byte slot,
        // behind the presence flag in the first.
        if let Some((csharp, _)) = primitive(payload_tag) {
            return plain(
                format!("{csharp}?"),
                format!("return __call.ResultPresent() ? __call.ResultAt<{csharp}>(16) : null;"),
            );
        }
        if let Some(asset) = payload_tag.strip_prefix("handle:") {
            let csharp = format!(
                "global::TracyLive.Handle<{}>",
                scope.resolve(asset)?.csharp_name
            );
            return plain(
                format!("{csharp}?"),
                format!("return __call.ResultPresent() ? __call.ResultAt<{csharp}>(16) : null;"),
            );
        }
        if let Some(named) = payload_tag.strip_prefix("val:") {
            let resolved = scope.resolve(named)?;
            let csharp = resolved.csharp_name.clone();
            return match resolved.kind {
                MirrorKind::Object => plain(
                    format!("{csharp}?"),
                    format!("return __call.ResultPresent() ? new {csharp}(__call.ResultObject(16)) : null;"),
                ),
                MirrorKind::Enum => plain(
                    format!("{csharp}?"),
                    format!("return __call.ResultPresent() ? __call.ResultAt<{csharp}>(16) : null;"),
                ),
                MirrorKind::Value => Ok(CsResult {
                    buffer_size_expression: format!(
                        "16 + global::System.Runtime.CompilerServices.Unsafe.SizeOf<{csharp}>()"
                    ),
                    read_statement: format!(
                        "return __call.ResultPresent() ? __call.ResultAt<{csharp}>(16) : null;"
                    ),
                    csharp_type: format!("{csharp}?"),
                }),
                MirrorKind::Resource => Err(format!("resource `{named}` cannot be returned")),
            };
        }
        return Err(format!("unsupported optional result tag `{payload_tag}`"));
    }
    Err(format!("unsupported result tag `{tag}`"))
}

// =============================================================================
// Members
// =============================================================================

/// The C# parameter name for one argument: the Rust name, or the positional
/// `arg<index>` when that name is blank or a C# keyword.
fn parameter_name(index: usize, rust_name: &str) -> String {
    super::codegen::csharp_parameter_name(index, rust_name)
}

/// Writes the C# member, or members, for one mirrored function.
///
/// # Errors
///
/// Returns an error that names the function when one of its argument or
/// result tags cannot be translated, or when its receiver cannot be written on
/// its host.
pub(super) fn emit_member(
    method: &ResolvedMirrorMethod,
    host: &MemberHost<'_>,
    scope: &MirrorScope,
) -> Result<String, String> {
    // Step 1: turn the arguments and the result into C# declarations, push
    // statements and a read.
    let function_label = || {
        format!(
            "mirrored function `{}::{}`",
            method.type_name, method.method_name
        )
    };
    let pascal_name = snake_to_pascal(&method.method_name);
    let mut declarations = Vec::new();
    let mut pushes = Vec::new();
    let mut forwards = Vec::new();
    for (index, tag) in method.arg_tags.iter().enumerate() {
        let rust_name = method.arg_names.get(index).map_or("", String::as_str);
        let argument = lower_argument(tag, &parameter_name(index, rust_name), scope)
            .map_err(|error| format!("{}: {error}", function_label()))?;
        declarations.push(argument.declaration);
        pushes.push(argument.push_statements);
        forwards.push(argument.forward_expression);
    }
    let result = lower_result(&method.return_tag, scope)
        .map_err(|error| format!("{}: {error}", function_label()))?;
    // Only a value result needs an explicit return-buffer size. Every other
    // result reads the default buffer.
    let begin = if result.buffer_size_expression.is_empty() {
        format!(
            "var __call = global::TracyLive.MirrorCall.Begin(\"{}\", \"{}\");",
            method.type_name, method.method_name
        )
    } else {
        format!(
            "var __call = global::TracyLive.MirrorCall.Begin(\"{}\", \"{}\", {});",
            method.type_name, method.method_name, result.buffer_size_expression
        )
    };
    let doc = format!(
        "    /// <summary>Calls the Rust function <c>{}::{}</c>.</summary>\n",
        method.type_name, method.method_name
    );

    // Step 2: work out the variants this host and receiver produce: a
    // signature prefix, an optional receiver parameter and the push of that
    // receiver.
    let variants: Vec<(String, Option<String>, Option<String>)> = match (host, method.receiver.as_str()) {
        (MemberHost::Module, "") => vec![("public static".to_string(), None, None)],
        (MemberHost::Module, other) => {
            return Err(format!(
                "{}: a free function cannot have a `{other}` receiver",
                function_label()
            ))
        }
        (MemberHost::ValueStruct | MemberHost::Object { .. }, "") => {
            vec![("public static".to_string(), None, None)]
        }
        (MemberHost::ValueStruct, "ref" | "mut") => vec![(
            "public".to_string(),
            None,
            Some(
                "__call.PushAddress(ref global::System.Runtime.CompilerServices.Unsafe.AsRef(in this));"
                    .to_string(),
            ),
        )],
        (MemberHost::ValueStruct, "value") => vec![(
            "public readonly".to_string(),
            None,
            Some("__call.PushCopy(this);".to_string()),
        )],
        (MemberHost::Object { .. }, "ref" | "mut") => vec![(
            "public".to_string(),
            None,
            Some("__call.PushBorrowed(this);".to_string()),
        )],
        (MemberHost::Object { .. }, "value") => vec![(
            "public".to_string(),
            None,
            Some("__call.PushMoved(this);".to_string()),
        )],
        (MemberHost::Resource { .. }, "") => vec![("public static".to_string(), None, None)],
        (MemberHost::Resource { marker }, "ref") => vec![
            (
                "public static".to_string(),
                Some(format!("this global::TracyLive.Res<{marker}> self")),
                Some(format!(
                    "__call.PushResource<{marker}>(global::TracyLive.QueryAccess.Read);"
                )),
            ),
            (
                "public static".to_string(),
                Some(format!("this global::TracyLive.ResMut<{marker}> self")),
                Some(format!(
                    "__call.PushResource<{marker}>(global::TracyLive.QueryAccess.Read);"
                )),
            ),
        ],
        (MemberHost::Resource { marker }, "mut") => vec![(
            "public static".to_string(),
            Some(format!("this global::TracyLive.ResMut<{marker}> self")),
            Some(format!(
                "__call.PushResource<{marker}>(global::TracyLive.QueryAccess.Write);"
            )),
        )],
        (_, other) => {
            return Err(format!("{}: unsupported receiver `{other}`", function_label()));
        }
    };

    // Step 3: write one member per variant.
    let mut output = String::new();
    for (prefix, receiver_parameter, receiver_push) in variants {
        let mut parameters: Vec<String> = Vec::new();
        if let Some(receiver_parameter) = receiver_parameter {
            parameters.push(receiver_parameter);
        }
        parameters.extend(declarations.iter().cloned());
        let mut body = String::new();
        body.push_str(&format!("        {begin}\n"));
        if let Some(receiver_push) = receiver_push {
            body.push_str(&format!("        {receiver_push}\n"));
        }
        for push in &pushes {
            body.push_str(&format!("        {push}\n"));
        }
        body.push_str("        __call.Invoke();\n");
        if !result.read_statement.is_empty() {
            body.push_str(&format!("        {}\n", result.read_statement));
        }
        output.push_str(&format!(
            "\n{doc}    {prefix} {} {pascal_name}({})\n    {{\n{body}    }}\n",
            result.csharp_type,
            parameters.join(", ")
        ));
    }

    // Step 4: an associated `new` that returns the object itself is also its
    // constructor, so C# reads `new ShaderParameterSlot("tint", parameter_type)`
    // just as Rust reads `ShaderParameterSlot::new("tint", parameter_type)`.
    // The static `New` stays beside it for code that prefers the Rust name.
    if let MemberHost::Object { name } = host {
        let returns_self = method
            .return_tag
            .strip_prefix("result:")
            .unwrap_or(&method.return_tag)
            == format!("val:{name}");
        if method.method_name == "new" && method.receiver.is_empty() && returns_self {
            output.push_str(&format!(
                "\n{doc}    public {name}({})\n        : this(New({}).TakeHandle())\n    {{\n    }}\n",
                declarations.join(", "),
                forwards.join(", ")
            ));
        }
    }
    Ok(output)
}

/// The functions mirrored onto one owner, skipping the `__`-named rows: the
/// `__type` type row and the runtime's own `__asset_*` operations.
pub(super) fn members_of<'a>(
    methods: &'a [ResolvedMirrorMethod],
    type_name: &'a str,
) -> impl Iterator<Item = &'a ResolvedMirrorMethod> {
    methods.iter().filter(move |method| {
        method.type_name == type_name && !method.method_name.starts_with("__")
    })
}

// =============================================================================
// Declared Types
// =============================================================================

/// Writes one declared type and wraps it in its namespace: an object class,
/// an enum, or a resource marker with the static class of its extension
/// methods.
///
/// # Errors
///
/// Returns an error when the type row cannot be written: an enum whose
/// representation is not a primitive, an enum with mirrored methods, a
/// resource with no shared name, a value type that should carry no type row,
/// or a member whose tags cannot be translated.
pub(super) fn emit_declared_type(
    declared: &DeclaredType,
    methods: &[ResolvedMirrorMethod],
    scope: &MirrorScope,
) -> Result<String, String> {
    // Step 1: emit the body for the kind the type row declares.
    let name = &declared.csharp_name;
    let body = match declared.kind {
        MirrorKind::Object => {
            let type_flags = &declared.row.arg_names;
            let mut interfaces = vec![format!("global::TracyLive.IRustObject<{name}>")];
            if type_flags.iter().any(|flag| flag == "asset") {
                interfaces.push(format!("global::TracyLive.IRustAsset<{name}>"));
            }
            if type_flags.iter().any(|flag| flag == "import") {
                interfaces.push(format!("global::TracyLive.IRustImportedAsset<{name}>"));
            }
            if type_flags.iter().any(|flag| flag == "standalone") {
                interfaces.push(format!("global::TracyLive.IRustStandaloneAsset<{name}>"));
            }
            let mut members = String::new();
            for method in members_of(methods, &declared.type_name) {
                members.push_str(&emit_member(method, &MemberHost::Object { name }, scope)?);
            }
            format!(
                "/// <summary>The Rust value <c>{type_name}</c>, owned by this object until it is moved\n\
                 /// into a Rust call or disposed.</summary>\n\
                 public sealed partial class {name} : global::TracyLive.RustObject, {interfaces}\n\
                 {{\n\
                 \x20\x20\x20\x20/// <summary>The Rust type this class wraps.</summary>\n\
                 \x20\x20\x20\x20public const string RustType = \"{type_name}\";\n\
                 \n\
                 \x20\x20\x20\x20static string global::TracyLive.IRustObject<{name}>.RustTypeName => RustType;\n\
                 \n\
                 \x20\x20\x20\x20static {name} global::TracyLive.IRustObject<{name}>.Wrap(global::TracyLive.RustObjectHandle handle) => new(handle);\n\
                 \n\
                 \x20\x20\x20\x20internal {name}(global::TracyLive.RustObjectHandle handle) : base(handle, RustType) {{ }}\n\
                 {members}}}\n",
                type_name = declared.type_name,
                interfaces = interfaces.join(", "),
            )
        }
        MirrorKind::Enum => {
            let representation = primitive(&declared.row.return_tag)
                .map(|(csharp, _)| csharp)
                .ok_or_else(|| {
                    format!(
                        "enum `{}` has unsupported representation `{}`",
                        declared.type_name, declared.row.return_tag
                    )
                })?;
            let variants: Vec<String> = declared
                .row
                .arg_names
                .iter()
                .zip(&declared.row.arg_tags)
                .map(|(variant, value)| format!("    {variant} = {value},"))
                .collect();
            if members_of(methods, &declared.type_name).next().is_some() {
                return Err(format!(
                    "enum `{}` has mirrored methods; a C# enum cannot carry them",
                    declared.type_name
                ));
            }
            format!(
                "/// <summary>The Rust enum <c>{}</c>.</summary>\n\
                 public enum {name} : {representation}\n{{\n{}\n}}\n",
                declared.type_name,
                variants.join("\n")
            )
        }
        MirrorKind::Resource => {
            let shared_name = declared.row.arg_names.first().ok_or_else(|| {
                format!("resource `{}` declares no shared name", declared.type_name)
            })?;
            let marker = format!("global::{}.{name}", declared.namespace);
            let mut members = String::new();
            for method in members_of(methods, &declared.type_name) {
                members.push_str(&emit_member(
                    method,
                    &MemberHost::Resource { marker: &marker },
                    scope,
                )?);
            }
            format!(
                "/// <summary>The Rust resource <c>{type_name}</c>. Declare <c>Res&lt;{name}&gt;</c> or\n\
                 /// <c>ResMut&lt;{name}&gt;</c> to reach it; its value stays in Rust.</summary>\n\
                 [global::TracyLive.NativeResource(\"{shared_name}\")]\n\
                 public readonly struct {name}\n{{\n}}\n\
                 \n\
                 /// <summary>The mirrored functions of <see cref=\"{name}\"/>.</summary>\n\
                 public static class {name}Methods\n{{{members}}}\n",
                type_name = declared.type_name,
            )
        }
        MirrorKind::Value => {
            return Err(format!(
                "`{}` is a value type, which has no type row",
                declared.type_name
            ))
        }
    };
    // Step 2: wrap the body in the namespace its crate root gives it.
    Ok(format!(
        "namespace {} {{\n\n{body}\n}}\n\n",
        declared.namespace
    ))
}

/// Writes the static class that mirrors a module's free functions.
///
/// `module_path` is the Rust module that declares them. The class takes its
/// last segment in PascalCase as its name and sits in the namespace of the
/// segments before it, so `pill_test::mirror::tint_alpha` reaches C# as
/// `pill_test.Mirror.TintAlpha()`.
///
/// # Errors
///
/// Returns an error when one of the functions' argument or result tags cannot
/// be translated, or when a function carries a receiver.
pub(super) fn emit_free_function_class(
    module_path: &str,
    functions: &[&ResolvedMirrorMethod],
    scope: &MirrorScope,
) -> Result<String, String> {
    // Step 1: derive the class name and namespace from the module path.
    let class_name = snake_to_pascal(last_segment(module_path));
    let namespace = module_path
        .rfind("::")
        .map(|separator| module_path[..separator].replace("::", "."))
        .unwrap_or_else(|| module_path.to_string());
    // Step 2: emit one static member per function.
    let mut body = String::new();
    for function in functions {
        body.push_str(&emit_member(function, &MemberHost::Module, scope)?);
    }
    // Step 3: wrap the members in the class and its namespace.
    Ok(format!(
        "\nnamespace {namespace} {{\n\n\
         /// Static mirror of the free functions the Rust module `{module_path}` declares;\n\
         /// each member calls its trampoline through `TracyLive.MirrorCall`.\n\
         public static class {class_name}\n{{{body}}}\n\n}}\n\n"
    ))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A row shaped like the loader's output, with a made-up trampoline
    /// address.
    fn row(
        type_name: &str,
        method_name: &str,
        receiver: &str,
        owner_kind: &str,
        return_tag: &str,
        arguments: &[(&str, &str)],
    ) -> ResolvedMirrorMethod {
        ResolvedMirrorMethod {
            type_name: type_name.to_string(),
            method_name: method_name.to_string(),
            return_tag: return_tag.to_string(),
            arg_tags: arguments.iter().map(|(tag, _)| tag.to_string()).collect(),
            arg_names: arguments
                .iter()
                .map(|(_, argument_name)| argument_name.to_string())
                .collect(),
            address: 1,
            is_free_function: owner_kind == "module",
            crate_name: "fixture".to_string(),
            receiver: receiver.to_string(),
            owner_kind: owner_kind.to_string(),
        }
    }

    /// The rows of a small fixture module: its type rows and a few mirrored
    /// functions.
    fn fixture() -> Vec<ResolvedMirrorMethod> {
        let mut texture_type = row("fixture::TextureType", "__type", "", "enum", "u8", &[]);
        texture_type.arg_names = vec!["Color".to_string(), "Normal".to_string()];
        texture_type.arg_tags = vec!["0".to_string(), "1".to_string()];
        let mut manager = row(
            "fixture::RenderingManager",
            "__type",
            "",
            "resource",
            "",
            &[],
        );
        manager.arg_names = vec!["fixture::RenderingManager".to_string()];
        let mut mesh = row("fixture::mesh::Mesh", "__type", "", "object", "", &[]);
        mesh.arg_names = vec!["asset".to_string(), "import".to_string()];
        vec![
            mesh,
            row(
                "fixture::mesh::Mesh",
                "from_obj_bytes",
                "",
                "",
                "result:val:Mesh",
                &[("str", "name"), ("slice:u8", "bytes")],
            ),
            row(
                "fixture::mesh::Mesh",
                "vertex_count",
                "ref",
                "",
                "usize",
                &[],
            ),
            row(
                "fixture::mesh::Mesh",
                "new",
                "",
                "",
                "result:val:Mesh",
                &[("str", "name")],
            ),
            texture_type,
            manager,
            row("fixture::RenderingManager", "clear", "mut", "", "", &[]),
            row(
                "fixture::RenderingManager",
                "pipeline",
                "ref",
                "",
                "option:handle:Mesh",
                &[],
            ),
            row(
                "fixture",
                "set_sky",
                "",
                "module",
                "result:",
                &[
                    ("mut:AssetManager", "assets"),
                    ("option:handle:Mesh", "mesh"),
                    ("tuple:u32,u32", "bindings"),
                    ("array:f32:3", "color"),
                ],
            ),
        ]
    }

    /// The scope built from a fixture's rows, with no value types.
    fn fixture_scope(methods: &[ResolvedMirrorMethod]) -> MirrorScope {
        MirrorScope::new(methods, &[], &|_| None).expect("the fixture resolves")
    }

    #[test]
    fn an_object_becomes_a_class_with_static_and_instance_members() {
        let methods = fixture();
        let scope = fixture_scope(&methods);
        let mesh = scope
            .declared
            .iter()
            .find(|declared| declared.csharp_name == "Mesh")
            .unwrap();
        let text = emit_declared_type(mesh, &methods, &scope).unwrap();
        assert!(text.contains("namespace fixture {"), "{text}");
        assert!(
            text.contains("public sealed partial class Mesh : global::TracyLive.RustObject, global::TracyLive.IRustObject<Mesh>, global::TracyLive.IRustAsset<Mesh>, global::TracyLive.IRustImportedAsset<Mesh>"),
            "{text}"
        );
        assert!(
            text.contains("public static global::fixture.Mesh FromObjBytes(string name, global::System.ReadOnlySpan<byte> bytes)"),
            "{text}"
        );
        assert!(text.contains("__call.PushString(name);"), "{text}");
        assert!(
            text.contains("return new global::fixture.Mesh(__call.ResultObject(0));"),
            "{text}"
        );
        assert!(text.contains("public nuint VertexCount()"), "{text}");
        assert!(text.contains("__call.PushBorrowed(this);"), "{text}");
        // An associated `new` returning the object is also its constructor.
        assert!(
            text.contains("public static global::fixture.Mesh New(string name)"),
            "{text}"
        );
        assert!(
            text.contains("public Mesh(string name)\n        : this(New(name).TakeHandle())"),
            "{text}"
        );
    }

    #[test]
    fn an_enum_keeps_its_representation_and_values() {
        let methods = fixture();
        let scope = fixture_scope(&methods);
        let declared = scope
            .declared
            .iter()
            .find(|declared| declared.csharp_name == "TextureType")
            .unwrap();
        let text = emit_declared_type(declared, &methods, &scope).unwrap();
        assert!(text.contains("public enum TextureType : byte"), "{text}");
        assert!(text.contains("    Normal = 1,"), "{text}");
    }

    #[test]
    fn a_resource_becomes_a_marker_with_extension_methods() {
        let methods = fixture();
        let scope = fixture_scope(&methods);
        let declared = scope
            .declared
            .iter()
            .find(|declared| declared.csharp_name == "RenderingManager")
            .unwrap();
        let text = emit_declared_type(declared, &methods, &scope).unwrap();
        assert!(
            text.contains("[global::TracyLive.NativeResource(\"fixture::RenderingManager\")]"),
            "{text}"
        );
        assert!(
            text.contains("public static void Clear(this global::TracyLive.ResMut<global::fixture.RenderingManager> self)"),
            "{text}"
        );
        assert!(
            !text.contains("Clear(this global::TracyLive.Res<"),
            "a write needs ResMut: {text}"
        );
        assert!(
            text.contains(
                "Pipeline(this global::TracyLive.Res<global::fixture.RenderingManager> self)"
            ),
            "a read is offered on Res: {text}"
        );
        assert!(
            text.contains(
                "Pipeline(this global::TracyLive.ResMut<global::fixture.RenderingManager> self)"
            ),
            "and on ResMut: {text}"
        );
        assert!(
            text.contains("return __call.ResultPresent() ? __call.ResultAt<global::TracyLive.Handle<global::fixture.Mesh>>(16) : null;"),
            "{text}"
        );
    }

    #[test]
    fn a_free_function_takes_resources_options_tuples_and_vectors() {
        let methods = fixture();
        let scope = fixture_scope(&methods);
        let functions: Vec<&ResolvedMirrorMethod> = methods
            .iter()
            .filter(|method| method.is_free_function)
            .collect();
        let text = emit_free_function_class("fixture", &functions, &scope).unwrap();
        assert!(
            text.contains("public static void SetSky(global::TracyLive.ResMut<global::TracyLive.AssetManager> assets, global::TracyLive.Handle<global::fixture.Mesh>? mesh, (uint, uint) bindings, global::System.Numerics.Vector3 color)"),
            "{text}"
        );
        assert!(
            text.contains("__call.PushResource<global::TracyLive.AssetManager>(global::TracyLive.QueryAccess.Write);"),
            "{text}"
        );
        assert!(
            text.contains("__call.PushField(0, bindings.Item1); __call.PushField(4, bindings.Item2); __call.EndSlot();"),
            "{text}"
        );
    }

    #[test]
    fn an_unknown_type_is_refused_with_its_name() {
        let methods = vec![row(
            "fixture",
            "broken",
            "",
            "module",
            "",
            &[("val:Missing", "value")],
        )];
        let scope = fixture_scope(&methods);
        let functions: Vec<&ResolvedMirrorMethod> = methods.iter().collect();
        let error = emit_free_function_class("fixture", &functions, &scope).unwrap_err();
        assert!(error.contains("Missing"), "{error}");
    }

    #[test]
    fn a_handle_field_is_typed_when_its_asset_is_mirrored() {
        let methods = fixture();
        let scope = fixture_scope(&methods);
        assert_eq!(
            scope.typed_handle("Mesh").as_deref(),
            Some("global::TracyLive.Handle<global::fixture.Mesh>")
        );
        assert_eq!(scope.typed_handle("Unknown"), None);
    }
}
