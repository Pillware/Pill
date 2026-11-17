//! Procedural macros that remove the hand-written registration and FFI
//! boilerplate from extension and project crates.
//!
//! # Responsibilities
//!
//! - [`PillComponent`]: the `Component` impl plus the descriptor a type submits
//!   into this artifact's registry. A persistable component also registers for
//!   schema migration and feeds the project schema fingerprint.
//! - [`PillLayout`]: the compile-time field layout alone, for a type that
//!   registers itself and must not auto-register in every artifact that links
//!   it (engine types, host-linked extensions).
//! - [`pill_value_type!`]: the layout of a foreign value type, with offsets
//!   taken from the compiler.
//! - [`script`]: the declaration a script component's implementation states -
//!   which lifecycle methods it defines, their orders and its edit-mode flag -
//!   plus the descriptor the artifact entry point registers the script from.
//! - [`pill_module`] / [`pill_project`]: the loadable-artifact exports
//!   (`pill_module_init`, `pill_module_abi_version`, the descriptor copies, the
//!   project's schema fingerprint), behind the panic guard and engine-pointer
//!   reconstruction every artifact would otherwise have to write itself.
//! - The mirror attributes ([`pill_mirror_impl`], [`pill_mirror_fn`],
//!   [`pill_mirror_object`], [`pill_mirror_resource`], [`PillMirror`]): Rust
//!   functions and types callable from C# through one C-ABI trampoline shape.
//!
//! # Design
//!
//! Generated code names the engine through fully-qualified `::pill_engine::`
//! paths, so no macro needs to know which crate uses it. Components are
//! collected with the `inventory` crate, which builds one registry per linked
//! artifact: each hot-reload generation DLL holds exactly the components its own
//! sources declared, so a stale generation cannot re-register its types. The
//! mirror lowering lives in `mirror`.

extern crate proc_macro;

/// Converts the mirror attributes' Rust signatures and types into the C ABI
/// form.
mod mirror;

// External crates
use proc_macro::TokenStream;
use quote::{format_ident, quote, ToTokens};
use syn::{parse_macro_input, spanned::Spanned, DeriveInput, ItemFn};

// =============================================================================
// #[derive(PillComponent)]
// =============================================================================

/// Turns a component type into its engine registration.
///
/// Generates `impl Component for T`, a descriptor submitted into this
/// artifact's compile-time registry, the field layout read by the schema hash
/// and the C# mirror codegen, and a `<Type>Field` enum naming every field in
/// code (see `pill_engine::ComponentFields`).
///
/// Type-level helpers (`#[pill(...)]` on the type):
/// - `persistable` - the component survives a reload through schema migration.
///   The type must implement `Serialize + DeserializeOwned + Default`, the same
///   bounds [`World::register_persistable_component_with_layout`] requires.
/// - `shared`, `shared = "crate::Name"` - gives the type one identity in every
///   binary that links it. Without it, each binary has its own `TypeId` and its
///   own column, and neither binary sees the other's entities. The identity is
///   `module_path!()::TypeName` unless pinned, and it must be unique across the
///   whole process.
///
/// Field decorators (`#[pill(...)]` on a field, Unity's vocabulary). A field
/// may take several, comma-separated or as one attribute each. They change
/// presentation only: storage, persistence, the schema fingerprint and the
/// generated C# mirror stay the same.
/// - `hide_in_inspector` (not drawn, still stored), `header("Text")` (a
///   separator line with text) and `space(10.0)` (a vertical gap);
/// - `tooltip("Text")` (hover text) and `min(0.0)` (a drag value with that
///   floor);
/// - `range(0.0, 100.0)` (a slider, in whole steps for an integer field);
/// - `text_area`, `text_area(3, 10)`, `multiline(5)` (multi-line text boxes);
/// - `color` (an array of three or four numbers, or a struct whose `r`, `g`,
///   `b`[, `a`] leaves are numbers; drawn as one colour row with a picker);
///   `vector` (consecutive scalar leaves sharing a dotted prefix, drawn as one
///   X/Y/Z/W row).
///
/// Field types: a primitive, a fixed-size array or a `#[derive(PillMirror)]`
/// struct becomes a typed C# field. `String`, `Vec<E>` and `Vec<String>` are
/// Rust-owned containers, mirrored as accessor members that call
/// derive-generated C-ABI trampolines, so managed code reads and writes the
/// live container in place. A `Vec` resizes through `E: Default + Clone`. The
/// elements of a `Vec<String>` are allocated individually, so each one gets its
/// own accessor. `DynamicBuffer<E>` lives in engine-owned native memory: the
/// mirror reads its `(ptr, len, cap)` handle straight out of the row and calls
/// native code only to resize. `E` must be `Copy`, because the buffer runs no
/// element drop glue.
///
/// [`World::register_persistable_component_with_layout`]: ::pill_engine::World::register_persistable_component_with_layout
#[proc_macro_derive(PillComponent, attributes(pill))]
pub fn derive_pill_component(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = &input.ident;

    // Step 1: Parse the `#[pill(...)]` helper attribute.
    let mut persistable = false;
    let mut shared = false;
    let mut shared_name_override: Option<String> = None;
    let mut gizmo_path: Option<syn::Path> = None;
    let mut gizmo_selected_only = false;
    let mut gizmo_when_declared = false;
    for attribute in &input.attrs {
        if attribute.path().is_ident("pill") {
            if let Err(error) = attribute.parse_nested_meta(|meta| {
                if meta.path.is_ident("persistable") {
                    persistable = true;
                    Ok(())
                } else if meta.path.is_ident("shared") {
                    shared = true;
                    // `shared` on its own derives the name from the module
                    // path; `shared = "..."` pins it explicitly.
                    if meta.input.peek(syn::Token![=]) {
                        let literal: syn::LitStr = meta.value()?.parse()?;
                        let name = literal.value();
                        if name.is_empty() {
                            return Err(syn::Error::new_spanned(
                                &literal,
                                "a shared component name cannot be empty",
                            ));
                        }
                        // A shared name is a process-wide identity: two types
                        // that hold it become one component, one column and one
                        // set of rows. If their layouts also agree, nothing
                        // downstream can tell them apart. So the check has to
                        // happen here, before the collision exists. Requiring a
                        // path separator turns an accidental `"Transform"` into
                        // a compile error, and leaves a deliberate qualified
                        // name - the only way two artifacts can legitimately
                        // share one - untouched.
                        if !name.contains("::") {
                            return Err(syn::Error::new_spanned(
                                &literal,
                                format!(
                                    "shared component name `{name}` is not namespaced. A shared \
                                     name is a process-wide identity, so an unrelated crate \
                                     declaring `{name}` too would bind to this component's \
                                     column and read its rows. Qualify it (`my_crate::{name}`), \
                                     or drop the argument to derive it from `module_path!()`."
                                ),
                            ));
                        }
                        shared_name_override = Some(name);
                    }
                    Ok(())
                } else if meta.path.is_ident("gizmo") {
                    // The function the engine calls to draw this component in
                    // an editor's views. Declared once.
                    if gizmo_path.is_some() {
                        return Err(meta.error("`gizmo` is declared once"));
                    }
                    let path: syn::Path = meta.value()?.parse()?;
                    gizmo_path = Some(path);
                    Ok(())
                } else if meta.path.is_ident("gizmo_when") {
                    let literal: syn::LitStr = meta.value()?.parse()?;
                    gizmo_when_declared = true;
                    gizmo_selected_only = match literal.value().as_str() {
                        "always" => false,
                        "selected" => true,
                        other => {
                            return Err(syn::Error::new(
                                literal.span(),
                                format!(
                                    "unknown `gizmo_when` value `{other}`; expected \
                                     \"always\" or \"selected\""
                                ),
                            ));
                        }
                    };
                    Ok(())
                } else {
                    Err(meta.error(
                        "unknown `pill` attribute; expected `persistable`, `shared`, \
                         `gizmo` or `gizmo_when`",
                    ))
                }
            }) {
                return error.to_compile_error().into();
            }
        }
    }
    // A switch with nothing to switch would read as a declaration that draws
    // something, so it is refused here instead of being silently ignored.
    if gizmo_when_declared && gizmo_path.is_none() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "`gizmo_when` states when the gizmo `gizmo = <path>` draws, so the two are \
             declared together",
        )
        .to_compile_error()
        .into();
    }

    // Step 2: Reject a generic component: the engine registers concrete types
    // keyed by `TypeId`, so a generic type has no single identity to register.
    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &input.generics,
            "`PillComponent` cannot be derived for a generic type",
        )
        .to_compile_error()
        .into();
    }

    // Step 3: Capture the compile-time field layout for the C# mirror codegen,
    // plus the accessor trampolines that let managed code reach a `Vec` or
    // `String` field's live buffer. An unsupported field type is a compile
    // error here, before the host would have to guess at a mirror.
    let (declared_layout, layout_reference, field_accessors, field_enum) =
        match component_field_descriptors(&input, ident, LayoutSubject::Component) {
            Ok(field_descriptor_emissions) => field_descriptor_emissions,
            Err(error) => return error.to_compile_error().into(),
        };

    // Step 4: Record each `Handle<T>` field's asset type, so the field layer
    // knows what the field references. The generated function records it in
    // this artifact, from `T`'s own shared name.
    let asset_field_records = match handle_field_records(&input, ident) {
        Ok(asset_field_records) => asset_field_records,
        Err(error) => return error.to_compile_error().into(),
    };

    let register_fn_name = format_ident!("__pill_register_{}", ident);
    let registration_call = if persistable {
        quote! {
            world.register_persistable_component_with_layout::<#ident>(#layout_reference);
        }
    } else {
        quote! {
            world.register_component_with_layout::<#ident>(#layout_reference);
        }
    };
    // A component that implements `Default` records that value, so the editor
    // adds it - and a scene fills a field it leaves out - from the type's own
    // defaults instead of zeros. Which of the two traits answers is decided by
    // method lookup on the concrete type, so a type without `Default` compiles
    // and simply records nothing.
    let registration_call = quote! {
        #registration_call
        {
            #[allow(unused_imports)]
            use ::pill_engine::component_registry::{
                RecordDefaultValue as _, SkipDefaultValue as _,
            };
            #[allow(clippy::needless_borrow)]
            (&::pill_engine::component_registry::DefaultValueProbe::<#ident>::new())
                .record_default_value(world);
        }
        {
            // A component that implements `Clone` records a row-cloning
            // function, so a storage copy clones its rows; a type without
            // `Clone` records nothing and is copied as bytes when its rows own
            // no heap data. Method lookup decides, exactly as it does for the
            // default value above.
            #[allow(unused_imports)]
            use ::pill_engine::component_registry::{
                RecordCloneValue as _, SkipCloneValue as _,
            };
            #[allow(clippy::needless_borrow)]
            (&::pill_engine::component_registry::CloneValueProbe::<#ident>::new())
                .record_clone_value(world);
        }
    };
    let type_name = quote! {
        ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#ident))
    };

    // A shared component reports a stable name that every binary linking it
    // computes identically. An ordinary component reports no name and keeps the
    // default `TypeId` identity, which is per binary.
    let shared_name_impl = if shared {
        let shared_name = match &shared_name_override {
            Some(name) => quote! { #name },
            None => type_name.clone(),
        };
        quote! {
            fn shared_name() -> ::core::option::Option<&'static str> {
                ::core::option::Option::Some(#shared_name)
            }

            fn shared_identity() -> ::core::option::Option<u128> {
                // The identity is a `const` item rather than the trait's
                // default method, because the default would hash the name on
                // every `ComponentId::of` call, and that cost shows on the
                // per-call random-access path. Computed here, the hash is folded
                // at compile time.
                const IDENTITY: u128 =
                    ::pill_engine::component::shared_component_identity(#shared_name);
                ::core::option::Option::Some(IDENTITY)
            }
        }
    } else {
        quote! {}
    };

    // Step 5: a type that declared a gizmo gets a wrapper that walks its own
    // entities. The walk belongs here, in the declaring artifact, because only
    // this expansion knows the concrete component type; the engine's pass only
    // calls the wrapper. Every generation regenerates it, so a reload replaces
    // the pointer before the old image is unmapped.
    let (gizmo_wrapper, gizmo_descriptor) = match &gizmo_path {
        Some(gizmo_fn) => {
            let wrapper_fn_name = format_ident!("__pill_gizmo_{}", ident);
            let wrapper = quote! {
                /// Draws this component's gizmo for every entity that carries
                /// it, generated from the type's `#[pill(gizmo = ...)]`
                /// declaration.
                #[allow(non_snake_case)]
                fn #wrapper_fn_name(
                    world: &::pill_engine::World,
                    frame: &::pill_engine::gizmos::GizmoFrame,
                    draw: &mut ::pill_engine::gizmos::GizmoDraw,
                ) -> usize {
                    let mut drawn = 0;
                    for archetype in world.archetypes_iter() {
                        if !archetype.has_component::<#ident>() {
                            continue;
                        }
                        for &entity in &archetype.entities {
                            let ::core::option::Option::Some(component) =
                                world.get_component::<#ident>(entity)
                            else {
                                continue;
                            };
                            let context = frame.context(world, entity);
                            // A `gizmo_when = "selected"` declaration draws
                            // only what the editor has selected.
                            if #gizmo_selected_only && !context.selected {
                                continue;
                            }
                            #gizmo_fn(component, &context, draw);
                            drawn += 1;
                        }
                    }
                    drawn
                }
            };
            let when = if gizmo_selected_only {
                quote! { ::pill_engine::gizmos::GizmoWhen::ForSelected }
            } else {
                quote! { ::pill_engine::gizmos::GizmoWhen::Always }
            };
            let descriptor = quote! {
                ::core::option::Option::Some(::pill_engine::gizmos::GizmoDeclaration {
                    type_name: #type_name,
                    owner: ::core::env!("CARGO_PKG_NAME"),
                    when: #when,
                    draw: #wrapper_fn_name,
                })
            };
            (wrapper, descriptor)
        }
        None => (quote! {}, quote! { ::core::option::Option::None }),
    };

    let expanded = quote! {
        impl ::pill_engine::Component for #ident {
            #shared_name_impl

            fn declared_schema_hash() -> ::core::option::Option<u64> {
                // These are the same descriptors the registration hashes, so a
                // type's declared hash always agrees with its registration. An
                // empty layout declares nothing, which matches
                // `ComponentRegistration::of`.
                let fields: &[::pill_engine::component_registry::ComponentFieldDescriptor] =
                    #layout_reference;
                (!fields.is_empty())
                    .then(|| ::pill_engine::component::component_schema_hash(fields))
            }
        }
        #declared_layout

        #field_enum

        #field_accessors

        #gizmo_wrapper

        /// Registers this component into the world; used by the artifact-wide
        /// registration loop generated for the module/project entry point.
        #[allow(non_snake_case)]
        fn #register_fn_name(world: &mut ::pill_engine::World) {
            #registration_call
            #asset_field_records
        }

        ::pill_engine::submit! {
            ::pill_engine::component_registry::PillComponentDescriptor {
                type_name: #type_name,
                persistable: #persistable,
                fields: #layout_reference,
                register: #register_fn_name,
                gizmo: #gizmo_descriptor,
            }
        }
    };

    expanded.into()
}

/// The registration statements that record each top-level `Handle<T>` field's
/// asset type: `T`'s shared name, or its Rust type name without one.
///
/// Fields of a struct that is not a named-field struct have no handle records;
/// the layout code reports those shapes itself.
fn handle_field_records(
    input: &DeriveInput,
    type_ident: &syn::Ident,
) -> syn::Result<proc_macro2::TokenStream> {
    let syn::Data::Struct(data) = &input.data else {
        return Ok(quote! {});
    };
    let syn::Fields::Named(fields) = &data.fields else {
        return Ok(quote! {});
    };
    let mut asset_field_records = Vec::new();
    for field in &fields.named {
        let Some(asset) = handle_type_argument(&field.ty) else {
            continue;
        };
        let field_name = field
            .ident
            .as_ref()
            .expect("named field has an identifier")
            .to_string();
        asset_field_records.push(quote! {
            world.record_asset_field_type(
                ::pill_engine::ComponentId::of::<#type_ident>(),
                #field_name,
                <#asset as ::pill_engine::asset::Asset>::shared_name()
                    .unwrap_or_else(::core::any::type_name::<#asset>),
            );
        });
    }
    Ok(quote! { #(#asset_field_records)* })
}

/// The `T` of a `Handle<T>` type, written with any path prefix.
fn handle_type_argument(field_type: &syn::Type) -> Option<&syn::Type> {
    let syn::Type::Path(path) = field_type else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Handle" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    arguments.args.iter().find_map(|argument| match argument {
        syn::GenericArgument::Type(asset) => Some(asset),
        _ => None,
    })
}

// =============================================================================
// #[derive(PillLayout)]
// =============================================================================

/// Emits a type's compile-time field layout without the component
/// registration machinery.
///
/// Generates the same descriptor list as [`PillComponent`], built from
/// `offset_of!`, `size_of` and `align_of` over the same tag vocabulary. The
/// list appears as an inherent `FIELD_LAYOUT` const plus a [`ComponentLayout`]
/// impl.
///
/// Use it for a type that registers itself explicitly: an engine type, a
/// host-linked extension, or anything else that must not submit an inventory
/// registration into every artifact that links it.
///
/// A nested struct field is tagged `struct:<path>`. The world flattens those
/// into dotted leaf rows at registration, provided the nested type's own layout
/// is registered first.
///
/// [`ComponentLayout`]: ::pill_engine::component_registry::ComponentLayout
#[proc_macro_derive(PillLayout, attributes(pill))]
pub fn derive_pill_layout(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &input.generics,
            "`PillLayout` cannot be derived for a generic type",
        )
        .to_compile_error()
        .into();
    }

    // A layout-only type emits no accessors. It has no registration that could
    // carry heap-field trampolines.
    match component_field_descriptors(&input, &input.ident, LayoutSubject::LayoutOnly) {
        Ok((declared_layout, _layout_reference, _accessors, _field_enum)) => declared_layout.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

/// The inherent `FIELD_LAYOUT` const and `ComponentLayout` impl every
/// layout-carrying derive emits, both naming the same descriptor slice.
fn layout_emission(
    type_ident: &syn::Ident,
    layout_reference: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        impl #type_ident {
            /// Compile-time field layout of this type, generated by the derive
            /// macro; pass it to `World::register_component_with_layout`.
            pub const FIELD_LAYOUT: &'static [::pill_engine::component_registry::ComponentFieldDescriptor] = #layout_reference;
        }

        impl ::pill_engine::component_registry::ComponentLayout for #type_ident {
            const FIELDS: &'static [::pill_engine::component_registry::ComponentFieldDescriptor] = #layout_reference;
        }
    }
}

/// The layout emission for a type with no named fields (unit struct, tuple
/// struct, enum, union), so the const and trait impl exist on every derived
/// type.
fn empty_field_layout(type_ident: &syn::Ident) -> proc_macro2::TokenStream {
    layout_emission(type_ident, &quote! { &[] })
}

/// Which derive is asking for field descriptors, and so what the emission may
/// contain.
///
/// The four variants are exactly the four callers, so every call site says what
/// it wants. Two `bool` flags could not express whether the caller keeps the
/// accessor stream.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LayoutSubject {
    /// `#[derive(PillComponent)]`: container fields allowed, heap-field
    /// accessors emitted, layout consts emitted.
    Component,
    /// `#[derive(PillLayout)]`: container fields allowed, no accessors - a
    /// layout-only type has no registration that could carry trampolines - and
    /// layout consts emitted.
    LayoutOnly,
    /// A `#[derive(PillMirror)]` value type: no container fields (it crosses
    /// the boundary as plain data), no accessors, layout consts emitted.
    MirroredValue,
    /// `pill_value_type!` on a foreign type: no container fields, no accessors,
    /// and no consts (an inherent const or trait impl would violate the orphan
    /// rule).
    ForeignValue,
}

impl LayoutSubject {
    /// Whether a `Vec`/`String`-shaped field may appear at all.
    fn allows_containers(self) -> bool {
        matches!(self, Self::Component | Self::LayoutOnly)
    }

    /// Whether the heap-field accessor trampolines are emitted.
    fn emits_accessors(self) -> bool {
        matches!(self, Self::Component)
    }

    /// Whether the inherently named field enum is emitted.
    fn emits_field_enum(self) -> bool {
        matches!(self, Self::Component)
    }

    /// Whether the inherent `FIELD_LAYOUT` const and the `ComponentLayout`
    /// impl are emitted.
    fn emits_layout_consts(self) -> bool {
        !matches!(self, Self::ForeignValue)
    }
}

/// Emit a `static` carrying one [`ComponentFieldDescriptor`] per named field,
/// the expression that names it, the heap-field accessor machinery when the
/// struct declares container fields, and the field enum when the subject is a
/// component.
///
/// `self_type` is the path every offset is taken through: the derived type's
/// own name for the derives, or the declared path when `pill_value_type!`
/// describes a foreign type. `layout_subject` says which derive is asking, and
/// that is what decides container fields, accessors, consts and the enum.
///
/// Enums, unions, unit structs and tuple structs have no named fields to mirror
/// and produce an empty layout.
fn component_field_descriptors(
    input: &DeriveInput,
    self_type: &dyn ToTokens,
    layout_subject: LayoutSubject,
) -> syn::Result<(
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
    proc_macro2::TokenStream,
)> {
    let type_ident = &input.ident;
    // Step 1: Any type without named fields has nothing to mirror, so it gets
    // an empty layout.
    let named_fields = match &input.data {
        syn::Data::Struct(data) => match &data.fields {
            syn::Fields::Named(fields) => &fields.named,
            _ => {
                return Ok((
                    empty_field_layout(type_ident),
                    quote! { &[] },
                    quote! {},
                    quote! {},
                ))
            }
        },
        _ => {
            return Ok((
                empty_field_layout(type_ident),
                quote! { &[] },
                quote! {},
                quote! {},
            ))
        }
    };
    if named_fields.is_empty() {
        return Ok((
            empty_field_layout(type_ident),
            quote! { &[] },
            quote! {},
            quote! {},
        ));
    }

    let type_name = quote! {
        ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#type_ident))
    };

    let mut field_descriptor_expressions = Vec::with_capacity(named_fields.len());
    let mut accessor_emissions: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut field_enum_variants: Vec<FieldEnumVariant> = Vec::new();
    // Step 2: Build one descriptor per field, one accessor set per container
    // field, and the field enum's variants for a component.
    for field in named_fields {
        let field_ident = field.ident.as_ref().expect("named field has an identifier");
        let field_type = &field.ty;
        let field_name = field_ident.to_string();
        let tag = field_type_tag(field_type, layout_subject.allows_containers())?;
        let type_tag = &tag.tag_text;
        let decorators = parse_field_decorators(&field.attrs)?;
        // The enum is built from this same walk, so a variant's name and tag
        // cannot disagree with the descriptor beside it.
        if layout_subject.emits_field_enum() {
            let variant_ident = field_variant_ident(field_ident)?;
            if let Some(known) = field_enum_variants
                .iter()
                .find(|known| known.variant_ident == variant_ident)
            {
                return Err(syn::Error::new(
                    field_ident.span(),
                    format!(
                        "`{}` and `{}` both generate the field enum variant `{}`; \
                         rename one field",
                        known.field_name, field_name, variant_ident
                    ),
                ));
            }
            field_enum_variants.push(FieldEnumVariant {
                variant_ident,
                field_name: field_name.clone(),
                tag_text: tag.tag_text.clone(),
            });
        }
        // An array length may be a literal or a `const` path. Both work in a
        // const context, so the count is emitted as an expression instead of
        // being parsed here.
        let element_count = match field_type {
            syn::Type::Array(array) => {
                let array_len = &array.len;
                quote! { (#array_len as usize) }
            }
            _ => quote! { 0usize },
        };
        field_descriptor_expressions.push(quote! {
            ::pill_engine::component_registry::ComponentFieldDescriptor {
                name: #field_name,
                type_tag: #type_tag,
                offset: ::core::mem::offset_of!(#self_type, #field_ident),
                size: ::core::mem::size_of::<#field_type>(),
                align: ::core::mem::align_of::<#field_type>(),
                element_count: #element_count,
                decorators: #decorators,
            }
        });
        // A container field also gets one accessor per supported operation, so
        // a managed mirror reads and writes the live buffer instead of copying
        // the container's header somewhere else.
        if layout_subject.emits_accessors() {
            if let Some(kind) = tag.heap_field_kind {
                accessor_emissions.push(emit_heap_field_accessor(
                    type_ident,
                    field_ident,
                    &field_name,
                    &type_name,
                    kind,
                ));
            }
        }
    }

    // Step 3: Name the fields in code, when the subject is a component. The
    // enum is the code-side view of the descriptors just built.
    let field_enum = if layout_subject.emits_field_enum() {
        field_enum_emission(type_ident, &field_enum_variants)
    } else {
        quote! {}
    };
    // Step 4: Put the descriptors behind one `static`, and add the layout
    // consts when this subject may carry them.
    let static_name = format_ident!("__PILL_FIELD_LAYOUT_{}", type_ident);
    // A foreign type can carry neither an inherent const nor the
    // `ComponentLayout` impl (both are orphan-rule violations), so its
    // declaration emits the descriptor list alone.
    let layout_consts = if layout_subject.emits_layout_consts() {
        layout_emission(type_ident, &quote! { #static_name })
    } else {
        quote! {}
    };
    let declared_layout = quote! {
        /// Compile-time field layout of `#type_ident`, consumed by the C# mirror
        /// codegen. Do not reference; generated by `#[derive(...)]`.
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        static #static_name: &[::pill_engine::component_registry::ComponentFieldDescriptor] =
            &[ #(#field_descriptor_expressions),* ];

        #layout_consts
    };
    Ok((
        declared_layout,
        quote! { #static_name },
        quote! { #(#accessor_emissions)* },
        field_enum,
    ))
}

/// One variant of a component's generated field enum: the variant, the field
/// name it answers to, and the tag its descriptor carries.
struct FieldEnumVariant {
    /// The enum variant's identifier.
    variant_ident: syn::Ident,
    /// The field's Rust name, as its descriptor spells it.
    field_name: String,
    /// The field's type tag, as its descriptor carries it.
    tag_text: String,
}

/// The variant name for one field: the field's name in PascalCase.
///
/// A raw identifier (`r#type`) keeps the name after its prefix. Two fields can
/// map to one variant (`foo_bar` and `foo__bar`), which the caller refuses
/// rather than generating an enum that cannot compile.
fn field_variant_ident(field_ident: &syn::Ident) -> syn::Result<syn::Ident> {
    let field_text = field_ident.to_string();
    let field_name = field_text.strip_prefix("r#").unwrap_or(&field_text);
    let mut variant_name = String::new();
    for segment in field_name.split('_').filter(|segment| !segment.is_empty()) {
        let mut characters = segment.chars();
        if let Some(first) = characters.next() {
            variant_name.extend(first.to_uppercase());
            variant_name.extend(characters);
        }
    }
    if variant_name.is_empty() {
        return Err(syn::Error::new(
            field_ident.span(),
            "the generated field enum needs a field name with letters in it",
        ));
    }
    Ok(format_ident!("{}", variant_name, span = field_ident.span()))
}

/// The `<Type>Field` enum and the `ComponentFields` link that names it from
/// the component type.
///
/// The enum's inherent methods answer from its trait impl, so a direct call, a
/// generic bound and the generated code all read one list.
fn field_enum_emission(
    type_ident: &syn::Ident,
    variants: &[FieldEnumVariant],
) -> proc_macro2::TokenStream {
    let enum_ident = format_ident!("{}Field", type_ident);
    let variant_ident_storage: Vec<&syn::Ident> = variants
        .iter()
        .map(|variant| &variant.variant_ident)
        .collect();
    let field_name_storage: Vec<&str> = variants
        .iter()
        .map(|variant| variant.field_name.as_str())
        .collect();
    let tag_text_storage: Vec<&str> = variants
        .iter()
        .map(|variant| variant.tag_text.as_str())
        .collect();
    // Every list is spliced more than once (the enum, the name table and the
    // trait impl), so the macro reads them through slices whose iterators
    // clone.
    let variant_idents = variant_ident_storage.as_slice();
    let field_names = field_name_storage.as_slice();
    let tag_texts = tag_text_storage.as_slice();
    quote! {
        /// The fields of this component, named in code; generated by
        /// `#[derive(PillComponent)]`.
        ///
        /// One variant per field, in declaration order. `name` is the name the
        /// field layer and the editor's commands spell, and `from_name` maps
        /// that name back to the variant.
        #[allow(dead_code)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum #enum_ident {
            #(
                #[doc = ::core::concat!("The `", #field_names, "` field.")]
                #variant_idents,
            )*
        }

        #[allow(dead_code)]
        impl #enum_ident {
            /// Every field, in the order the type declares them.
            pub const ALL: &'static [#enum_ident] =
                &[ #( Self::#variant_idents ),* ];

            /// The field's Rust name, as the field layer spells it.
            pub fn name(self) -> &'static str {
                <Self as ::pill_engine::ComponentFieldId>::name(self)
            }

            /// The field `name` names, or `None` when the type has no such
            /// field.
            pub fn from_name(name: &str) -> ::core::option::Option<Self> {
                <Self as ::pill_engine::ComponentFieldId>::from_name(name)
            }

            /// The field's registered type tag, as its descriptor reports it.
            pub fn value_tag(self) -> &'static str {
                <Self as ::pill_engine::ComponentFieldId>::value_tag(self)
            }
        }

        impl ::pill_engine::ComponentFieldId for #enum_ident {
            const ALL: &'static [Self] = &[ #( Self::#variant_idents ),* ];

            fn name(self) -> &'static str {
                match self {
                    #( Self::#variant_idents => #field_names, )*
                }
            }

            fn from_name(name: &str) -> ::core::option::Option<Self> {
                match name {
                    #( #field_names => ::core::option::Option::Some(Self::#variant_idents), )*
                    _ => ::core::option::Option::None,
                }
            }

            fn value_tag(self) -> &'static str {
                match self {
                    #( Self::#variant_idents => #tag_texts, )*
                }
            }
        }

        impl ::pill_engine::ComponentFields for #type_ident {
            type Field = #enum_ident;
        }
    }
}

/// The argument list of a decorator written with parentheses, e.g.
/// `range(0.0, 100.0)`.
type DecoratorArguments = syn::punctuated::Punctuated<syn::Lit, syn::Token![,]>;

/// Every field decorator a field may declare, with the usage text a refusal
/// shows.
///
/// The parser and both refusal messages read this one list, so they cannot
/// drift apart. A listed decorator the parser does not accept fails the test at
/// the bottom of the file, and so does one the refusal text stops naming. A
/// decorator with two forms (with or without parentheses) appears once per
/// form.
const FIELD_DECORATOR_USAGE: &[(&str, &str)] = &[
    ("hide_in_inspector", "hide_in_inspector"),
    ("color", "color"),
    ("vector", "vector"),
    ("header", "header(\"Text\")"),
    ("tooltip", "tooltip(\"Text\")"),
    ("space", "space(10.0)"),
    ("min", "min(0.0)"),
    ("range", "range(0.0, 100.0)"),
    ("text_area", "text_area"),
    ("text_area", "text_area(3, 10)"),
    ("multiline", "multiline(5)"),
];

/// The decorator vocabulary as one sentence list, for a refusal message.
fn field_decorator_list() -> String {
    FIELD_DECORATOR_USAGE
        .iter()
        .map(|(_, usage)| format!("`{usage}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse the `#[pill(...)]` decorators a field declares into the expression
/// the descriptor emission stores in `decorators`.
///
/// A field that declares none yields an empty borrowed slice, so the descriptor
/// literal stays const-constructible.
fn parse_field_decorators(attributes: &[syn::Attribute]) -> syn::Result<proc_macro2::TokenStream> {
    let mut decorators: Vec<proc_macro2::TokenStream> = Vec::new();
    for attribute in attributes {
        if !attribute.path().is_ident("pill") {
            continue;
        }
        attribute.parse_nested_meta(|meta| add_field_decorator(&meta, &mut decorators))?;
    }
    Ok(quote! {
        ::std::borrow::Cow::Borrowed(&[ #(#decorators),* ])
    })
}

/// Append one parsed field decorator to `decorators`, or refuse the key with
/// the set a field actually takes.
fn add_field_decorator(
    meta: &syn::meta::ParseNestedMeta<'_>,
    decorators: &mut Vec<proc_macro2::TokenStream>,
) -> syn::Result<()> {
    let path = &meta.path;
    if path.is_ident("hide_in_inspector") {
        decorators.push(quote! {
            ::pill_engine::component_registry::FieldDecorator::HideInInspector
        });
        return Ok(());
    }
    // The two grouping decorators take no argument.
    if path.is_ident("color") || path.is_ident("vector") {
        decorators.push(if path.is_ident("color") {
            quote! { ::pill_engine::component_registry::FieldDecorator::Color }
        } else {
            quote! { ::pill_engine::component_registry::FieldDecorator::Vector }
        });
        return Ok(());
    }
    if path.is_ident("header") || path.is_ident("tooltip") {
        let key = if path.is_ident("header") {
            "header"
        } else {
            "tooltip"
        };
        let argument = single_argument(meta, key, "header(\"Text\")")?;
        let syn::Lit::Str(text) = argument else {
            return Err(syn::Error::new_spanned(
                argument,
                format!("`{key}` takes a string, as in {key}(\"Text\")"),
            ));
        };
        decorators.push(if path.is_ident("header") {
            quote! {
                ::pill_engine::component_registry::FieldDecorator::Header(
                    ::std::borrow::Cow::Borrowed(#text)
                )
            }
        } else {
            quote! {
                ::pill_engine::component_registry::FieldDecorator::Tooltip(
                    ::std::borrow::Cow::Borrowed(#text)
                )
            }
        });
        return Ok(());
    }
    if path.is_ident("space") || path.is_ident("min") {
        let key = if path.is_ident("space") {
            "space"
        } else {
            "min"
        };
        let usage = if path.is_ident("space") {
            "space(10.0)"
        } else {
            "min(0.0)"
        };
        let argument = single_argument(meta, key, usage)?;
        let numeric_argument = literal_as_f32(&argument, key)?;
        if !numeric_argument.is_finite() || (path.is_ident("space") && numeric_argument < 0.0) {
            return Err(syn::Error::new_spanned(
                argument,
                format!("`{key}` takes a finite, non-negative number"),
            ));
        }
        decorators.push(if path.is_ident("space") {
            quote! { ::pill_engine::component_registry::FieldDecorator::Space(#numeric_argument) }
        } else {
            quote! { ::pill_engine::component_registry::FieldDecorator::Min(#numeric_argument) }
        });
        return Ok(());
    }
    if path.is_ident("range") {
        let arguments = decorator_arguments(meta, "range", "range(0.0, 100.0)")?;
        let [min_literal, max_literal] =
            arguments_of_two(&arguments, "range", "range(0.0, 100.0)")?;
        let min = literal_as_f32(min_literal, "range")?;
        let max = literal_as_f32(max_literal, "range")?;
        if !min.is_finite() || !max.is_finite() || min > max {
            return Err(syn::Error::new_spanned(
                min_literal,
                "`range` needs finite bounds with the minimum first, as in range(0.0, 100.0)",
            ));
        }
        decorators.push(quote! {
            ::pill_engine::component_registry::FieldDecorator::Range { min: #min, max: #max }
        });
        return Ok(());
    }
    if path.is_ident("text_area") {
        // Unity's parameterless `[TextArea]` opens a three-row box.
        if !meta.input.peek(syn::token::Paren) {
            decorators.push(quote! {
                ::pill_engine::component_registry::FieldDecorator::TextArea {
                    min_lines: 3usize,
                    max_lines: 3usize,
                }
            });
            return Ok(());
        }
        let arguments = decorator_arguments(meta, "text_area", "text_area(3, 10)")?;
        let [min_literal, max_literal] =
            arguments_of_two(&arguments, "text_area", "text_area(3, 10)")?;
        let min_lines = literal_as_usize(min_literal, "text_area")?;
        let max_lines = literal_as_usize(max_literal, "text_area")?;
        if min_lines == 0 || min_lines > max_lines {
            return Err(syn::Error::new_spanned(
                min_literal,
                "`text_area` needs at least one line with the minimum count first, as in text_area(3, 10)",
            ));
        }
        decorators.push(quote! {
            ::pill_engine::component_registry::FieldDecorator::TextArea {
                min_lines: #min_lines,
                max_lines: #max_lines,
            }
        });
        return Ok(());
    }
    if path.is_ident("multiline") {
        let argument = single_argument(meta, "multiline", "multiline(5)")?;
        let line_count = literal_as_usize(&argument, "multiline")?;
        if line_count == 0 {
            return Err(syn::Error::new_spanned(
                argument,
                "`multiline` needs at least one line, as in multiline(5)",
            ));
        }
        decorators.push(quote! {
            ::pill_engine::component_registry::FieldDecorator::Multiline(#line_count)
        });
        return Ok(());
    }
    if path.is_ident("persistable") || path.is_ident("shared") {
        return Err(syn::Error::new_spanned(
            path,
            format!(
                "`{}` declares the type, not a field; a field takes one of the field \
                 decorators ({})",
                path.to_token_stream(),
                field_decorator_list()
            ),
        ));
    }
    Err(syn::Error::new_spanned(
        path,
        format!(
            "unknown `pill` field decorator; expected {}",
            field_decorator_list()
        ),
    ))
}

/// Read a decorator's parenthesised argument list, insisting the parentheses
/// are there so `#[pill(min)]` names its usage instead of failing on a token.
fn decorator_arguments(
    meta: &syn::meta::ParseNestedMeta<'_>,
    key: &str,
    usage: &str,
) -> syn::Result<DecoratorArguments> {
    if !meta.input.peek(syn::token::Paren) {
        return Err(syn::Error::new_spanned(
            meta.path.clone(),
            format!("`{key}` takes arguments, as in {usage}"),
        ));
    }
    let content;
    syn::parenthesized!(content in meta.input);
    content.call(DecoratorArguments::parse_terminated)
}

/// The one argument of a single-argument decorator, or a usage error.
fn single_argument(
    meta: &syn::meta::ParseNestedMeta<'_>,
    key: &str,
    usage: &str,
) -> syn::Result<syn::Lit> {
    let arguments = decorator_arguments(meta, key, usage)?;
    let mut literals = arguments.into_iter();
    let (Some(only), None) = (literals.next(), literals.next()) else {
        return Err(syn::Error::new_spanned(
            meta.path.clone(),
            format!("`{key}` takes one argument, as in {usage}"),
        ));
    };
    Ok(only)
}

/// The two arguments of a two-argument decorator, or a usage error.
fn arguments_of_two<'a>(
    arguments: &'a DecoratorArguments,
    key: &str,
    usage: &str,
) -> syn::Result<[&'a syn::Lit; 2]> {
    let mut literals = arguments.iter();
    let (Some(first), Some(second), None) = (literals.next(), literals.next(), literals.next())
    else {
        return Err(syn::Error::new_spanned(
            arguments,
            format!("`{key}` takes two arguments, as in {usage}"),
        ));
    };
    Ok([first, second])
}

/// Read a numeric decorator argument, accepting an integer literal for a
/// `f32` parameter (`space(10)` as well as `space(10.0)`).
fn literal_as_f32(literal: &syn::Lit, key: &str) -> syn::Result<f32> {
    match literal {
        syn::Lit::Float(float_literal) => float_literal.base10_parse::<f32>(),
        syn::Lit::Int(integer_literal) => integer_literal.base10_parse::<f32>(),
        _ => Err(syn::Error::new_spanned(
            literal,
            format!("`{key}` takes a number"),
        )),
    }
}

/// Read a line-count decorator argument, insisting it is a plain integer.
fn literal_as_usize(literal: &syn::Lit, key: &str) -> syn::Result<usize> {
    match literal {
        syn::Lit::Int(integer_literal) => integer_literal.base10_parse::<usize>(),
        _ => Err(syn::Error::new_spanned(
            literal,
            format!("`{key}` takes a whole line count"),
        )),
    }
}

/// What the managed side needs to know about one heap-owning field.
enum HeapFieldKind {
    /// A `Vec<E>` field; the element tag drives the generated span type.
    Vec {
        /// Element type tag (`f32`, `struct:<path>`, ...).
        element_tag: String,
    },
    /// A `DynamicBuffer<E>` field: elements in engine-owned native memory,
    /// whose `(ptr, len, cap)` handle the mirror reads straight out of the
    /// row.
    DynamicBuffer {
        /// Element type tag (`f32`, `struct:<path>`, ...).
        element_tag: String,
    },
    /// A `Vec<String>` field: elements are individually allocated, so managed
    /// code reaches each one through its own accessor instead of a span.
    VecString,
    /// A `String` field, exposed to C# as UTF-8 text.
    String,
}

/// A field's wire tag, together with the accessor set the tag implies.
///
/// The code that builds the tag already decides whether the field is a
/// container and what its element is. Carrying that decision out with the text
/// means no second function has to strip the prefixes off again. A mistake
/// there would emit a `vec:f32` row with no accessor trampolines.
struct FieldTag {
    /// The tag the descriptor carries (`f32`, `vec:f32`, `struct:<path>`, ...).
    tag_text: String,
    /// The accessor set the tag implies, when the field owns heap memory.
    heap_field_kind: Option<HeapFieldKind>,
}

impl FieldTag {
    /// A tag with no accessor trampolines behind it.
    fn plain(tag_text: String) -> Self {
        Self {
            tag_text,
            heap_field_kind: None,
        }
    }
}

/// One trampoline an accessor kind offers, and what the descriptor says about
/// it.
///
/// [`emit_heap_field_accessor`] decides the list, so which operations a
/// container has is visible in one place.
struct AccessorEmission {
    /// The operation the descriptor names it by (`view`, `resize`, ...).
    operation: &'static str,
    /// The exported symbol the host resolves.
    symbol: String,
    /// The trampoline itself.
    trampoline: proc_macro2::TokenStream,
}

/// Emits the resize trampoline that every container kind shares.
///
/// `Vec`, `DynamicBuffer` and `Vec<String>` all rewrite their length the same
/// way: they grow with `Default` and refuse a null row. They differ only in
/// what the documentation says the rewrite does to outstanding views, so the
/// doc lines arrive as the caller's token stream and the body is written once.
///
/// Resizing a `Vec` needs `E: Default + Clone`. Those bounds land on the
/// field's element type (a primitive, a `String` or a mirrored struct), not on
/// the component.
fn emit_resize_trampoline(
    type_ident: &syn::Ident,
    field_ident: &syn::Ident,
    doc: proc_macro2::TokenStream,
) -> AccessorEmission {
    let symbol = format!("pill_accessor_{type_ident}_{field_ident}_resize");
    let resize_fn = syn::Ident::new(&symbol, field_ident.span());
    let trampoline = quote! {
        #doc
        #[doc(hidden)]
        #[no_mangle]
        pub unsafe extern "C" fn #resize_fn(row: *mut u8, new_length: usize) -> u8 {
            if row.is_null() {
                return 1;
            }
            // SAFETY: the caller guarantees `row` addresses a live component
            // value and holds the write declaration the mutation needs, so the
            // exclusive reference is valid for the whole call.
            let component = unsafe { &mut *(row as *mut #type_ident) };
            component.#field_ident.resize(
                new_length,
                ::core::default::Default::default(),
            );
            0
        }
    };
    AccessorEmission {
        operation: "resize",
        symbol,
        trampoline,
    }
}

/// Emits the accessor trampolines and the descriptor submission for one
/// heap-owning field.
///
/// Every trampoline receives the address of the live component value, which is
/// the row a managed query is iterating. The view reports `(data, length)` for
/// any container, resize grows or shrinks a container, and set replaces a
/// `String`'s contents from UTF-8 bytes. Nothing here serializes or copies a
/// buffer: the managed side reads and writes elements through the pointer it
/// receives.
fn emit_heap_field_accessor(
    type_ident: &syn::Ident,
    field_ident: &syn::Ident,
    field_name: &str,
    type_name: &proc_macro2::TokenStream,
    kind: HeapFieldKind,
) -> proc_macro2::TokenStream {
    let view_symbol = format!("pill_accessor_{type_ident}_{field_ident}_view");
    let view_fn = syn::Ident::new(&view_symbol, field_ident.span());

    // A `Vec<String>` has no contiguous run of elements to point at. Each
    // element is its own allocation, so the view reports the count and a null
    // data pointer, and the elements are reached through their own trampolines
    // below.
    let view_data = match &kind {
        HeapFieldKind::VecString => quote! {
            *out_data = ::core::ptr::null();
        },
        _ => quote! {
            *out_data = values.as_ptr().cast::<u8>();
        },
    };

    let view = quote! {
        /// View trampoline for the `#field_ident` field, generated by
        /// `#[derive(PillComponent)]`.
        ///
        /// Writes what one borrowed view of the field can offer: the element
        /// address and count for a contiguous run, or a null address and the
        /// count for a `Vec<String>`, whose elements are separately allocated
        /// and have no span. Returns 0, or returns 1 when a pointer is null.
        /// The buffer stays owned by the component: the caller may read (and,
        /// for a `Vec`, write) elements until the container is structurally
        /// modified.
        ///
        /// # Safety
        ///
        /// `row` must point at a live `#type_ident` value, and both out
        /// pointers must be writable.
        #[doc(hidden)]
        #[no_mangle]
        pub unsafe extern "C" fn #view_fn(
            row: *const u8,
            out_data: *mut *const u8,
            out_length: *mut usize,
        ) -> u8 {
            if row.is_null() || out_data.is_null() || out_length.is_null() {
                return 1;
            }
            // SAFETY: the caller guarantees `row` addresses a live component
            // value, so the shared reference is valid for the whole call.
            let component = unsafe { &*(row as *const #type_ident) };
            let values = &component.#field_ident;
            // SAFETY: both out pointers were checked non-null above.
            unsafe {
                #view_data
                *out_length = values.len();
            }
            0
        }
    };

    let mut emissions: Vec<AccessorEmission> = Vec::new();
    let (kind_tag, element_tag) = match &kind {
        HeapFieldKind::Vec { element_tag } => {
            emissions.push(emit_resize_trampoline(
                type_ident,
                field_ident,
                quote! {
                    /// Resize trampoline for the `#field_ident` field, generated by
                    /// `#[derive(PillComponent)]`.
                    ///
                    /// Rewrites the vector to `new_length` elements and returns 0,
                    /// or returns 1 when `row` is null. Growing fills the new
                    /// elements with the element type's `Default`. Existing
                    /// elements keep their values; the buffer may move, so a view
                    /// taken before the call must not be used afterwards.
                    ///
                    /// # Safety
                    ///
                    /// `row` must point at a live `#type_ident` value the caller
                    /// holds write access to.
                },
            ));
            ("vec", element_tag.as_str())
        }
        HeapFieldKind::DynamicBuffer { element_tag } => {
            emissions.push(emit_resize_trampoline(
                type_ident,
                field_ident,
                quote! {
                    /// Resize trampoline for the `#field_ident` buffer, generated
                    /// by `#[derive(PillComponent)]`.
                    ///
                    /// Rewrites the buffer to `new_length` elements and returns 0,
                    /// or returns 1 when `row` is null. Growing fills the new
                    /// elements with the element type's `Default`. This is the one
                    /// operation that can retire the block, so it ends every
                    /// outstanding view of it; the handle is re-read from the row
                    /// afterwards.
                    ///
                    /// # Safety
                    ///
                    /// `row` must point at a live `#type_ident` value the caller
                    /// holds write access to.
                },
            ));
            ("dynbuf", element_tag.as_str())
        }
        HeapFieldKind::VecString => {
            emissions.push(emit_resize_trampoline(
                type_ident,
                field_ident,
                quote! {
                    /// Resize trampoline for the `#field_ident` list, generated by
                    /// `#[derive(PillComponent)]`.
                    ///
                    /// Rewrites the list to `new_length` elements and returns 0,
                    /// or returns 1 when `row` is null. Growing fills the new
                    /// elements with empty strings. This is the one operation that
                    /// can move the outer allocation, so it ends every outstanding
                    /// element pointer.
                    ///
                    /// # Safety
                    ///
                    /// `row` must point at a live `#type_ident` value the caller
                    /// holds write access to.
                },
            ));
            let item_symbol = format!("pill_accessor_{type_ident}_{field_ident}_item");
            let set_item_symbol = format!("pill_accessor_{type_ident}_{field_ident}_set_item");
            let push_symbol = format!("pill_accessor_{type_ident}_{field_ident}_push");
            let item_fn = syn::Ident::new(&item_symbol, field_ident.span());
            let set_item_fn = syn::Ident::new(&set_item_symbol, field_ident.span());
            let push_fn = syn::Ident::new(&push_symbol, field_ident.span());
            emissions.push(AccessorEmission {
                operation: "item",
                symbol: item_symbol,
                trampoline: quote! {
                /// Element-view trampoline for the `#field_ident` list,
                /// generated by `#[derive(PillComponent)]`.
                ///
                /// Writes element `index`'s UTF-8 bytes through
                /// `out_data`/`out_length` and returns 0, returns 1 when a
                /// pointer is null, or returns 2 when `index` is out of
                /// range. The bytes borrow the element only for the call.
                ///
                /// # Safety
                ///
                /// `row` must point at a live `#type_ident` value, and both
                /// out pointers must be writable.
                #[doc(hidden)]
                #[no_mangle]
                pub unsafe extern "C" fn #item_fn(
                    row: *const u8,
                    index: usize,
                    out_data: *mut *const u8,
                    out_length: *mut usize,
                ) -> u8 {
                    if row.is_null() || out_data.is_null() || out_length.is_null() {
                        return 1;
                    }
                    // SAFETY: the caller guarantees `row` addresses a live
                    // component value, so the shared reference is valid for
                    // the whole call.
                    let component = unsafe { &*(row as *const #type_ident) };
                    let Some(text) = component.#field_ident.get(index) else {
                        return 2;
                    };
                    // SAFETY: both out pointers were checked non-null above,
                    // and `text` borrows the live element for this call.
                    unsafe {
                        *out_data = text.as_ptr();
                        *out_length = text.len();
                    }
                    0
                }
                },
            });
            emissions.push(AccessorEmission {
                operation: "set_item",
                symbol: set_item_symbol,
                trampoline: quote! {
                /// Element-replace trampoline for the `#field_ident` list,
                /// generated by `#[derive(PillComponent)]`.
                ///
                /// Replaces element `index` with the UTF-8 bytes
                /// `utf8..utf8+length` and returns 0, returns 1 for a null
                /// pointer, returns 2 when `index` is out of range, or returns
                /// 3 when the bytes are not valid UTF-8.
                ///
                /// # Safety
                ///
                /// `row` must point at a live `#type_ident` value the caller
                /// holds write access to, and `utf8` must be readable for
                /// `length` bytes (it may be null only when `length` is zero).
                #[doc(hidden)]
                #[no_mangle]
                pub unsafe extern "C" fn #set_item_fn(
                    row: *mut u8,
                    index: usize,
                    utf8: *const u8,
                    length: usize,
                ) -> u8 {
                    if row.is_null() || (utf8.is_null() && length != 0) {
                        return 1;
                    }
                    // SAFETY: the caller promises `utf8` is readable for
                    // `length` bytes; the slice borrows only for this call.
                    let bytes: &[u8] = if length == 0 {
                        &[]
                    } else {
                        unsafe { ::core::slice::from_raw_parts(utf8, length) }
                    };
                    let Ok(text) = ::core::str::from_utf8(bytes) else {
                        return 3;
                    };
                    // SAFETY: the caller guarantees `row` addresses a live
                    // component value the caller holds write access to, so the
                    // exclusive reference is valid for the whole call.
                    let component = unsafe { &mut *(row as *mut #type_ident) };
                    let Some(slot) = component.#field_ident.get_mut(index) else {
                        return 2;
                    };
                    // Reuse the element's allocation rather than replacing it.
                    slot.clear();
                    slot.push_str(text);
                    0
                }
                },
            });
            emissions.push(AccessorEmission {
                operation: "push",
                symbol: push_symbol,
                trampoline: quote! {
                /// Append trampoline for the `#field_ident` list, generated by
                /// `#[derive(PillComponent)]`.
                ///
                /// Appends the UTF-8 bytes `utf8..utf8+length` as a new last
                /// element and returns 0, returns 1 for a null pointer, or
                /// returns 2 when the bytes are not valid UTF-8.
                ///
                /// # Safety
                ///
                /// `row` must point at a live `#type_ident` value the caller
                /// holds write access to, and `utf8` must be readable for
                /// `length` bytes (it may be null only when `length` is zero).
                #[doc(hidden)]
                #[no_mangle]
                pub unsafe extern "C" fn #push_fn(
                    row: *mut u8,
                    utf8: *const u8,
                    length: usize,
                ) -> u8 {
                    if row.is_null() || (utf8.is_null() && length != 0) {
                        return 1;
                    }
                    // SAFETY: the caller promises `utf8` is readable for
                    // `length` bytes; the slice borrows only for this call.
                    let bytes: &[u8] = if length == 0 {
                        &[]
                    } else {
                        unsafe { ::core::slice::from_raw_parts(utf8, length) }
                    };
                    let Ok(text) = ::core::str::from_utf8(bytes) else {
                        return 2;
                    };
                    // SAFETY: as above: `row` addresses a live component value
                    // the caller holds write access to.
                    let component = unsafe { &mut *(row as *mut #type_ident) };
                    component.#field_ident.push(text.to_string());
                    0
                }
                },
            });
            ("vecstring", "string")
        }
        HeapFieldKind::String => {
            let set_symbol = format!("pill_accessor_{type_ident}_{field_ident}_set");
            let set_fn = syn::Ident::new(&set_symbol, field_ident.span());
            emissions.push(AccessorEmission {
                operation: "set",
                symbol: set_symbol,
                trampoline: quote! {
                /// Replace-in-place trampoline for the `#field_ident` field,
                /// generated by `#[derive(PillComponent)]`.
                ///
                /// Replaces the string with the UTF-8 bytes `utf8..utf8+length`
                /// and returns 0, returns 1 for a null pointer, or returns 2
                /// when the bytes are not valid UTF-8. Invalid input is
                /// refused rather than lossily replaced, so an edit can never
                /// silently corrupt the stored text.
                ///
                /// # Safety
                ///
                /// `row` must point at a live `#type_ident` value the caller
                /// holds write access to, and `utf8` must be readable for
                /// `length` bytes (it may be null only when `length` is zero).
                #[doc(hidden)]
                #[no_mangle]
                pub unsafe extern "C" fn #set_fn(
                    row: *mut u8,
                    utf8: *const u8,
                    length: usize,
                ) -> u8 {
                    if row.is_null() || (utf8.is_null() && length != 0) {
                        return 1;
                    }
                    // SAFETY: the caller promises `utf8` is readable for
                    // `length` bytes; the slice borrows only for this call.
                    let bytes: &[u8] = if length == 0 {
                        &[]
                    } else {
                        unsafe { ::core::slice::from_raw_parts(utf8, length) }
                    };
                    let Ok(text) = ::core::str::from_utf8(bytes) else {
                        return 2;
                    };
                    // SAFETY: as in the view trampoline: `row` addresses a live
                    // component value the caller holds write access to.
                    let component = unsafe { &mut *(row as *mut #type_ident) };
                    component.#field_ident.clear();
                    component.#field_ident.push_str(text);
                    0
                }
                },
            });
            ("string", "")
        }
    };

    // The descriptor spells "this kind has no such operation" as an empty
    // symbol, so each field is looked up in the list instead of accumulated.
    let symbol_for_operation = |operation: &str| {
        emissions
            .iter()
            .find(|emission| emission.operation == operation)
            .map(|emission| emission.symbol.clone())
            .unwrap_or_default()
    };
    let resize_symbol = symbol_for_operation("resize");
    let set_symbol = symbol_for_operation("set");
    let item_symbol = symbol_for_operation("item");
    let set_item_symbol = symbol_for_operation("set_item");
    let push_symbol = symbol_for_operation("push");
    let trampolines = emissions.iter().map(|emission| &emission.trampoline);

    quote! {
        #view
        #(#trampolines)*

        ::pill_engine::submit! {
            ::pill_engine::component_registry::PillFieldAccessorDescriptor {
                type_name: #type_name,
                field_name: #field_name,
                kind: #kind_tag,
                element_tag: #element_tag,
                view_symbol: #view_symbol,
                resize_symbol: #resize_symbol,
                set_symbol: #set_symbol,
                item_symbol: #item_symbol,
                set_item_symbol: #set_item_symbol,
                push_symbol: #push_symbol,
            }
        }
    }
}

/// Joins a path's segments into one string, with `separator` between them.
///
/// The result is a wire format in two places. One is the `struct:` tag a
/// component field's nested type carries. The other is the descriptor name of a
/// mirrored declaration, which the C# side matches against the artifact
/// registries. The same function also builds the name of the synthetic struct a
/// mirrored declaration parses through, where only uniqueness matters. Sharing
/// one implementation keeps those spellings from drifting apart.
fn flattened_path(path: &syn::Path, separator: &str) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join(separator)
}

/// Maps a Rust field type to the closed C#-mirror type-tag vocabulary.
///
/// Primitives and fixed-size arrays have a tag of their own. Any other path
/// type is tagged as a nested struct by its fully-qualified name. The codegen
/// resolves that against the artifact's `PillMirror` inventory, and falls back
/// to an opaque blob of the field's size when no declaration exists.
///
/// `String`, `Vec<E>` and `DynamicBuffer<E>` become the heap-owning container
/// tags `string`, `vec:<element>` and `dynbuf:<element>` when the caller allows
/// containers. Component rows allow them, because managed code reaches their
/// buffers through generated accessor members. Mirrored value types do not,
/// because they cross the boundary as plain data.
///
/// A `char` field is rejected: Rust's `char` is 4 bytes and C#'s is 2.
fn field_type_tag(field_type: &syn::Type, containers_allowed: bool) -> syn::Result<FieldTag> {
    match field_type {
        syn::Type::Path(path) if path.qself.is_none() => {
            let segment = path.path.segments.last();
            let last_segment_name = segment
                .map(|segment| segment.ident.to_string())
                .unwrap_or_default();
            match last_segment_name.as_str() {
                "f32" | "f64" | "i8" | "u8" | "i16" | "u16" | "i32" | "u32" | "i64"
                | "u64" | "bool" | "usize" | "isize" => Ok(FieldTag::plain(last_segment_name)),
                "char" => Err(syn::Error::new_spanned(
                    field_type,
                    "`char` fields cannot be mirrored: Rust `char` is 4 bytes while C# `char` is 2",
                )),
                "String" => {
                    if containers_allowed {
                        Ok(FieldTag {
                            tag_text: "string".to_string(),
                            heap_field_kind: Some(HeapFieldKind::String),
                        })
                    } else {
                        Err(heap_owned_rejection(field_type, "String"))
                    }
                }
                "Vec" | "DynamicBuffer" => {
                    if !containers_allowed {
                        return Err(heap_owned_rejection(field_type, &last_segment_name));
                    }
                    // The element type decides the span C# iterates, so it
                    // must be inside the closed vocabulary too.
                    let element = container_element_type(segment, &last_segment_name)
                        .map_err(|message| syn::Error::new_spanned(field_type, message))?;
                    let element_tag = field_type_tag(element, containers_allowed)?.tag_text;
                    if is_primitive_tag(&element_tag) || element_tag.starts_with("struct:") {
                        let prefix = if last_segment_name == "Vec" {
                            "vec"
                        } else {
                            "dynbuf"
                        };
                        let tag_text = format!("{prefix}:{element_tag}");
                        let heap_field_kind = if last_segment_name == "Vec" {
                            HeapFieldKind::Vec { element_tag }
                        } else {
                            HeapFieldKind::DynamicBuffer { element_tag }
                        };
                        Ok(FieldTag {
                            tag_text,
                            heap_field_kind: Some(heap_field_kind),
                        })
                    } else if element_tag == "string" {
                        if last_segment_name == "Vec" {
                            // The elements are individually allocated strings,
                            // so there is no span over them. C# reaches each
                            // one through its own accessor instead.
                            Ok(FieldTag {
                                tag_text: "vec:string".to_string(),
                                heap_field_kind: Some(HeapFieldKind::VecString),
                            })
                        } else {
                            Err(syn::Error::new_spanned(
                                field_type,
                                "`DynamicBuffer<String>` fields are not supported; a \
                                 `DynamicBuffer` holds plain `Copy` elements - use a `Vec<String>` \
                                 for a text list",
                            ))
                        }
                    } else if element_tag.starts_with("vec:")
                        || element_tag.starts_with("dynbuf:")
                    {
                        Err(syn::Error::new_spanned(
                            field_type,
                            format!(
                                "nested containers are not supported; a `{last_segment_name}` field's \
                                 element must be a primitive or a `#[derive(PillMirror)]` struct"
                            ),
                        ))
                    } else if element_tag.starts_with("array:") {
                        Err(syn::Error::new_spanned(
                            field_type,
                            format!(
                                "a `{last_segment_name}` of fixed-size arrays is not supported; \
                                 use a `{last_segment_name}` of primitives"
                            ),
                        ))
                    } else {
                        Err(syn::Error::new_spanned(
                            field_type,
                            format!(
                                "`{last_segment_name}<{element_tag}>` is not a supported container field"
                            ),
                        ))
                    }
                }
                // Heap-owning standard types that have no accessor machinery.
                "Box" | "Arc" | "Rc" | "Option" | "Result" | "Cow" | "HashMap" | "HashSet"
                | "BTreeMap" | "BTreeSet" | "VecDeque" | "LinkedList" => {
                    Err(heap_owned_rejection(field_type, &last_segment_name))
                }
                // A field holding another entity's handle. It gets its own tag
                // instead of a `struct:` tag, so the field layer reads and
                // writes it as a reference the editor can pick, not as an
                // opaque pair of numbers.
                "Entity" => Ok(FieldTag::plain("entity".to_string())),
                // An asset handle keeps the asset it names, so the C# mirror
                // can type the field as `Handle<Mesh>` instead of an opaque
                // `Handle`. The tag is still a `struct:` tag, so every reader
                // that folds or resolves struct tags treats it like any other
                // struct.
                "Handle" => {
                    let asset = segment
                        .and_then(|segment| match &segment.arguments {
                            syn::PathArguments::AngleBracketed(arguments) => {
                                arguments.args.iter().find_map(|argument| match argument {
                                    syn::GenericArgument::Type(syn::Type::Path(asset)) => asset
                                        .path
                                        .segments
                                        .last()
                                        .map(|segment| segment.ident.to_string()),
                                    _ => None,
                                })
                            }
                            _ => None,
                        });
                    Ok(match asset {
                        Some(asset) => FieldTag::plain(format!("struct:Handle<{asset}>")),
                        None => FieldTag::plain("struct:Handle".to_string()),
                    })
                }
                _ => {
                    // A nested struct, or an enum, which the codegen cannot
                    // resolve and renders opaque. Tagged by its fully-qualified
                    // path so a `PillMirror` descriptor resolves it by name.
                    let qualified = flattened_path(&path.path, "::");
                    Ok(FieldTag::plain(format!("struct:{qualified}")))
                }
            }
        }
        syn::Type::Array(array) => {
            // The element count lives in `element_count` (computed from the
            // length expression at compile time), so the tag only carries the
            // element type.
            let element_tag = field_type_tag(&array.elem, containers_allowed)?.tag_text;
            if element_tag.starts_with("vec:")
                || element_tag.starts_with("dynbuf:")
                || element_tag == "string"
            {
                return Err(syn::Error::new_spanned(
                    field_type,
                    "arrays of heap-owning fields are not supported; use a `Vec` or \
                     `DynamicBuffer` field instead",
                ));
            }
            Ok(FieldTag::plain(format!("array:{element_tag}")))
        }
        other => Err(syn::Error::new_spanned(
            other,
            "unsupported field type for the C# mirror; use a primitive, a fixed-size array, or a `#[derive(PillMirror)]` struct",
        )),
    }
}

/// The rejection for a heap-owning type outside the container vocabulary.
///
/// `String`, `Vec` and `DynamicBuffer` reach this only from a value type that
/// is mirrored as plain data (`#[derive(PillMirror)]` or `pill_value_type!`).
/// Such a type has no live row to reach a buffer through.
fn heap_owned_rejection(field_type: &syn::Type, field_type_name: &str) -> syn::Error {
    syn::Error::new_spanned(
        field_type,
        format!(
            "field type `{field_type_name}` owns heap memory and cannot be mirrored to C#; use a \
             blittable value type, a `String`, a `Vec`, or a `DynamicBuffer`"
        ),
    )
}

/// Extract the element type of a `Vec<E>` or `DynamicBuffer<E>` field.
fn container_element_type<'a>(
    segment: Option<&'a syn::PathSegment>,
    container_type_name: &str,
) -> Result<&'a syn::Type, String> {
    let missing_element_message = || {
        format!(
            "a `{container_type_name}` field must name its element type \
             (`{container_type_name}<f32>`, not bare `{container_type_name}`)"
        )
    };
    let Some(segment) = segment else {
        return Err(missing_element_message());
    };
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(missing_element_message());
    };
    arguments
        .args
        .iter()
        .find_map(|argument| match argument {
            syn::GenericArgument::Type(element) => Some(element),
            _ => None,
        })
        .ok_or_else(missing_element_message)
}

/// Whether a tag names one of the scalar primitives.
fn is_primitive_tag(tag: &str) -> bool {
    matches!(
        tag,
        "f32"
            | "f64"
            | "i8"
            | "u8"
            | "i16"
            | "u16"
            | "i32"
            | "u32"
            | "i64"
            | "u64"
            | "bool"
            | "usize"
            | "isize"
    )
}

// =============================================================================
// #[derive(PillMirror)]
// =============================================================================

/// Turns a plain (non-component) value type, or a fieldless enum, into a typed
/// C# mirror.
///
/// A struct with named fields gets a [`PillValueTypeDescriptor`] submitted into
/// this artifact's inventory, so the host's C# codegen can resolve
/// `struct:<path>` field tags to typed nested structs. It also gets the
/// encodings that let mirrored functions take and return the type by value.
///
/// A fieldless enum with an integer `#[repr]` becomes a C# `enum`, and
/// mirrored functions pass its values as the discriminant. Generic types are
/// rejected.
///
/// [`PillValueTypeDescriptor`]: ::pill_engine::component_registry::PillValueTypeDescriptor
#[proc_macro_derive(PillMirror, attributes(pill))]
pub fn derive_pill_mirror(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = &input.ident;

    if let syn::Data::Enum(data) = &input.data {
        return mirror::mirror_enum(&input, data)
            .unwrap_or_else(syn::Error::into_compile_error)
            .into();
    }

    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &input.generics,
            "`PillMirror` cannot be derived for a generic type",
        )
        .to_compile_error()
        .into();
    }

    // Value types cross the boundary as plain data, so a heap-owning field has
    // no meaning here. Nothing gives managed code a live row to reach a buffer
    // through, and the mirror is copied by value.
    let (declared_layout, layout_reference, _accessors, _field_enum) =
        match component_field_descriptors(&input, ident, LayoutSubject::MirroredValue) {
            Ok(field_descriptor_emissions) => field_descriptor_emissions,
            Err(error) => return error.to_compile_error().into(),
        };

    let type_name = quote! {
        ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#ident))
    };

    let encodings = mirror::value_type_encodings(ident);
    let expanded = quote! {
        #declared_layout

        ::pill_engine::submit! {
            ::pill_engine::component_registry::PillValueTypeDescriptor {
                type_name: #type_name,
                crate_name: env!("CARGO_PKG_NAME"),
                size: ::core::mem::size_of::<#ident>(),
                align: ::core::mem::align_of::<#ident>(),
                fields: #layout_reference,
            }
        }

        #encodings
    };

    expanded.into()
}

// =============================================================================
// pill_value_type!
// =============================================================================

/// Declares the layout of a foreign value type for the managed mirror.
///
/// `#[derive(PillMirror)]` reads the type definition, so a type this crate does
/// not define (say `glam::Vec3`) cannot carry the derive. This macro takes the
/// declaration the derive would read: the type path and its `field: type` list.
/// It emits the same
/// [`PillValueTypeDescriptor`](::pill_engine::component_registry::PillValueTypeDescriptor)
/// into the calling artifact's registry. Managed code then gets typed members
/// for the fields instead of an opaque byte blob.
///
/// Spell the path exactly as the component field is written. The field tag is
/// the path as spelled at the field site (`Vector3f` for a `[Vector3f; 16]`
/// field), and the host resolves descriptors by exact name. A generated
/// assertion fails the build when the last declared field does not end the
/// type, so a field added behind the declaration cannot go unseen. The
/// declaration belongs to the artifact whose mirror needs it: another module's
/// registry does not see it.
#[proc_macro]
pub fn pill_value_type(input: TokenStream) -> TokenStream {
    let declaration = parse_macro_input!(input as ValueTypeDeclaration);

    if declaration.fields.is_empty() {
        return syn::Error::new_spanned(
            &declaration.type_path,
            "declare at least one field; a type with none has nothing to mirror",
        )
        .to_compile_error()
        .into();
    }

    // The field walk runs over a synthetic struct, so tags and offsets come
    // from the same code path as the derives. Its name only has to keep the
    // generated `static` unique: two declarations whose paths end in the same
    // segment would collide under the raw name, so the full path is flattened
    // into the identifier.
    let flat_name = flattened_path(&declaration.type_path, "_");
    let synthetic_ident = syn::Ident::new(&flat_name, declaration.type_path.span());
    let field_names = declaration.fields.iter().map(|field| &field.field_name);
    let field_types = declaration.fields.iter().map(|field| &field.field_type);
    let synthetic: DeriveInput = syn::parse_quote! {
        struct #synthetic_ident {
            #(#field_names: #field_types,)*
        }
    };

    // A foreign type can carry neither the inherent `FIELD_LAYOUT` const nor
    // the trait impl, so the emitter skips them and the submit below is the
    // only registration output.
    let (declared_layout, layout_reference, _accessors, _field_enum) =
        match component_field_descriptors(
            &synthetic,
            &declaration.type_path,
            LayoutSubject::ForeignValue,
        ) {
            Ok(field_descriptor_emissions) => field_descriptor_emissions,
            Err(error) => return error.to_compile_error().into(),
        };

    // Joined the same way the field tag is, so the descriptor name matches
    // what a component field spelled with this path produces.
    let type_name = flattened_path(&declaration.type_path, "::");
    let type_path = &declaration.type_path;
    let last_field = declaration
        .fields
        .last()
        .expect("the empty declaration was rejected above");
    let last_field_name = &last_field.field_name;
    let last_field_type = &last_field.field_type;

    let expanded = quote! {
        #declared_layout

        ::pill_engine::submit! {
            ::pill_engine::component_registry::PillValueTypeDescriptor {
                type_name: #type_name,
                crate_name: env!("CARGO_PKG_NAME"),
                size: ::core::mem::size_of::<#type_path>(),
                align: ::core::mem::align_of::<#type_path>(),
                fields: #layout_reference,
            }
        }

        // The declared fields have to account for the whole type: if the
        // foreign type gained a field behind the last declared one, managed
        // code would see fewer members than the bytes contain.
        const _: () = assert!(
            ::core::mem::offset_of!(#type_path, #last_field_name)
                + ::core::mem::size_of::<#last_field_type>()
                == ::core::mem::size_of::<#type_path>(),
            "the declared fields must cover the foreign type exactly; declare any missing field"
        );
    };

    expanded.into()
}

/// One `field: type` entry of a [`pill_value_type!`] declaration.
struct ValueTypeField {
    /// The field's name, as it appears on the foreign type.
    field_name: syn::Ident,
    /// The field's type, used for its size, alignment and tag.
    field_type: syn::Type,
}

impl syn::parse::Parse for ValueTypeField {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let field_name = input.parse()?;
        input.parse::<syn::Token![:]>()?;
        let field_type = input.parse()?;
        Ok(Self {
            field_name,
            field_type,
        })
    }
}

/// The `Type { field: type, ... }` form [`pill_value_type!`] accepts.
struct ValueTypeDeclaration {
    /// The foreign type's path, spelled as the component field is.
    type_path: syn::Path,
    /// The declared fields, in `repr(C)` order.
    fields: Vec<ValueTypeField>,
}

impl syn::parse::Parse for ValueTypeDeclaration {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let type_path: syn::Path = input.parse()?;
        let content;
        syn::braced!(content in input);
        let fields = content.parse_terminated(ValueTypeField::parse, syn::Token![,])?;
        Ok(Self {
            type_path,
            fields: fields.into_iter().collect(),
        })
    }
}

// =============================================================================
// #[pill_mirror_impl] / #[pill_mirror_method]
// =============================================================================

/// Mirrors selected functions of an inherent `impl` block to C#.
///
/// Every function inside the block that carries `#[pill_mirror_method]` becomes
/// a member of the type's C# mirror:
/// - a `&self` or `&mut self` method becomes an instance member that reaches the
///   live value through its address: a component row, a `#[derive(PillMirror)]`
///   value, the box of a `#[pill_mirror_object]`, or, on a
///   `#[pill_mirror_resource]`, an extension method on `Res<T>` / `ResMut<T>`;
/// - a `self` method consumes an object (a builder's `with_*` and `build`);
/// - a function with no receiver becomes a static member (`Shader::new`).
///
/// The marker's path may be written fully qualified: only its last segment is
/// matched. The `impl` block must live in the module that declares the type,
/// because the qualified name the methods are filed under is built there.
///
/// ```ignore
/// #[pill_mirror_object(asset)]
/// pub struct Mesh { /* ... */ }
///
/// #[pill_mirror_impl]
/// impl Mesh {
///     #[pill_mirror_method]
///     pub fn from_obj_bytes(name: impl Into<String>, bytes: &[u8]) -> Result<Self, AssetLoadError> {
///         /* ... */
///     }
/// }
/// ```
///
/// For every marked function the macro also emits, next to the `impl` block, a
/// C-ABI trampoline of the one shape `pill_engine::mirror` defines, and a
/// [`PillMethodDescriptor`] carrying the trampoline's address and the tags of
/// its arguments and result. That is everything the host's C# codegen reads.
///
/// Arguments and results may be primitives, strings, slices and `Vec`s,
/// `impl Into<String>` and `impl IntoIterator`, `Handle<T>`, `AssetLoader`,
/// `Option<T>`, tuples, arrays of primitives, and mirrored types by value or by
/// reference. A `Result<T, E>` with `E: Display` fails the managed call with
/// `E`'s message; any other type is a compile error that names the parameter.
///
/// [`PillMethodDescriptor`]: ::pill_engine::component_registry::PillMethodDescriptor
#[proc_macro_attribute]
pub fn pill_mirror_impl(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    let impl_block = parse_macro_input!(item as syn::ItemImpl);
    mirror::mirror_impl(impl_block)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Marker attribute for functions inside a `#[pill_mirror_impl]` block.
///
/// The impl-level macro reads it from the token stream; it is otherwise a
/// no-op that leaves the function untouched, so a function compiles the same
/// with or without the mirror machinery.
#[proc_macro_attribute]
pub fn pill_mirror_method(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    item
}

// =============================================================================
// #[pill_mirror_fn]
// =============================================================================

/// Mirrors a free function to C#, as a static method on a static class.
///
/// The free-function counterpart of [`pill_mirror_impl`]:
/// `pill_dummy_color::get_color_a` becomes
/// `pill_dummy_color.PillDummyColor.GetColorA()`. The class is the last path
/// segment in PascalCase. Its namespace is the preceding segments joined with
/// dots, or the whole module path when there is no preceding segment. The
/// arguments and results follow [`pill_mirror_impl`]'s vocabulary: a free
/// function may take a resource (`assets: &mut AssetManager` becomes a
/// `ResMut<AssetManager>` parameter) as well as plain values.
///
/// List it before [`pill_hot_fn`]. It then records the original signature, the
/// contract C# compiled against, while the body stays hot-patchable, so a patch
/// replaces the code a managed call executes.
///
/// ```ignore
/// #[pill_mirror_fn]
/// #[pill_hot_fn]
/// pub fn get_color_a() -> f32 {
///     1.0
/// }
/// ```
#[proc_macro_attribute]
pub fn pill_mirror_fn(_attribute: TokenStream, item: TokenStream) -> TokenStream {
    let item_fn = parse_macro_input!(item as ItemFn);
    mirror::mirror_fn(item_fn)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

// =============================================================================
// #[pill_mirror_object] / #[pill_mirror_resource]
// =============================================================================

/// Mirrors a Rust type that C# holds by reference: a class wrapping a box the
/// managed side owns.
///
/// Use it for a type that is not plain data: a mesh, a builder, or a slot
/// description with a `String` in it.
/// - A function that returns the type hands C# a new object.
/// - A function that takes it by value moves it out of the C# object, which is
///   unusable afterwards.
/// - A `&self` or `&mut self` method borrows it.
///
/// Disposing the C# object, or letting the garbage collector finalize it, drops
/// the Rust value, so the type must be `Send`.
///
/// Flags:
/// - `asset` - the type is an `Asset`: C# can add it to the world's
///   `AssetManager`, remove it and look it up (`assets.AddNamed(name, mesh)`).
/// - `import` - the type is also an `ImportedAsset`:
///   `assets.Import<Mesh>(path, ...)`.
/// - `standalone` - the type is also a `StandaloneAsset`, and this flag sets
///   `asset` too: `assets.ImportStandalone<Material>(path)`.
#[proc_macro_attribute]
pub fn pill_mirror_object(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let annotated_item = parse_macro_input!(item as syn::Item);
    mirror::mirror_object(attribute.into(), annotated_item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Mirrors a world resource to C# under a shared name.
///
/// `#[pill_mirror_resource("my_crate::MyResource")]` implements `Resource`
/// with that shared name and declares the type, so C# names it as a marker
/// type:
/// - systems take `Res<T>` or `ResMut<T>`, and the scheduler orders them
///   against every Rust user of the resource;
/// - its `#[pill_mirror_impl]` methods become extension methods on those
///   parameters;
/// - mirrored functions can take it as `&T` or `&mut T`.
///
/// The resource's bytes are never exposed to C#.
///
/// A mirrored resource is play-mode game state, so a type that derives `Clone`
/// is copied at Play and put back at Stop, and one that does not refuses Play
/// by name.
#[proc_macro_attribute]
pub fn pill_mirror_resource(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let annotated_item = parse_macro_input!(item as syn::Item);
    mirror::mirror_resource(attribute.into(), annotated_item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

// =============================================================================
// #[pill_hot]
// =============================================================================

/// Marks a system function as hot-patchable, so the host can replace its
/// implementation without registering it again.
///
/// The function itself is emitted unchanged. The macro only adds a descriptor
/// that carries the function's fully-qualified path, its dispatch address and
/// its signature identity, and submits it into this artifact's compile-time
/// registry.
///
/// ```ignore
/// #[pill_hot]
/// fn movement_system(mut query: Query<(&mut Position, &Velocity)>) {
///     for (mut position, velocity) in query.iter_mut() {
///         position.x += velocity.x;
///     }
/// }
/// ```
///
/// The host patches by `module_path!() + "::" + fn name`, the same shape
/// `PillComponent` uses for its type name. The address and the signature come
/// from the function value, not from its syntax: a function-item type
/// satisfies exactly one arity of `SystemParamFunction`, so the parameter tuple
/// is inferred instead of being rebuilt from tokens.
///
/// `#[pill_hot(name = "project::movement_system")]` overrides the derived name.
/// A generated patch library is its own crate, so `module_path!()` there would
/// report the patch's name instead of the original's. The host therefore passes
/// the name it is patching, so the two agree.
#[proc_macro_attribute]
pub fn pill_hot(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let item_fn = parse_macro_input!(item as ItemFn);
    let fn_ident = &item_fn.sig.ident;

    // Step 1: Parse the optional `name = "..."` override.
    let mut name_override: Option<String> = None;
    if !attribute.is_empty() {
        let name_override_parse = syn::parse::<syn::MetaNameValue>(attribute);
        match name_override_parse {
            Ok(meta) if meta.path.is_ident("name") => match &meta.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(literal),
                    ..
                }) => name_override = Some(literal.value()),
                other => {
                    return syn::Error::new_spanned(
                        other,
                        "`#[pill_hot(name = ...)]` expects a string literal",
                    )
                    .to_compile_error()
                    .into();
                }
            },
            Ok(meta) => {
                return syn::Error::new_spanned(
                    meta.path,
                    "unknown `pill_hot` argument; expected `name = \"...\"`",
                )
                .to_compile_error()
                .into();
            }
            Err(error) => return error.to_compile_error().into(),
        }
    }

    // Step 2: Reject a generic system: the engine patches one concrete
    // monomorphization, so a generic function has no single address to swap.
    if !item_fn.sig.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &item_fn.sig.generics,
            "`#[pill_hot]` cannot be applied to a generic function: hot patching \
             replaces one concrete implementation, and a generic has one per \
             instantiation",
        )
        .to_compile_error()
        .into();
    }

    // Step 3: Emit the function unchanged, next to a descriptor that names its
    // dispatch address and signature identity.
    let descriptor_fn = format_ident!("__pill_hot_descriptor_{}", fn_ident);
    let qualified_name = match &name_override {
        Some(name) => quote! { #name },
        None => quote! {
            ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#fn_ident))
        },
    };

    let expanded = quote! {
        #item_fn

        /// Resolves this function's dispatch address and signature identity for
        /// the artifact-wide hot-patch registry.
        #[allow(non_snake_case)]
        fn #descriptor_fn() -> (usize, u64) {
            (
                ::pill_engine::hot_patch::local_implementation_address(&#fn_ident),
                ::pill_engine::hot_patch::signature_hash_of(&#fn_ident),
            )
        }

        ::pill_engine::submit! {
            ::pill_engine::hot_patch::PillHotFunctionDescriptor {
                qualified_name: #qualified_name,
                resolve: #descriptor_fn,
            }
        }
    };

    expanded.into()
}

/// Declares an ECS system, so the artifact registers it without a call.
///
/// A project or module writes only this attribute. The artifact entry point
/// registers every declared system before its own `register` and `init` run,
/// for a first load, every reload and a statically linked build alike, so the
/// declaration reaches every posture that compiles the artifact.
///
/// ```ignore
/// #[system]
/// fn movement(query: Query<(&mut Position, &Velocity)>) { }
///
/// #[system(name = "spin_cube")]
/// fn spin_cube(query: Query<&mut TransformComponent>) { }
///
/// #[system(name = "physics", order = 900)]
/// fn physics_system(query: Query<&mut PhysicsState>) { }
///
/// #[system(name = "rendering", run_phase = "post_project", run_in_edit_mode = true)]
/// fn rendering_system(commands: Commands) { }
/// ```
///
/// # Arguments
///
/// - `name = "..."` - the display name logs, the editor's Systems tab and a
///   hot patch use. Defaults to `module_path!()` plus the function's name, the
///   same identity a patch uses.
/// - `order = <integer>` - the sort key within the phase. The engine orders
///   every system of a phase by this value across every artifact, and equal
///   values keep registration order, so this is how a declaration states that
///   one system must precede another. `AGENTS.md` documents the range each
///   area uses.
/// - `run_phase = "pre_project" | "project" | "post_project"` - which phase of
///   the frame the system runs in: before the first command flush, after it
///   (the game's own frame work, the default), or after the second flush.
/// - `run_in_edit_mode = true | false` - whether the system also runs while
///   the editor is editing, and while the engine is paused. `false` keeps the
///   game's behaviour under Play and Stop.
/// - `run_if = <path>` - repeatable. A condition `fn(...) -> bool`; the system
///   runs only while every one of them holds. A condition takes the same
///   parameters a system does but only the read-only ones, so one that could
///   write does not compile. A skipped system keeps its place in the schedule.
///
/// An artifact can still register a system by call, in its `register` half;
/// such a system takes its place by the same `(phase, order)`.
#[proc_macro_attribute]
pub fn system(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let item_fn = parse_macro_input!(item as ItemFn);
    let fn_ident = &item_fn.sig.ident;

    // Step 1: Read the optional arguments.
    let mut name_override: Option<String> = None;
    let mut order: i32 = 0;
    let mut phase = quote! { ::pill_engine::RunPhase::Project };
    let mut run_in_edit_mode = false;
    let mut conditions: Vec<syn::Path> = Vec::new();
    if !attribute.is_empty() {
        let attribute = proc_macro2::TokenStream::from(attribute);
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("name") {
                let name_literal: syn::LitStr = meta.value()?.parse()?;
                name_override = Some(name_literal.value());
                return Ok(());
            }
            if meta.path.is_ident("order") {
                let order_literal: syn::LitInt = meta.value()?.parse()?;
                order = order_literal.base10_parse()?;
                return Ok(());
            }
            if meta.path.is_ident("run_phase") {
                let phase_literal: syn::LitStr = meta.value()?.parse()?;
                phase = match phase_literal.value().as_str() {
                    "pre_project" => quote! { ::pill_engine::RunPhase::PreProject },
                    "project" => quote! { ::pill_engine::RunPhase::Project },
                    "post_project" => quote! { ::pill_engine::RunPhase::PostProject },
                    other => {
                        return Err(syn::Error::new(
                            phase_literal.span(),
                            format!(
                                "unknown `run_phase` value `{other}`; expected \
                                 \"pre_project\", \"project\" or \"post_project\""
                            ),
                        ));
                    }
                };
                return Ok(());
            }
            if meta.path.is_ident("run_in_edit_mode") {
                let flag_literal: syn::LitBool = meta.value()?.parse()?;
                run_in_edit_mode = flag_literal.value();
                return Ok(());
            }
            if meta.path.is_ident("run_if") {
                // Repeatable: every condition must hold for the system to run.
                let condition: syn::Path = meta.value()?.parse()?;
                conditions.push(condition);
                return Ok(());
            }
            Err(meta.error(
                "unknown `system` argument; expected `name = \"...\"`, \
                 `order = <integer>`, `run_phase = \"...\"`, \
                 `run_in_edit_mode = true|false` or `run_if = <path>`",
            ))
        });
        if let Err(error) = syn::parse::Parser::parse2(parser, attribute) {
            return error.to_compile_error().into();
        }
    }

    // Step 2: Refuse the shapes a declaration cannot carry. A generic function
    // has no single registration, and a method cannot be named without its
    // receiver type.
    if !item_fn.sig.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &item_fn.sig.generics,
            "`#[system]` cannot be applied to a generic function: the engine \
             registers one concrete system",
        )
        .to_compile_error()
        .into();
    }
    if item_fn.sig.receiver().is_some() {
        return syn::Error::new_spanned(
            &item_fn.sig,
            "`#[system]` applies to a free function: a method cannot be \
             registered without naming the type that owns it",
        )
        .to_compile_error()
        .into();
    }

    // Step 3: Emit the function unchanged, a helper that registers it, and the
    // descriptor the artifact entry point walks.
    let name = match &name_override {
        Some(name) => quote! { #name },
        None => quote! {
            ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#fn_ident))
        },
    };
    let register_ident = format_ident!("__pill_system_register_{}", fn_ident);
    // The options the helper builds and the descriptor the entry point sorts
    // come from the same parsed values, so the two cannot disagree.
    let expanded = quote! {
        #item_fn

        /// Registers this system with the engine; called by the artifact
        /// entry point.
        #[doc(hidden)]
        fn #register_ident(engine: &mut ::pill_engine::Engine) {
            let options = ::pill_engine::SystemOptions::new(#phase, #order)
                .with_run_in_edit_mode(#run_in_edit_mode)
                #( .run_if(#conditions) )*;
            engine.register_system_with_options(#name, options, #fn_ident);
        }

        ::pill_engine::submit! {
            ::pill_engine::declarations::PillSystemDescriptor {
                name: #name,
                order: #order,
                phase: #phase,
                register: #register_ident,
            }
        }
    };
    expanded.into()
}

/// Declares a script component, so the engine registers its lifecycle methods
/// without a call.
///
/// The attribute goes on the `impl ScriptComponent for ...` block. It reads
/// which lifecycle methods the block defines, each lifecycle method's
/// `#[order(n)]`, and whether the script also runs while the editor edits, and
/// turns all of it into the declaration the engine takes the lifecycle methods
/// from. The artifact entry point registers every declared script before its
/// own `register` and `init` run, for a first load, every reload and a
/// statically linked build alike.
///
/// ```ignore
/// #[script]
/// impl ScriptComponent for Spin {
///     fn on_update(&mut self, script_context: &mut ScriptContext) { }
/// }
///
/// #[script(edit_mode)]
/// impl ScriptComponent for EditorSpin {
///     #[order(5)]
///     fn on_create(&mut self, script_context: &mut ScriptContext, create_info: &CreateInfo) { }
///
///     fn on_update(&mut self, script_context: &mut ScriptContext) { }
/// }
/// ```
///
/// # Arguments
///
/// - `edit_mode` - the script also runs while the editor edits, not only while
///   the game plays.
///
/// Every lifecycle method takes its own `#[order(n)]` marker, default `0`.
/// Lower values run first within the same pass, and equal values run in type
/// name order. An `#[order]` on a method that is not one of the five lifecycle
/// methods is refused, because nothing would read it.
///
/// A script component cannot be written without this attribute: the trait
/// requires a declaration that only the attribute implements, so leaving it
/// out is a compile error rather than a script the engine never runs.
#[proc_macro_attribute]
pub fn script(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let mut item_impl = parse_macro_input!(item as syn::ItemImpl);

    // Step 1: Read the optional `edit_mode` flag.
    let mut edit_mode = false;
    if !attribute.is_empty() {
        let attribute = proc_macro2::TokenStream::from(attribute);
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("edit_mode") {
                edit_mode = true;
                return Ok(());
            }
            Err(meta.error("unknown `script` argument; expected `edit_mode`"))
        });
        if let Err(error) = syn::parse::Parser::parse2(parser, attribute) {
            return error.to_compile_error().into();
        }
    }

    // Step 2: Refuse the shapes the declaration cannot carry. The block has to
    // implement `ScriptComponent`, and a generic block has no single concrete
    // type for the engine to register.
    let implements_script_component = item_impl
        .trait_
        .as_ref()
        .and_then(|(_, trait_path, _)| trait_path.segments.last())
        .is_some_and(|segment| segment.ident == "ScriptComponent");
    if !implements_script_component {
        return syn::Error::new_spanned(
            &item_impl.self_ty,
            "`#[script]` applies to an `impl ScriptComponent for ...` block",
        )
        .to_compile_error()
        .into();
    }
    if !item_impl.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &item_impl.generics,
            "`#[script]` cannot be applied to a generic implementation: the \
             engine registers one concrete script type",
        )
        .to_compile_error()
        .into();
    }

    // Step 3: Record which lifecycle methods the block defines and the
    // `#[order(n)]` each one carries, then strip those markers so the emitted
    // block holds only what the trait declares.
    let mut lifecycle_methods = LifecycleMethodOrders::default();
    for item in &mut item_impl.items {
        let syn::ImplItem::Fn(method) = item else {
            continue;
        };
        let method_name = method.sig.ident.to_string();
        let order_attributes = method
            .attrs
            .iter()
            .filter(|attribute| attribute.path().is_ident("order"))
            .count();
        let Some(order_slot) = lifecycle_methods.order_slot_mut(&method_name) else {
            if order_attributes > 0 {
                return syn::Error::new_spanned(
                    &method.sig.ident,
                    format!(
                        "`#[order(n)]` applies to a script lifecycle method; \
                         `{method_name}` is not one (expected `on_create`, \
                         `on_start`, `on_update`, `on_stop` or `on_destroy`)"
                    ),
                )
                .to_compile_error()
                .into();
            }
            continue;
        };
        if order_attributes > 1 {
            return syn::Error::new_spanned(
                &method.sig.ident,
                "a lifecycle method takes one `#[order(n)]`",
            )
            .to_compile_error()
            .into();
        }
        let order = match method
            .attrs
            .iter()
            .position(|attribute| attribute.path().is_ident("order"))
        {
            Some(position) => match method.attrs[position].parse_args::<syn::LitInt>() {
                Ok(order_literal) => match order_literal.base10_parse::<i32>() {
                    Ok(order) => order,
                    Err(error) => return error.to_compile_error().into(),
                },
                Err(error) => return error.to_compile_error().into(),
            },
            // A lifecycle method without the marker still counts: it runs in
            // order 0.
            None => 0,
        };
        *order_slot = Some(order);
        method
            .attrs
            .retain(|attribute| !attribute.path().is_ident("order"));
    }

    // Step 4: Emit the block unchanged, the registration helper and the
    // declaration the engine reads the lifecycle methods from.
    let Some(type_ident) = self_type_ident(&item_impl.self_ty) else {
        return syn::Error::new_spanned(
            &item_impl.self_ty,
            "`#[script]` needs a named type to register",
        )
        .to_compile_error()
        .into();
    };
    let self_ty = &item_impl.self_ty;
    let register_ident = format_ident!("__pill_script_register_{}", type_ident);
    let implements_on_create = lifecycle_methods.on_create.is_some();
    let implements_on_start = lifecycle_methods.on_start.is_some();
    let implements_on_update = lifecycle_methods.on_update.is_some();
    let implements_on_stop = lifecycle_methods.on_stop.is_some();
    let implements_on_destroy = lifecycle_methods.on_destroy.is_some();
    let on_create_order = lifecycle_methods.on_create.unwrap_or(0);
    let on_start_order = lifecycle_methods.on_start.unwrap_or(0);
    let on_update_order = lifecycle_methods.on_update.unwrap_or(0);
    let on_stop_order = lifecycle_methods.on_stop.unwrap_or(0);
    let on_destroy_order = lifecycle_methods.on_destroy.unwrap_or(0);

    let expanded = quote! {
        #item_impl

        /// Registers this script with the engine; called by the artifact
        /// entry point.
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #register_ident(world: &mut ::pill_engine::World) {
            world.register_script_component::<#self_ty>();
        }

        impl ::pill_engine::ScriptDeclaration for #self_ty {
            const DECLARATION: ::pill_engine::ScriptDeclarationData =
                ::pill_engine::ScriptDeclarationData {
                    implements_on_create: #implements_on_create,
                    implements_on_start: #implements_on_start,
                    implements_on_update: #implements_on_update,
                    implements_on_stop: #implements_on_stop,
                    implements_on_destroy: #implements_on_destroy,
                    on_create_order: #on_create_order,
                    on_start_order: #on_start_order,
                    on_update_order: #on_update_order,
                    on_stop_order: #on_stop_order,
                    on_destroy_order: #on_destroy_order,
                    run_in_edit_mode: #edit_mode,
                };
        }

        ::pill_engine::submit! {
            ::pill_engine::declarations::PillScriptDescriptor {
                register: #register_ident,
            }
        }
    };
    expanded.into()
}

/// The lifecycle methods one `#[script]` block declares, each with its order
/// when the block defines it.
#[derive(Default)]
struct LifecycleMethodOrders {
    /// The `on_create` order, or `None` when the block does not define it.
    on_create: Option<i32>,
    /// The `on_start` order, or `None` when the block does not define it.
    on_start: Option<i32>,
    /// The `on_update` order, or `None` when the block does not define it.
    on_update: Option<i32>,
    /// The `on_stop` order, or `None` when the block does not define it.
    on_stop: Option<i32>,
    /// The `on_destroy` order, or `None` when the block does not define it.
    on_destroy: Option<i32>,
}

impl LifecycleMethodOrders {
    /// The slot for the lifecycle method called `method_name`, or `None` when
    /// the name is not a lifecycle method.
    fn order_slot_mut(&mut self, method_name: &str) -> Option<&mut Option<i32>> {
        match method_name {
            "on_create" => Some(&mut self.on_create),
            "on_start" => Some(&mut self.on_start),
            "on_update" => Some(&mut self.on_update),
            "on_stop" => Some(&mut self.on_stop),
            "on_destroy" => Some(&mut self.on_destroy),
            _ => None,
        }
    }
}

/// The identifier a block's self type registers under, or `None` for a type
/// with no name of its own.
fn self_type_ident(self_type: &syn::Type) -> Option<&syn::Ident> {
    match self_type {
        syn::Type::Path(type_path) => type_path.path.segments.last().map(|segment| &segment.ident),
        _ => None,
    }
}

/// Declares an ECS resource, so the artifact declares its type without a call.
///
/// A project or module writes only this derive. The artifact entry point
/// declares every derived type before its own `register` and `init` run, for a
/// first load, every reload and a statically linked build alike, so the
/// declaration reaches every posture that compiles the artifact. The value is
/// still stored by `world.insert_resource(..)`, exactly as before; only the
/// declaration call moves.
///
/// ```ignore
/// #[derive(PillResource)]
/// pub struct FrameStats { frames: u64 }
///
/// #[derive(PillResource)]
/// #[pill(shared, layout_sources = ["src/frame_stats.rs"])]
/// pub struct FrameStats { frames: u64 }
///
/// #[derive(PillResource)]
/// #[pill(manager)]
/// pub struct AudioManager { /* ... */ }
/// ```
///
/// # Arguments
///
/// - `shared` - give the type a stable identity instead of a per-binary
///   `TypeId`, which is what lets a reload keep the value instead of retiring
///   it. On its own the name is derived from `module_path!()`; `shared =
///   "..."` pins it, and must be namespaced for the same reason
///   `PillComponent`'s must be.
/// - `layout_sources = ["..."]` - the files whose contents define the type's
///   layout. The development host fingerprints them, so an edit that changes
///   the layout rebuilds the value instead of keeping bytes that no longer
///   mean what they did.
/// - `layout_dependencies = ["..."]` - the crates whose resolved version can
///   change the type's layout, as package names.
///
/// A manager resource declares itself with
/// `World::register_settings_asset_manager` in its crate's `register` instead:
/// the settings surface holds function pointers into the image that declared
/// it, so every generation re-points them.
#[proc_macro_derive(PillResource, attributes(pill))]
pub fn derive_pill_resource(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = &input.ident;

    // Step 1: Parse the `#[pill(...)]` helper attribute.
    let mut shared = false;
    let mut shared_name_override: Option<String> = None;
    let mut layout_sources: Option<Vec<String>> = None;
    let mut layout_dependencies: Option<Vec<String>> = None;
    let mut play_state: Option<String> = None;
    for attribute in &input.attrs {
        if !attribute.path().is_ident("pill") {
            continue;
        }
        if let Err(error) = attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("shared") {
                shared = true;
                if meta.input.peek(syn::Token![=]) {
                    let literal: syn::LitStr = meta.value()?.parse()?;
                    let name = literal.value();
                    if name.is_empty() {
                        return Err(syn::Error::new_spanned(
                            &literal,
                            "a shared resource name cannot be empty",
                        ));
                    }
                    // The same rule a shared component name follows: the name
                    // is a process-wide identity, so an unrelated type
                    // declaring it too would read this resource's bytes.
                    if !name.contains("::") {
                        return Err(syn::Error::new_spanned(
                            &literal,
                            format!(
                                "shared resource name `{name}` is not namespaced. A shared \
                                 name is a process-wide identity, so an unrelated crate \
                                 declaring `{name}` too would bind to this resource and read \
                                 its bytes. Qualify it (`my_crate::{name}`), or drop the \
                                 argument to derive it from `module_path!()`."
                            ),
                        ));
                    }
                    shared_name_override = Some(name);
                }
                return Ok(());
            }
            if meta.path.is_ident("layout_sources") {
                layout_sources = Some(parse_string_list(meta.value()?)?);
                return Ok(());
            }
            if meta.path.is_ident("layout_dependencies") {
                layout_dependencies = Some(parse_string_list(meta.value()?)?);
                return Ok(());
            }
            if meta.path.is_ident("play_state") {
                let literal: syn::LitStr = meta.value()?.parse()?;
                let value = literal.value();
                if value != "keep" && value != "rebuild" {
                    return Err(syn::Error::new_spanned(
                        &literal,
                        "`play_state` is `keep` or `rebuild`; a resource that is restored on \
                         Stop needs no attribute, because restoring is the default",
                    ));
                }
                play_state = Some(value);
                return Ok(());
            }
            Err(meta.error(
                "unknown `pill` attribute; expected `shared`, `layout_sources`, \
                 `layout_dependencies` or `play_state`",
            ))
        }) {
            return error.to_compile_error().into();
        }
    }

    // Step 2: Reject a generic resource: the engine keys a resource by
    // `TypeId`, so a generic type has no single identity to declare.
    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &input.generics,
            "`PillResource` cannot be derived for a generic type",
        )
        .to_compile_error()
        .into();
    }

    // Step 3: Emit the trait implementation and the descriptor.
    let type_name = quote! {
        ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#ident))
    };
    let shared_impl = if shared {
        let shared_name = match &shared_name_override {
            Some(name) => quote! { #name },
            None => type_name.clone(),
        };
        quote! {
            fn shared_name() -> ::core::option::Option<&'static str> {
                ::core::option::Option::Some(#shared_name)
            }

            fn shared_identity() -> ::core::option::Option<u128> {
                // A `const` rather than the trait's default method for the
                // same reason `PillComponent` computes one: the default hashes
                // the name on every call, and this folds at compile time.
                const IDENTITY: u128 =
                    ::pill_engine::component::shared_component_identity(#shared_name);
                ::core::option::Option::Some(IDENTITY)
            }
        }
    } else {
        quote! {}
    };
    let layout_sources_impl = layout_field_impl("layout_sources", layout_sources.as_deref());
    let layout_dependencies_impl =
        layout_field_impl("layout_dependencies", layout_dependencies.as_deref());
    let play_state_impl = match play_state.as_deref() {
        Some("keep") => quote! {
            fn play_state() -> ::pill_engine::resource::PlayState {
                ::pill_engine::resource::PlayState::Keep
            }
        },
        Some("rebuild") => quote! {
            fn play_state() -> ::pill_engine::resource::PlayState {
                ::pill_engine::resource::PlayState::Rebuild
            }
        },
        // Restoring on Stop is the trait's default, so an unmarked resource
        // emits nothing here.
        _ => quote! {},
    };
    // The copy hook. The probe is resolved here, at the type's own site, where
    // the compiler can see whether the type is `Clone`; a generic insert path
    // could not.
    let clone_erased_impl = quote! {
        fn clone_erased(
            value: &::pill_engine::resource::ErasedResource,
        ) -> ::core::option::Option<::pill_engine::resource::ErasedResource> {
            // The two probes only answer as methods, so their traits have to be
            // in scope here; the calling module does not import them.
            #[allow(unused_imports)]
            use ::pill_engine::resource::{RecordResourceClone as _, SkipResourceClone as _};
            (&&::pill_engine::resource::CloneResourceProbe::<Self>::new()).copy_value(value)
        }
    };
    let register_ident = format_ident!("__pill_register_resource_{}", ident);
    let register_call = quote! { world.register_resource::<#ident>(); };

    let expanded = quote! {
        impl ::pill_engine::Resource for #ident {
            #shared_impl

            #clone_erased_impl

            #play_state_impl

            #layout_sources_impl

            #layout_dependencies_impl
        }

        /// Declares this resource with a world; called by the artifact entry
        /// point.
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #register_ident(world: &mut ::pill_engine::World) {
            #register_call
        }

        ::pill_engine::submit! {
            ::pill_engine::declarations::PillResourceDescriptor {
                type_name: #type_name,
                register: #register_ident,
            }
        }
    };

    expanded.into()
}

/// Declares an asset type, so the artifact declares it without a call.
///
/// A crate that owns an asset type writes only this derive. The artifact entry
/// point declares every derived type before its own `register` and `init` run,
/// for a first load, every reload and a statically linked build alike. Loading
/// (`import`, `load_asset`, `import_standalone`) is unchanged: only the
/// declaration call moves.
///
/// The trait implementations themselves stay hand-written - which source
/// extensions a type imports, how it decodes them, and its on-disk document
/// shape are decisions about the type, not something a derive can invent. This
/// derive only says how the world should declare it.
///
/// ```ignore
/// #[derive(PillAsset)]
/// #[pill(imported)]
/// pub struct Sound { /* ... */ }
///
/// #[derive(PillAsset)]
/// #[pill(standalone)]
/// pub struct Material { /* ... */ }
/// ```
///
/// # Arguments
///
/// - `imported` - the type is an `ImportedAsset`, declared with
///   `World::register_imported_asset`, so it can be imported from a source
///   file by extension.
/// - `standalone` - the type is a `StandaloneAsset`, declared with
///   `World::register_standalone_asset`, so it is stored as its own document
///   file.
///
/// Exactly one of the two is required: a type is one or the other.
#[proc_macro_derive(PillAsset, attributes(pill))]
pub fn derive_pill_asset(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let ident = &input.ident;

    // Step 1: Parse the `#[pill(...)]` helper attribute.
    let mut imported = false;
    let mut standalone = false;
    for attribute in &input.attrs {
        if !attribute.path().is_ident("pill") {
            continue;
        }
        if let Err(error) = attribute.parse_nested_meta(|meta| {
            if meta.path.is_ident("imported") {
                imported = true;
                return Ok(());
            }
            if meta.path.is_ident("standalone") {
                standalone = true;
                return Ok(());
            }
            Err(meta.error("unknown `pill` attribute; expected `imported` or `standalone`"))
        }) {
            return error.to_compile_error().into();
        }
    }
    if imported == standalone {
        return syn::Error::new_spanned(
            &input.ident,
            "`PillAsset` needs exactly one of `#[pill(imported)]` or \
             `#[pill(standalone)]`: a type imports from a source file or is its \
             own document file, never both",
        )
        .to_compile_error()
        .into();
    }

    // Step 2: Reject a generic asset type: the world keys an asset by `TypeId`,
    // so a generic type has no single identity to declare.
    if !input.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &input.generics,
            "`PillAsset` cannot be derived for a generic type",
        )
        .to_compile_error()
        .into();
    }

    // Step 3: Emit the declaration the entry point walks.
    let type_name = quote! {
        ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#ident))
    };
    let register_ident = format_ident!("__pill_register_asset_{}", ident);
    let register_call = if standalone {
        quote! { world.register_standalone_asset::<#ident>(); }
    } else {
        quote! { world.register_imported_asset::<#ident>(); }
    };

    let expanded = quote! {
        /// Declares this asset type with a world; called by the artifact entry
        /// point.
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #register_ident(world: &mut ::pill_engine::World) {
            #register_call
        }

        ::pill_engine::submit! {
            ::pill_engine::declarations::PillAssetDescriptor {
                type_name: #type_name,
                register: #register_ident,
            }
        }
    };

    expanded.into()
}

/// The `Resource` method that reports a declared list, or nothing when the
/// derive was given no list.
fn layout_field_impl(method: &str, values: Option<&[String]>) -> proc_macro2::TokenStream {
    let Some(values) = values else {
        return quote! {};
    };
    let method = format_ident!("{}", method);
    quote! {
        fn #method() -> ::core::option::Option<&'static [&'static str]> {
            ::core::option::Option::Some(&[#(#values),*])
        }
    }
}

/// Parse a `["a", "b"]` list from one attribute argument.
fn parse_string_list(input: syn::parse::ParseStream) -> syn::Result<Vec<String>> {
    let content;
    syn::bracketed!(content in input);
    let literals =
        content.parse_terminated(<syn::LitStr as syn::parse::Parse>::parse, syn::Token![,])?;
    let mut values = Vec::new();
    for literal in literals {
        let value = literal.value();
        if value.is_empty() {
            return Err(syn::Error::new_spanned(
                &literal,
                "a declared path or crate name cannot be empty",
            ));
        }
        values.push(value);
    }
    Ok(values)
}

/// Arguments `#[pill_hot_fn]` accepts when it generates a patch body.
///
/// A patch for an inherent method cannot copy the method into a free function,
/// because the body names `self`. The body is placed in a local trait
/// implemented for the receiver type instead. A trait method has the same call
/// shape as the inherent method it replaces, so its address can go straight
/// into the slot.
///
/// The host supplies both values. A developer never writes them.
#[derive(Default)]
struct PatchArguments {
    /// Registry name the generated descriptor is filed under.
    registry_name: Option<String>,
    /// Concrete receiver type the local trait is implemented for.
    self_type: Option<syn::Type>,
}

/// Parse `name = "...", self_type = Path` from an attribute argument list.
///
/// An empty list is the ordinary case: the attribute is being applied by a
/// developer to their own function or method.
fn parse_patch_arguments(attribute: TokenStream) -> Result<PatchArguments, syn::Error> {
    let mut patch_arguments = PatchArguments::default();
    if attribute.is_empty() {
        return Ok(patch_arguments);
    }
    let attribute = proc_macro2::TokenStream::from(attribute);
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("name") {
            let name_literal: syn::LitStr = meta.value()?.parse()?;
            patch_arguments.registry_name = Some(name_literal.value());
            return Ok(());
        }
        if meta.path.is_ident("self_type") {
            patch_arguments.self_type = Some(meta.value()?.parse()?);
            return Ok(());
        }
        Err(meta.error("expected `name` or `self_type`"))
    });
    syn::parse::Parser::parse2(parser, attribute)?;
    Ok(patch_arguments)
}

/// Makes an ordinary function hot-patchable.
///
/// Use this for a plain `pub fn`, and `#[pill_hot]` for an ECS system. A system
/// already has an indirection: the engine holds its boxed closure and can swap
/// what it calls. An ordinary function is called directly, so the indirection
/// has to live inside the function itself.
///
/// ```ignore
/// #[pill_hot_fn]
/// pub fn get_color_a() -> f32 {
///     133.0
/// }
/// ```
///
/// The real body is renamed, and the public name becomes a dispatcher that
/// reads a slot. Every caller then goes through the redirect, including callers
/// in other crates that linked this one statically.
///
/// An inherent method takes a different shape. Its body cannot be renamed
/// because it uses `self`, so it is emitted in place and the dispatcher sits in
/// front of it. A method parameter must therefore be a plain binding, because
/// that is the name the dispatcher forwards by.
///
/// # A crate linked into several artifacts
///
/// The slot is a `static` in whichever artifact compiled the function. A crate
/// linked into both a module DLL and the project has an independent copy of its
/// code in each, so each copy has its own slot and must be patched separately.
/// The host installs into every loaded artifact that declares the name.
#[proc_macro_attribute]
pub fn pill_hot_fn(attribute: TokenStream, item: TokenStream) -> TokenStream {
    // Step 1: Read the arguments the host passes when it generates a patch.
    let patch_arguments = match parse_patch_arguments(attribute) {
        Ok(parsed_arguments) => parsed_arguments,
        Err(error) => return error.to_compile_error().into(),
    };
    let item_fn = parse_macro_input!(item as ItemFn);
    let signature = &item_fn.sig;
    let fn_ident = &signature.ident;
    let visibility = &item_fn.vis;
    let attributes = &item_fn.attrs;
    let body = &item_fn.block;

    // Step 2: Reject a generic function. It has one instantiation per set of
    // type arguments, so there is no single address a slot could hold.
    if !signature.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &signature.generics,
            concat!(
                "`#[pill_hot_fn]` cannot be applied to a generic function: ",
                "patching replaces one concrete implementation, and a generic ",
                "has one per instantiation"
            ),
        )
        .to_compile_error()
        .into();
    }

    // An inherent method is supported, and takes a different shape: its body
    // stays inline in the dispatcher rather than being hoisted into a function
    // of its own. Hoisting is impossible for a method, because every item
    // inside a method body is barred from naming `Self` (error E0401), and a
    // hoisted body would have to name the receiver type.
    let receiver = signature.receiver().cloned();

    // Step 3: Build the parameter lists. A free function's body is hoisted
    // under its original signature, so its dispatcher may rename every
    // parameter and forward the renamed values. A method keeps its body inline
    // and forwards through the names that body uses. Its declaration therefore
    // keeps each pattern exactly as written, and the forwarded name is the one
    // the pattern binds.
    let mut parameter_names = Vec::new();
    let mut parameter_declarations = Vec::new();
    let mut parameter_declaration_shapes = Vec::new();
    let mut parameter_types = Vec::new();
    for (index, argument) in signature.inputs.iter().enumerate() {
        let syn::FnArg::Typed(typed) = argument else {
            continue;
        };
        let argument_type = &*typed.ty;
        parameter_types.push(quote! { #argument_type });
        if receiver.is_some() {
            // A destructuring pattern binds no single name to forward, and the
            // inline body is emitted verbatim below, so it is refused here
            // with the reason rather than surfacing as an unresolved name
            // inside the body.
            let syn::Pat::Ident(identifier) = &*typed.pat else {
                return syn::Error::new_spanned(
                    &typed.pat,
                    concat!(
                        "`#[pill_hot_fn]` on a method requires a plain parameter ",
                        "binding: the body is emitted in place, and the dispatcher ",
                        "forwards through the names it uses. Bind the value to a ",
                        "name here and destructure it inside the body."
                    ),
                )
                .to_compile_error()
                .into();
            };
            let identifier = &identifier.ident;
            parameter_names.push(quote! { #identifier });
            parameter_declarations.push(quote! { #typed });
            parameter_declaration_shapes.push(quote! { #identifier: #argument_type });
        } else {
            let argument_name = format_ident!("argument_{index}");
            parameter_names.push(quote! { #argument_name });
            parameter_declarations.push(quote! { #argument_name: #argument_type });
        }
    }

    let return_type = &signature.output;
    let slot_ident = format_ident!("__PILL_HOT_SLOT_{}", fn_ident.to_string().to_uppercase());

    // Step 4: Build the name, the signature text and the call shapes that this
    // function and a generated patch share. The receiver type is deliberately
    // absent from the name: a method-level attribute cannot learn it, because
    // the descriptor is an item and items may not name `Self`. Two hot
    // functions sharing a name in one module therefore collide, which the
    // host source scanner detects and refuses with a clear message.
    let qualified_name = match &patch_arguments.registry_name {
        // A generated patch is filed under the name the host asks for, which is
        // the running function's own path behind a prefix that cannot collide
        // with the copy the linked rlib also contributes.
        Some(name) => quote! { #name },
        None => quote! {
            ::core::concat!(::core::module_path!(), "::", ::core::stringify!(#fn_ident))
        },
    };

    // The gate: the signature exactly as written, receiver included. A patch
    // derives the same text from the same source through this same shape, so a
    // reshaped function is refused rather than installed behind call sites
    // compiled for the old shape.
    let receiver_type = receiver.as_ref().map(|receiver| receiver.ty.clone());
    let mut signature_parts: Vec<proc_macro2::TokenStream> = Vec::new();
    if let Some(receiver_type) = &receiver_type {
        signature_parts.push(quote! { ::core::stringify!(#receiver_type), "," });
    }
    for parameter_type in &parameter_types {
        signature_parts.push(quote! { ::core::stringify!(#parameter_type), "," });
    }
    let signature_text = quote! {
        ::core::concat!("(", #(#signature_parts,)* ")", ::core::stringify!(#return_type))
    };

    // The pointer type an installed replacement is called through. A method
    // receiver is simply its first argument.
    let dispatch_type = match &receiver_type {
        Some(receiver_type) => {
            quote! { fn(#receiver_type #(, #parameter_types)*) #return_type }
        }
        None => quote! { fn(#(#parameter_types),*) #return_type },
    };
    let dispatch_arguments = match &receiver {
        Some(_) => quote! { self #(, #parameter_names)* },
        None => quote! { #(#parameter_names),* },
    };
    let implementation_declarations = match &receiver {
        Some(receiver) => quote! { #receiver #(, #parameter_declarations)* },
        None => quote! { #(#parameter_declarations),* },
    };

    // The declaration used by the body-less trait a method patch is carried in:
    // binding modifiers are stripped, because `mut value` in a signature without
    // a body trips the `patterns_in_fns_without_body` lint. The implementation
    // that carries the body keeps every pattern exactly as written, which is
    // what the body itself needs.
    let trait_declarations = match &receiver {
        Some(receiver) => quote! { #receiver #(, #parameter_declaration_shapes)* },
        None => quote! { #(#parameter_declarations),* },
    };

    // Step 5: A generated patch for an inherent method. The body names `self`,
    // so it cannot be copied into a free function. A local trait may be
    // implemented for a foreign type, though, and a trait method has the same
    // call shape as the inherent method it replaces: the receiver is simply its
    // first argument. So the body goes verbatim into a trait implementation for
    // the concrete receiver type. That method's address then goes straight into
    // the running artifact's slot.
    //
    // No dispatcher is generated: nothing ever calls a patch through a slot of
    // its own. The signature text comes from the same computation the running
    // artifact used, which is what keeps the two comparable.
    if let Some(self_type) = &patch_arguments.self_type {
        if receiver.is_none() {
            return syn::Error::new_spanned(
                signature,
                "a `self_type` patch requires a method, but this function takes no receiver",
            )
            .to_compile_error()
            .into();
        }
        let slot_ident = format_ident!("__PILL_PATCH_SLOT_{}", fn_ident.to_string().to_uppercase());
        let address_ident = format_ident!("__pill_patch_address_{}", fn_ident);
        let expanded = quote! {
            /// The replacement body, in a local trait so it keeps using `self`.
            trait PillHotMethodPatch {
                fn #fn_ident(#trait_declarations) #return_type;
            }

            impl PillHotMethodPatch for #self_type {
                #[inline(never)]
                fn #fn_ident(#implementation_declarations) #return_type #body
            }

            /// Address of the replacement, reported through the descriptor.
            #[doc(hidden)]
            fn #address_ident() -> usize {
                <#self_type as PillHotMethodPatch>::#fn_ident as *const () as usize
            }

            /// Unused here; a patch is never itself patched.
            #[doc(hidden)]
            #[allow(non_upper_case_globals)]
            static #slot_ident: ::pill_engine::hot_patch::PlainSlot =
                ::pill_engine::hot_patch::PlainSlot::new();

            ::pill_engine::submit! {
                ::pill_engine::hot_patch::PillHotSlotDescriptor {
                    qualified_name: #qualified_name,
                    slot: &#slot_ident,
                    signature: #signature_text,
                    implementation_address:
                        ::core::option::Option::Some(#address_ident as fn() -> usize),
                }
            }
        };
        return expanded.into();
    }

    // Only the slot machinery is conditional. The body is emitted exactly once,
    // unconditionally, so an optimized build compiles the function as written
    // and an editor never greys the source out as inactive code.
    let (implementation_item, implementation_address, fallback) = match &receiver {
        // A method keeps its body inline; there is nothing to address, and only
        // a patch - which names the receiver type concretely - ever needs one.
        Some(_) => (
            quote! {},
            quote! { ::core::option::Option::None },
            quote! { #body },
        ),
        // A free function hoists its body, so a patch built from this same
        // attribute has a symbol to report.
        None => {
            let implementation_ident = format_ident!("__pill_hot_impl_{}", fn_ident);
            let address_ident = format_ident!("__pill_hot_address_{}", fn_ident);
            let mut renamed = item_fn.clone();
            renamed.sig.ident = implementation_ident.clone();
            renamed.vis = syn::Visibility::Inherited;
            renamed.attrs.clear();
            (
                quote! {
                    /// The original body, renamed so the public name can dispatch.
                    ///
                    /// `inline(never)` only where its address is taken; an
                    /// optimized build folds it back into the caller.
                    #[doc(hidden)]
                    #[cfg_attr(debug_assertions, inline(never))]
                    #renamed

                    /// Address of the body above.
                    ///
                    /// A function rather than a constant because casting a fn
                    /// item to `usize` is not allowed while building a `static`.
                    #[doc(hidden)]
                    #[cfg(debug_assertions)]
                    fn #address_ident() -> usize {
                        #implementation_ident as *const () as usize
                    }
                },
                quote! { ::core::option::Option::Some(#address_ident as fn() -> usize) },
                quote! { #implementation_ident(#(#parameter_names),*) },
            )
        }
    };

    let expanded = quote! {
        #implementation_item

        #(#attributes)*
        #visibility fn #fn_ident(#implementation_declarations) #return_type {
            #[cfg(debug_assertions)]
            {
                /// Redirect slot for this function, private to this artifact.
                #[doc(hidden)]
                #[allow(non_upper_case_globals)]
                static #slot_ident: ::pill_engine::hot_patch::PlainSlot =
                    ::pill_engine::hot_patch::PlainSlot::new();

                ::pill_engine::submit! {
                    ::pill_engine::hot_patch::PillHotSlotDescriptor {
                        qualified_name: #qualified_name,
                        slot: &#slot_ident,
                        signature: #signature_text,
                        implementation_address: #implementation_address,
                    }
                }

                // One acquire load from a hot cache line, then a call.
                let installed = #slot_ident.installed();
                if installed != 0 {
                    // SAFETY: the slot holds an address accepted by
                    // `install_plain_function`, which refuses any whose
                    // signature text differs from the one recorded above. A
                    // replacement lives in a patch library the host never
                    // unloads, so it stays executable for the process lifetime.
                    let implementation: #dispatch_type =
                        unsafe { ::core::mem::transmute(installed) };
                    return implementation(#dispatch_arguments);
                }
            }
            #fallback
        }
    };

    expanded.into()
}

/// Emits the hot-patch resolver export on its own.
///
/// `#[pill_project]` and `#[pill_module]` already include it. This macro exists
/// for an artifact that carries hot functions but has neither entry point, in
/// particular the patch libraries the host generates.
///
/// Takes an optional export name, which defaults to `pill_hot_resolve`:
///
/// ```ignore
/// pill_engine::pill_hot_resolver!();                    // pill_hot_resolve
/// pill_engine::pill_hot_resolver!(pill_patch_resolve);  // custom
/// ```
///
/// A generated patch **must** pass a different name. It links the project's
/// rlib to reach that crate's types and helpers, and that rlib already exports
/// `pill_hot_resolve`. Two `#[no_mangle]` definitions of one symbol in a single
/// artifact are a linker error.
#[proc_macro]
pub fn pill_hot_resolver(item: TokenStream) -> TokenStream {
    let export_name = if item.is_empty() {
        format_ident!("pill_hot_resolve")
    } else {
        match syn::parse::<syn::Ident>(item) {
            Ok(identifier) => identifier,
            Err(error) => return error.to_compile_error().into(),
        }
    };
    hot_patch_resolver_export(&export_name, &quote! { #[cfg(debug_assertions)] }).into()
}

/// The exported resolver every loadable artifact provides.
///
/// There is one export rather than one per hot function. The module ABI keeps
/// its export list small for the same reason: a Windows DLL cannot exceed 65535
/// exports. A lookup by name is cheap at reload time.
fn hot_patch_resolver_export(
    export_name: &proc_macro2::Ident,
    gate: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let install_name = format_ident!("{}_install", export_name);
    let plain_name = format_ident!("{}_plain", export_name);
    let reset_name = format_ident!("{}_reset", export_name);
    let address_name = format_ident!("{}_address", export_name);
    let coverage_name = format_ident!("{}_extent_coverage", export_name);
    quote! {
        /// Reports the address of any function in this artifact by qualified
        /// path, from the build-script-generated inventory.
        ///
        /// Unlike the slot exports this needs no annotation on the function -
        /// discovery is mechanical - which is what makes prologue patching
        /// macro-free. Returns zero when this artifact has no such entry.
        ///
        /// # Safety
        ///
        /// `qualified_name` must be a valid NUL-terminated C string that stays
        /// readable for the call.
        #gate
        #[no_mangle]
        pub unsafe extern "C" fn #address_name(
            qualified_name: *const ::core::ffi::c_char,
            out_signature: *mut *const u8,
            out_signature_length: *mut usize,
        ) -> usize {
            if qualified_name.is_null() {
                return 0;
            }
            // SAFETY: the caller upholds the contract documented above.
            unsafe {
                ::pill_engine::hot_patch::with_qualified_name(qualified_name, 0, |name| {
                    let Some(address) =
                        ::pill_engine::hot_patch::scanned_function_address(name)
                    else {
                        return 0;
                    };
                    // The declaration this artifact was built with, so the
                    // caller can refuse a replacement whose shape no longer
                    // matches.
                    if let Some(signature) =
                        ::pill_engine::hot_patch::scanned_function_signature(name)
                    {
                        // SAFETY: the caller upholds the contract documented
                        // above for both out-parameters.
                        unsafe {
                            ::pill_engine::hot_patch::write_signature_out(
                                signature.as_bytes(),
                                out_signature,
                                out_signature_length,
                            );
                        }
                    }
                    address
                })
            }
        }

        /// How many of this artifact's functions have a length the exception
        /// directory records, packed as `(known << 32) | total`.
        ///
        /// A prologue patch can only overwrite a function whose extent is
        /// known, so this is what a host reports when one is refused: it
        /// separates "this function happens to be a leaf" from "nothing in this
        /// artifact is reachable by that route".
        #gate
        #[no_mangle]
        pub extern "C" fn #coverage_name() -> u64 {
            let (known, total) = ::pill_engine::hot_patch::functions_with_known_extent();
            ((known as u64) << 32) | (total as u64 & 0xFFFF_FFFF)
        }

        /// Return a `#[pill_hot_fn]` declared in THIS artifact to its own body.
        ///
        /// The counterpart of the install export, used to roll a patch back to
        /// generation zero. A plain function has no single baseline address a
        /// host could reinstall - every artifact linking the crate holds its own
        /// copy - so each artifact is asked to empty its own slot instead.
        ///
        /// Returns 0 on success and 1 when this artifact declares no such
        /// function.
        ///
        /// # Safety
        ///
        /// `qualified_name` must be a valid NUL-terminated C string that stays
        /// readable for the call.
        #gate
        #[no_mangle]
        pub unsafe extern "C" fn #reset_name(
            qualified_name: *const ::core::ffi::c_char,
        ) -> u32 {
            // SAFETY: the caller upholds the contract documented above.
            unsafe {
                ::pill_engine::hot_patch::with_qualified_name(qualified_name, 1, |name| {
                    match ::pill_engine::hot_patch::reset_plain_function(name) {
                        Ok(()) => 0,
                        Err(_) => 1,
                    }
                })
            }
        }

        /// Report where a `#[pill_hot_fn]` declared in THIS artifact lives.
        ///
        /// Returns the implementation's address, or zero when this artifact
        /// declares no such function. On success the signature text is written
        /// through `out_signature` and `out_signature_length` as a pointer and
        /// byte count, because the text comes from `concat!` and is therefore
        /// not NUL-terminated.
        ///
        /// # Safety
        ///
        /// `qualified_name` must be a valid NUL-terminated C string readable
        /// for the call. `out_signature` and `out_signature_length` must be
        /// null or point at writable slots. The reported signature borrows
        /// static storage inside this artifact and stays valid while it is
        /// loaded.
        #gate
        #[no_mangle]
        pub unsafe extern "C" fn #plain_name(
            qualified_name: *const ::core::ffi::c_char,
            out_signature: *mut *const u8,
            out_signature_length: *mut usize,
        ) -> usize {
            if qualified_name.is_null() {
                return 0;
            }
            // SAFETY: the caller upholds the contract documented above.
            unsafe {
                ::pill_engine::hot_patch::with_qualified_name(qualified_name, 0, |name| {
                    match ::pill_engine::hot_patch::plain_function_entry(name) {
                        Some((address, signature)) => {
                            // SAFETY: the caller upholds the contract documented
                            // above for both out-parameters.
                            unsafe {
                                ::pill_engine::hot_patch::write_signature_out(
                                    signature.as_bytes(),
                                    out_signature,
                                    out_signature_length,
                                );
                            }
                            address
                        }
                        None => 0,
                    }
                })
            }
        }

        /// Redirect a `#[pill_hot_fn]` declared in THIS artifact.
        ///
        /// A crate linked into several artifacts has an independent copy of its
        /// code in each, so the host calls this on every loaded artifact rather
        /// than assuming one of them owns the function.
        ///
        /// Returns 0 on success, 1 when this artifact declares no such function,
        /// and 2 when the signature no longer matches. A non-zero result always
        /// means the running implementation was left untouched.
        ///
        /// # Safety
        ///
        /// `qualified_name` and `signature` must be valid NUL-terminated C
        /// strings that stay readable for the call, and `address` must point at
        /// a function with the signature `signature` describes, in a library
        /// that outlives the process's use of it.
        #gate
        #[no_mangle]
        pub unsafe extern "C" fn #install_name(
            qualified_name: *const ::core::ffi::c_char,
            address: usize,
            signature: *const ::core::ffi::c_char,
        ) -> u32 {
            if qualified_name.is_null() || signature.is_null() {
                return 1;
            }
            // SAFETY: the caller upholds the contract documented above.
            unsafe {
                ::pill_engine::hot_patch::with_qualified_name(qualified_name, 1, |name| {
                    // SAFETY: the caller guarantees a NUL-terminated string
                    // readable for the duration of this call.
                    let signature = unsafe { ::std::ffi::CStr::from_ptr(signature) };
                    let Ok(signature) = signature.to_str() else {
                        return 1;
                    };
                    match ::pill_engine::hot_patch::install_plain_function(
                        name,
                        address,
                        signature,
                    ) {
                        Ok(()) => 0,
                        Err(::pill_engine::hot_patch::HotPatchError::UnknownSystem { .. }) => 1,
                        Err(_) => 2,
                    }
                })
            }
        }

        /// Resolves a `#[pill_hot]` function's dispatch address by qualified
        /// name, writing its signature hash through `out_signature_hash`.
        ///
        /// Returns zero when this artifact declares no such function, in which
        /// case `out_signature_hash` is left untouched.
        ///
        /// # Safety
        ///
        /// `qualified_name` must be a valid NUL-terminated C string that stays
        /// readable for the call. `out_signature_hash` must be null or point at
        /// a writable `u64`.
        #gate
        #[no_mangle]
        pub unsafe extern "C" fn #export_name(
            qualified_name: *const ::core::ffi::c_char,
            out_signature_hash: *mut u64,
        ) -> usize {
            // SAFETY: the caller upholds the contract documented above.
            unsafe {
                ::pill_engine::hot_patch::with_qualified_name(qualified_name, 0, |name| {
                    match ::pill_engine::hot_patch::resolve_hot_function(name) {
                        Some((address, signature_hash)) => {
                            if !out_signature_hash.is_null() {
                                // SAFETY: the caller guarantees a writable `u64`
                                // when the pointer is non-null.
                                unsafe { *out_signature_hash = signature_hash };
                            }
                            address
                        }
                        None => 0,
                    }
                })
            }
        }
    }
}

// =============================================================================
// Loadable-artifact entry points
// =============================================================================

/// Emits the `pill_module_init` entry point every loadable artifact exports.
///
/// One body serves both attributes. A project and an extension share one DLL
/// contract. They export the same symbols, are loaded the same way, reload
/// through the same transaction, and keep their retired images the same way.
/// So the entry point is written once here, and the caller decides which
/// `#[cfg]` gate, if any, wraps it.
///
/// `gate` is empty in both emissions: a project is always built as its own
/// artifact, and a module's entry points are emitted by the wrapper macro, where
/// `init_fn` is the path `$crate::` plus the annotated function's name.
///
/// `register_fn` is the artifact's registration half, or `no_registration` when
/// it declares none: the entry point runs the components, then that function,
/// then the one-time `init`.
fn module_init_export_with(
    gate: &proc_macro2::TokenStream,
    init_fn: &proc_macro2::TokenStream,
    registration_runner: &proc_macro2::TokenStream,
    register_fn: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        /// Registers this artifact against the host engine; returns zero on
        /// success.
        ///
        /// # Safety
        ///
        /// `api` must be a valid [`EngineApi`] pointer owned by the host and
        /// kept alive for the whole duration of this call.
        ///
        /// [`EngineApi`]: ::pill_engine::EngineApi
        #gate
        #[no_mangle]
        pub unsafe extern "C" fn pill_module_init(api: *const ::pill_engine::EngineApi) -> u32 {
            // A panic must never unwind across the C ABI boundary, so it is
            // converted into a non-zero status and the host keeps the previous
            // generation. `catch_unwind` lives in `std::panic` (not `core`),
            // because unwinding is a std-level feature.
            let result = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                // SAFETY: The host guarantees `api` points at a live `EngineApi`
                // whose `engine_handle` addresses the single engine instance,
                // and that both outlive this call. The engine is not otherwise
                // borrowed while an artifact initializes, so the reconstructed
                // `&mut Engine` is unique.
                let api = unsafe { &*api };
                let engine = unsafe { &mut *(api.engine_handle as *mut ::pill_engine::Engine) };
                // Register this artifact and run its init with the artifact's
                // value types installed for the whole entry point, so a
                // `struct:`-tagged field expands into editable rows even when
                // the artifact registers components itself.
                ::pill_engine::component_registry::#registration_runner(engine, #register_fn, #init_fn)
            }));
            result.unwrap_or(u32::MAX)
        }
    }
}

/// Emit the registration-only entry point of an artifact that declares one.
///
/// The host calls this instead of `pill_module_init` on every generation after
/// the first: the artifact's components come back with the new image, its
/// `register` function re-registers the systems and resources whose code moved
/// with it, and its one-time `init` is not run again. An artifact without a
/// registration function emits nothing here, and the host keeps calling
/// `pill_module_init`.
fn module_register_export(
    gate: &proc_macro2::TokenStream,
    register_fn: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    quote! {
        /// Re-registers this artifact against the host engine for one more
        /// generation; returns zero on success.
        ///
        /// # Safety
        ///
        /// `api` must be a valid [`EngineApi`] pointer owned by the host and
        /// kept alive for the whole duration of this call.
        ///
        /// [`EngineApi`]: ::pill_engine::EngineApi
        #gate
        #[no_mangle]
        pub unsafe extern "C" fn pill_module_register(
            api: *const ::pill_engine::EngineApi,
        ) -> u32 {
            // A panic must never unwind across the C ABI boundary, so it
            // becomes a non-zero status and the host keeps the previous
            // generation.
            let result = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                // SAFETY: The host guarantees `api` points at a live `EngineApi`
                // whose `engine_handle` addresses the single engine instance,
                // and that both outlive this call. The engine is not otherwise
                // borrowed while an artifact registers, so the reconstructed
                // `&mut Engine` is unique.
                let api = unsafe { &*api };
                let engine = unsafe { &mut *(api.engine_handle as *mut ::pill_engine::Engine) };
                ::pill_engine::component_registry::run_artifact_registration(engine, #register_fn)
            }));
            result.unwrap_or(u32::MAX)
        }
    }
}

/// Emit the ABI revision export the host reads before it calls anything else.
///
/// Shared for the same reason [`module_init_export_with`] is: the revision
/// belongs to the one loadable-artifact contract, not to one kind of artifact.
fn module_abi_version_export(gate: &proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    quote! {
        /// Loadable-artifact ABI revision this crate was built against.
        #gate
        const PILL_MODULE_ABI_VERSION: u32 = ::pill_engine::module_abi::MODULE_ABI_VERSION;

        /// ABI revision, checked by the host before anything else is called.
        #gate
        #[no_mangle]
        pub extern "C" fn pill_module_abi_version() -> u32 {
            PILL_MODULE_ABI_VERSION
        }
    }
}

// =============================================================================
// #[pill_module]
// =============================================================================

/// Wraps an extension's `init` function and generates the `pill_module_*`
/// C-ABI exports. The annotated function must have the signature
/// `fn(engine: &mut Engine) -> u32`.
///
/// `#[pill_module(register = path)]` also names the artifact's registration
/// half: the repeatable work the host calls on every generation after the
/// first, so the one-time `init` does not run against a world that already has
/// its entities. Without the argument the artifact exports `pill_module_init`
/// alone, which is what every module built before the split does.
///
/// Before calling it, the macro registers every component that
/// `#[derive(PillComponent)]` declared in this artifact. It also derives the
/// module name from `CARGO_PKG_NAME`, null-terminated, and reads the ABI
/// version from `::pill_engine::module_abi::MODULE_ABI_VERSION`, so module and
/// host cannot drift apart. The whole init runs inside `catch_unwind`, so a
/// panic becomes a non-zero status and the host rolls back instead of unwinding
/// across the C ABI.
///
/// The `#[no_mangle]` exports come from `__pill_module_entry_points!`, the macro
/// this attribute emits. The loadable artifact is the generated
/// `host_module_<name>` wrapper crate. It compiles this crate as a plain library
/// and expands the macro, so a crate linked into several artifacts never defines
/// a symbol more than once. The wrapped function itself is always compiled,
/// because a statically linked build calls it directly.
#[proc_macro_attribute]
pub fn pill_module(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let item_fn = parse_macro_input!(item as ItemFn);
    let fn_ident = &item_fn.sig.ident;

    // `#[pill_module(register = path)]` names the extension's registration
    // half: the repeatable part of its body, which the host calls on every
    // generation after the first. Without it the module keeps the single entry
    // point it always had.
    let mut register_path: Option<syn::Path> = None;
    if !attribute.is_empty() {
        let parse_register = syn::meta::parser(|meta| {
            if meta.path.is_ident("register") {
                register_path = Some(meta.value()?.parse()?);
                return Ok(());
            }
            Err(meta.error("unknown `#[pill_module]` argument; expected `register = path`"))
        });
        parse_macro_input!(attribute with parse_register);
    }

    // The entry points are written once, as the body of the macro below: the
    // loadable artifact is always a generated wrapper crate, which compiles
    // this one as a plain library, so the symbols cannot be emitted by this
    // crate itself - a crate linked into several artifacts would define each
    // one more than once.
    fn entry_points(
        init_fn: &proc_macro2::TokenStream,
        register_fn: &proc_macro2::TokenStream,
        register_export: &proc_macro2::TokenStream,
        name_bytes: &proc_macro2::TokenStream,
    ) -> proc_macro2::TokenStream {
        // The resolver stays behind `debug_assertions`: hot patching is a
        // development facility and a shipped artifact should carry no trace of
        // it.
        let hot_patch_resolver = hot_patch_resolver_export(
            &format_ident!("pill_hot_resolve"),
            &quote! { #[cfg(debug_assertions)] },
        );
        let abi_version_export = module_abi_version_export(&quote! {});
        let init_export = module_init_export_with(
            &quote! {},
            init_fn,
            &quote! { run_artifact_init },
            register_fn,
        );

        quote! {
            #hot_patch_resolver

        #abi_version_export

        /// Name reported to the host for diagnostics; null-terminated for the
        /// C ABI.
        const PILL_MODULE_NAME: &[u8] = #name_bytes;

        /// Human-readable module name used in host log messages.
        #[no_mangle]
        pub extern "C" fn pill_module_name() -> *const ::core::ffi::c_char {
            PILL_MODULE_NAME.as_ptr() as *const ::core::ffi::c_char
        }

        /// Number of `#[derive(PillMirror)]` value-type descriptors this
        /// artifact declares, letting the host size its copy buffer.
        #[no_mangle]
        pub extern "C" fn pill_value_type_descriptor_count() -> u32 {
            ::pill_engine::component_registry::value_type_descriptors().len() as u32
        }

        /// Copy up to `max` value-type descriptors into `out`; returns the
        /// count actually copied. The descriptors (and their inner field
        /// slices) point into this artifact's static data, which stays mapped
        /// for the artifact's lifetime.
        ///
        /// # Safety
        ///
        /// `out` must point at `max` writable
        /// [`PillValueTypeDescriptor`](::pill_engine::component_registry::PillValueTypeDescriptor)
        /// slots owned by the host for the duration of this call.
        #[no_mangle]
        pub unsafe extern "C" fn pill_copy_value_type_descriptors(
            out: *mut ::pill_engine::component_registry::PillValueTypeDescriptor,
            max: u32,
        ) -> u32 {
            // SAFETY: the caller upholds the contract documented above.
            unsafe {
                ::pill_engine::component_registry::copy_descriptors(
                    &::pill_engine::component_registry::value_type_descriptors(),
                    out,
                    max,
                )
            }
        }

        /// Number of `#[pill_mirror_method]` descriptors this artifact
        /// declares, letting the host size its copy buffer.
        #[no_mangle]
        pub extern "C" fn pill_mirror_method_descriptor_count() -> u32 {
            ::pill_engine::component_registry::mirror_method_descriptors().len() as u32
        }

        /// Copy up to `max` mirrored-method descriptors into `out`; returns
        /// the count actually copied. Each descriptor names a `#[no_mangle]`
        /// trampoline this artifact exports, which the host resolves by symbol
        /// to obtain the callable address.
        ///
        /// # Safety
        ///
        /// `out` must point at `max` writable
        /// [`PillMethodDescriptor`](::pill_engine::component_registry::PillMethodDescriptor)
        /// slots owned by the host for the duration of this call.
        #[no_mangle]
        pub unsafe extern "C" fn pill_copy_mirror_method_descriptors(
            out: *mut ::pill_engine::component_registry::PillMethodDescriptor,
            max: u32,
        ) -> u32 {
            // SAFETY: the caller upholds the contract documented above.
            unsafe {
                ::pill_engine::component_registry::copy_descriptors(
                    &::pill_engine::component_registry::mirror_method_descriptors(),
                    out,
                    max,
                )
            }
        }

        /// Number of heap-field accessor descriptors this artifact declares,
        /// letting the host size its copy buffer.
        #[no_mangle]
        pub extern "C" fn pill_field_accessor_descriptor_count() -> u32 {
            ::pill_engine::component_registry::field_accessor_descriptors().len() as u32
        }

        /// Copy up to `max` heap-field accessor descriptors into `out`;
        /// returns the count actually copied. Each descriptor names the
        /// `#[no_mangle]` trampolines this artifact exports for one `Vec` or
        /// `String` component field, which the host resolves by symbol to
        /// obtain the callable addresses.
        ///
        /// # Safety
        ///
        /// `out` must point at `max` writable
        /// [`PillFieldAccessorDescriptor`](::pill_engine::component_registry::PillFieldAccessorDescriptor)
        /// slots owned by the host for the duration of this call.
        #[no_mangle]
        pub unsafe extern "C" fn pill_copy_field_accessor_descriptors(
            out: *mut ::pill_engine::component_registry::PillFieldAccessorDescriptor,
            max: u32,
        ) -> u32 {
            // SAFETY: the caller upholds the contract documented above.
            unsafe {
                ::pill_engine::component_registry::copy_descriptors(
                    &::pill_engine::component_registry::field_accessor_descriptors(),
                    out,
                    max,
                )
            }
        }

            #init_export

            #register_export
        }
    }

    let (register_fn, register_export) = match &register_path {
        Some(path) => (
            quote! { $crate::#path },
            module_register_export(&quote! {}, &quote! { $crate::#path }),
        ),
        None => (
            // Fully qualified: the entry points expand inside the generated
            // `host_module_<name>` wrapper crate, where nothing of this crate
            // is in scope.
            quote! { ::pill_engine::component_registry::no_registration },
            quote! {},
        ),
    };

    let entry_point_items = entry_points(
        &quote! { $crate::#fn_ident },
        &register_fn,
        &register_export,
        &quote! { $crate::PILL_MODULE_NAME_BYTES },
    );

    let expanded = quote! {
        // Emitted unconditionally: a statically linked build calls this
        // directly, and without it a shipping binary would have no way to
        // initialize the module it just linked in.
        #item_fn

        /// Null-terminated crate name the entry-point macro reports, so the
        /// artifact the host loads carries the extension's name rather than a
        /// wrapper's package name.
        #[doc(hidden)]
        pub const PILL_MODULE_NAME_BYTES: &[u8] =
            ::core::concat!(::core::env!("CARGO_PKG_NAME"), "\0").as_bytes();

        /// Expands to this crate's loadable-artifact entry points.
        ///
        /// The loadable artifact is the generated `host_module_<name>` wrapper
        /// crate, which compiles the extension as a plain library and expands
        /// this macro: the entry points cannot come from this crate directly,
        /// because a crate linked into several artifacts would define each
        /// `#[no_mangle]` symbol more than once. `$crate` roots everything the
        /// expansion generates back in this crate, its name included.
        #[doc(hidden)]
        #[macro_export]
        macro_rules! __pill_module_entry_points {
            () => {
                #entry_point_items
            };
        }
    };

    expanded.into()
}

// =============================================================================
// #[pill_project]
// =============================================================================

/// Wraps a project's `init` function and generates the loadable-artifact exports
/// (`pill_module_init`, `pill_module_abi_version`) plus the project-only
/// `project_schema_fingerprint`.
///
/// The annotated function must have the signature
/// `fn(engine: &mut Engine) -> u32`. Before calling it, the macro registers
/// every component that `#[derive(PillComponent)]` declared in this artifact.
/// It also generates the schema fingerprint from the same registry, so adding a
/// component cannot leave the fingerprint stale.
///
/// The entry points are [`pill_module`]'s, because a project and an extension
/// follow one DLL contract and are loaded and reloaded the same way. Two things
/// are project-specific: the schema fingerprint, and the exports that carry no
/// `#[cfg]` gate. A project is always built as its own artifact, so there is no
/// second definition to collide with.
#[proc_macro_attribute]
pub fn pill_project(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let item_fn = parse_macro_input!(item as ItemFn);
    let fn_ident = &item_fn.sig.ident;

    // `#[pill_project(register = path)]` names the project's registration
    // function: the half a reload repeats once `init` has run for the last
    // time. Without it the project keeps the one entry point it always had.
    let mut register_path: Option<syn::Path> = None;
    if !attribute.is_empty() {
        let parse_register = syn::meta::parser(|meta| {
            if meta.path.is_ident("register") {
                register_path = Some(meta.value()?.parse()?);
                return Ok(());
            }
            Err(meta.error("unknown `#[pill_project]` argument; expected `register = path`"))
        });
        parse_macro_input!(attribute with parse_register);
    }

    // Debug-only: a released project keeps no hot-patching surface.
    let hot_patch_resolver = hot_patch_resolver_export(
        &format_ident!("pill_hot_resolve"),
        &quote! { #[cfg(debug_assertions)] },
    );
    // The same loadable-artifact contract a module emits, ungated: a project is
    // always built as its own artifact, so there is no second definition to
    // collide with.
    let ungated = quote! {};
    let abi_version_export = module_abi_version_export(&ungated);
    let (register_fn, register_export) = match &register_path {
        Some(path) => (
            quote! { #path },
            module_register_export(&ungated, &quote! { #path }),
        ),
        None => (
            quote! { ::pill_engine::component_registry::no_registration },
            quote! {},
        ),
    };
    let init_export = module_init_export_with(
        &ungated,
        &quote! { #fn_ident },
        &quote! { run_project_init },
        &register_fn,
    );

    let expanded = quote! {
        #item_fn

        #hot_patch_resolver

        #abi_version_export

        #init_export

        #register_export

        /// Aggregate schema fingerprint of every persistable component,
        /// computed from the compile-time registry.
        #[no_mangle]
        pub extern "C" fn project_schema_fingerprint() -> u64 {
            ::pill_engine::component_registry::persistable_schema_fingerprint()
        }
    };

    expanded.into()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod decorator_vocabulary_tests {
    use super::*;

    /// Every decorator the vocabulary lists is one the parser accepts, and the
    /// refusal text names every listed one.
    ///
    /// Both checks read the one vocabulary list. So the test fails when the
    /// parser stops accepting a listed decorator, and when the refusal text
    /// stops naming one.
    #[test]
    fn the_refusal_text_lists_exactly_the_accepted_decorators() {
        for (key, usage) in FIELD_DECORATOR_USAGE {
            let source = format!("struct Probe {{ #[pill({usage})] value: f32 }}");
            let probe: syn::DeriveInput =
                syn::parse_str(&source).expect("the usage is a valid field declaration");
            let syn::Data::Struct(data) = &probe.data else {
                panic!("the probe is a struct");
            };
            let field = data
                .fields
                .iter()
                .next()
                .expect("the probe declares one field");
            let emitted = parse_field_decorators(&field.attrs)
                .unwrap_or_else(|error| panic!("`{key}` is listed but refused: {error}"));
            assert!(
                !emitted.is_empty(),
                "`{key}` is listed but emitted no decorator"
            );
        }

        let message = field_decorator_list();
        for (_, usage) in FIELD_DECORATOR_USAGE {
            assert!(message.contains(usage), "the refusal text names `{usage}`");
        }
    }
}
