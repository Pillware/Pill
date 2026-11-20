//! `#[engine_error]` - one error definition that produces both the plain and
//! the styled form of its message.
//!
//! # Responsibilities
//!
//! - Parse the `#[engine_error]` attribute and the message language inside
//!   `#[message(...)]`.
//! - Generate the plain `thiserror::Error` message from the semantic nodes, so
//!   the display text never has to be written twice.
//! - Generate `miette::Diagnostic` implementations whose diagnostic codes come
//!   from the namespace and the variant name, unless the variant sets one.
//! - Generate `EngineMessage` implementations that render the semantic message
//!   into a `MessageRenderer`.
//!
//! # Design
//!
//! A `#[message(...)]` definition becomes a list of `MessageNode` values:
//! plain text, a styled token in one of four roles (`general_style`,
//! `specific_style`, `module_style`, `name_style`) holding either a fixed
//! string or a field, or a plain dynamic value (`value` / `debug_value`). A
//! `#[transparent]` variant instead forwards both renderings to the error it
//! wraps, which is the only way to build one message out of another. A
//! diagnostic code is the namespace plus the variant name in snake case,
//! unless `#[code(...)]` replaces the suffix.

extern crate proc_macro;

// Standard library
use std::collections::HashSet;

// External crates
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::parse::ParseStream;
use syn::punctuated::Punctuated;
use syn::{
    parenthesized, parse_macro_input, Attribute, Fields, Ident, ItemEnum, LitStr, Meta, Path,
    Token, Variant,
};

// =============================================================================
// Macro Arguments
// =============================================================================

/// Arguments accepted by `#[engine_error(...)]`.
struct MacroArguments {
    /// Diagnostic-code namespace, e.g. `host::build`.
    namespace_path: Path,
    /// Path of the diagnostics runtime module used by generated code.
    runtime_path: Path,
}

impl syn::parse::Parse for MacroArguments {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut namespace_path: Option<Path> = None;
        let mut runtime_path: Option<Path> = None;

        // Step 1: Consume `key = value` pairs until the argument list ends.
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "namespace" => namespace_path = Some(Path::parse_mod_style(input)?),
                "runtime" => runtime_path = Some(Path::parse_mod_style(input)?),
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown `engine_error` argument `{other}`"),
                    ));
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        // Step 2: `namespace` is required. `runtime` defaults to the
        // crate-local diagnostics module.
        let namespace_path = namespace_path
            .ok_or_else(|| input.error("missing `namespace = ...` argument for `engine_error`"))?;
        let runtime_path = runtime_path.unwrap_or_else(|| syn::parse_quote!(crate::error));
        Ok(Self {
            namespace_path,
            runtime_path,
        })
    }
}

// =============================================================================
// Message AST
// =============================================================================

/// Semantic role of one message token.
///
/// The variants match the diagnostics runtime's `SemanticRole` by name, so the
/// generated code can name the variants the runtime draws.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SemanticRole {
    /// A token written with `general_style`.
    General,
    /// A token written with `specific_style`.
    Specific,
    /// A token written with `module_style`.
    Module,
    /// A token written with `name_style`.
    Name,
}

impl SemanticRole {
    /// Name of this role's variant in the diagnostic runtime enum.
    fn runtime_variant_name(self) -> &'static str {
        match self {
            SemanticRole::General => "General",
            SemanticRole::Specific => "Specific",
            SemanticRole::Module => "Module",
            SemanticRole::Name => "Name",
        }
    }
}

/// Display format of a dynamic message value.
///
/// Chooses which formatting trait a dynamic value uses, both in the plain
/// message and in the semantic renderer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ValueFormat {
    /// Render the value with `Display`.
    Display,
    /// Render the value with `Debug` (`{:?}`).
    Debug,
}

/// One parsed node of a `#[message(...)]` definition.
enum MessageNode {
    /// `"plain text"`
    Text(LitStr),
    /// `general_style("Renderer")` or any other role call holding a fixed
    /// string.
    StaticStyled { role: SemanticRole, text: LitStr },
    /// `name_style(field)`, `value(field)`, or `debug_value(field)`.
    FieldStyled {
        role: Option<SemanticRole>,
        field: Ident,
        format: ValueFormat,
    },
}

/// Argument of one role call in the message language: a fixed string or a
/// field name.
///
/// A fixed string becomes a constant token. A field name becomes a dynamic
/// value that the renderer draws with the role around it.
enum StyleArgument {
    /// A string literal, e.g. `general_style("Renderer")`.
    Static(LitStr),
    /// A field name, e.g. `name_style(path)`.
    Field(Ident),
}

impl syn::parse::Parse for MessageNode {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Step 1: A bare string literal is a plain-text node.
        if input.peek(LitStr) {
            return Ok(MessageNode::Text(input.parse()?));
        }

        // Step 2: Otherwise read a call `function(argument)`. The argument is
        // either a fixed string or a field name.
        let function_name: Ident = input.parse()?;
        let content;
        parenthesized!(content in input);
        let argument = if content.peek(LitStr) {
            StyleArgument::Static(content.parse()?)
        } else if content.peek(Ident) {
            StyleArgument::Field(content.parse()?)
        } else {
            return Err(content.error("expected a string literal or a field name"));
        };

        // Step 3: Pick the node by the function name. An unknown name gets a
        // spelling hint.
        match function_name.to_string().as_str() {
            "general_style" => Ok(styled_node(SemanticRole::General, argument)),
            "specific_style" => Ok(styled_node(SemanticRole::Specific, argument)),
            "module_style" => Ok(styled_node(SemanticRole::Module, argument)),
            "name_style" => Ok(styled_node(SemanticRole::Name, argument)),
            "value" => Ok(plain_node(argument, ValueFormat::Display)),
            "debug_value" => Ok(plain_node(argument, ValueFormat::Debug)),
            other => {
                let hint = did_you_mean(other).map_or_else(String::new, |candidate| {
                    format!("; help: did you mean `{candidate}`?")
                });
                Err(syn::Error::new(
                    function_name.span(),
                    format!("unknown message function `{other}`{hint}"),
                ))
            }
        }
    }
}

/// Build one styled token, either fixed or taken from a field.
fn styled_node(role: SemanticRole, argument: StyleArgument) -> MessageNode {
    match argument {
        StyleArgument::Static(text) => MessageNode::StaticStyled { role, text },
        StyleArgument::Field(field) => MessageNode::FieldStyled {
            role: Some(role),
            field,
            format: ValueFormat::Display,
        },
    }
}

/// Build one value node carrying no semantic role.
fn plain_node(argument: StyleArgument, format: ValueFormat) -> MessageNode {
    match argument {
        StyleArgument::Static(text) => MessageNode::Text(text),
        StyleArgument::Field(field) => MessageNode::FieldStyled {
            role: None,
            field,
            format,
        },
    }
}

/// Closest message function name to `given_function_name`, when it is close
/// enough to be a typo.
fn did_you_mean(given_function_name: &str) -> Option<&'static str> {
    const FUNCTION_NAME_CANDIDATES: [&str; 6] = [
        "general_style",
        "specific_style",
        "module_style",
        "name_style",
        "value",
        "debug_value",
    ];
    FUNCTION_NAME_CANDIDATES
        .into_iter()
        .filter(|candidate| edit_distance(given_function_name, candidate) <= 2)
        .min_by_key(|candidate| edit_distance(given_function_name, candidate))
}

/// Classic Levenshtein distance between two strings.
fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0_usize; right.len() + 1];
    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right.iter().enumerate() {
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + usize::from(left_char != *right_char));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

// =============================================================================
// Expansion
// =============================================================================

/// Everything the macro extracted from one variant.
struct VariantInfo {
    /// Attributes passed through to the generated enum.
    attributes: Vec<Attribute>,
    /// Parsed `#[message(...)]` nodes, when present.
    message: Option<Vec<MessageNode>>,
    /// Whether `#[transparent]` was present.
    is_transparent: bool,
    /// `#[code(...)]` override for the generated diagnostic code.
    code_override: Option<Ident>,
    /// User-supplied `#[diagnostic(...)]` attribute, kept to merge the code.
    user_diagnostic: Option<Attribute>,
    /// Whether the user's diagnostic attribute already declares a code.
    user_declares_code: bool,
}

/// Split one variant's attributes into the ones the macro consumes and the
/// ones it copies to the generated enum.
fn classify_variant(variant: &Variant) -> syn::Result<VariantInfo> {
    let mut attributes = Vec::new();
    let mut message: Option<Vec<MessageNode>> = None;
    let mut is_transparent = false;
    let mut code_override = None;
    let mut user_diagnostic = None;
    let mut user_declares_code = false;

    for attribute in &variant.attrs {
        if attribute.path().is_ident("message") {
            if message.is_some() {
                return Err(syn::Error::new_spanned(
                    attribute,
                    "duplicate `#[message(...)]` attribute",
                ));
            }
            message = Some(
                attribute
                    .parse_args_with(Punctuated::<MessageNode, Token![,]>::parse_terminated)?
                    .into_iter()
                    .collect(),
            );
        } else if attribute.path().is_ident("transparent") {
            is_transparent = true;
        } else if attribute.path().is_ident("code") {
            code_override = Some(attribute.parse_args()?);
        } else if attribute.path().is_ident("diagnostic") {
            // Read the user's diagnostic arguments to find an explicit
            // `code(...)`. When the macro adds its own code, it merges it into
            // this attribute, so the derive still sees one `#[diagnostic]`.
            if let Ok(arguments) =
                attribute.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            {
                user_declares_code = arguments
                    .iter()
                    .any(|argument| argument.path().is_ident("code"));
            }
            user_diagnostic = Some(attribute.clone());
        } else {
            attributes.push(attribute.clone());
        }
    }

    Ok(VariantInfo {
        attributes,
        message,
        is_transparent,
        code_override,
        user_diagnostic,
        user_declares_code,
    })
}

/// Build the plain `#[error(...)]` format string from the semantic nodes.
fn build_format_string(nodes: &[MessageNode]) -> String {
    let mut format_string = String::new();
    for node in nodes {
        match node {
            MessageNode::Text(text) | MessageNode::StaticStyled { text, .. } => {
                format_string.push_str(&text.value());
            }
            MessageNode::FieldStyled {
                field,
                format: ValueFormat::Display,
                ..
            } => {
                format_string.push('{');
                format_string.push_str(&field.to_string());
                format_string.push('}');
            }
            MessageNode::FieldStyled {
                field,
                format: ValueFormat::Debug,
                ..
            } => {
                format_string.push('{');
                format_string.push_str(&field.to_string());
                format_string.push_str(":?}");
            }
        }
    }
    format_string
}

/// Convert a variant name to snake case for the diagnostic code suffix.
fn to_snake_case(ident: &Ident) -> String {
    let name = ident.to_string();
    let mut snake_case_name = String::with_capacity(name.len() + 4);
    let mut previous_uppercase = false;
    for character in name.chars() {
        if character.is_ascii_uppercase() {
            if !snake_case_name.is_empty() && !previous_uppercase {
                snake_case_name.push('_');
            }
            snake_case_name.push(character.to_ascii_lowercase());
            previous_uppercase = true;
        } else {
            snake_case_name.push(character);
            previous_uppercase = false;
        }
    }
    snake_case_name
}

/// Join the namespace path segments with `::` for the diagnostic code prefix.
fn namespace_string(path: &Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// Check that every field a message names exists, and return the names in the
/// order they first appear.
fn referenced_fields(variant: &Variant, nodes: &[MessageNode]) -> syn::Result<Vec<Ident>> {
    let named_fields: HashSet<String> = match &variant.fields {
        Fields::Named(named) => named
            .named
            .iter()
            .filter_map(|field| field.ident.as_ref())
            .map(|ident| ident.to_string())
            .collect(),
        _ => HashSet::new(),
    };

    let mut referenced_names: Vec<Ident> = Vec::new();
    for node in nodes {
        let MessageNode::FieldStyled { field, .. } = node else {
            continue;
        };
        if !named_fields.contains(&field.to_string()) {
            if matches!(variant.fields, Fields::Unnamed(_)) {
                return Err(syn::Error::new(
                    field.span(),
                    "styled field references require named fields; give the variant named fields",
                ));
            }
            return Err(syn::Error::new(
                field.span(),
                format!(
                    "`{field}` references an unknown field on variant `{}`",
                    variant.ident
                ),
            ));
        }
        if !referenced_names.iter().any(|existing| existing == field) {
            referenced_names.push(field.clone());
        }
    }
    Ok(referenced_names)
}

/// Generate one `EngineMessage` match arm for a leaf message variant.
fn build_message_arm(
    runtime_path: &Path,
    variant: &Variant,
    nodes: &[MessageNode],
    referenced_fields: &[Ident],
) -> TokenStream2 {
    let ident = &variant.ident;
    let pattern = match &variant.fields {
        Fields::Unit => quote!(),
        Fields::Named(_) => {
            if referenced_fields.is_empty() {
                quote!({ .. })
            } else {
                quote!({ #(#referenced_fields,)* .. })
            }
        }
        Fields::Unnamed(_) => quote!((..)),
    };

    let steps = build_message_steps(runtime_path, nodes);
    let cfg_attributes = cfg_attributes(variant);

    quote! {
        #(#cfg_attributes)* Self::#ident #pattern => { #(#steps;)* Ok(()) },
    }
}

/// Build the renderer calls for one leaf variant's semantic message.
fn build_message_steps(runtime_path: &Path, nodes: &[MessageNode]) -> Vec<TokenStream2> {
    nodes
        .iter()
        .map(|node| {
            let step = match node {
                MessageNode::Text(text) => quote!(renderer.text(#text)),
                MessageNode::StaticStyled { role, text } => {
                    let role_ident =
                        Ident::new(role.runtime_variant_name(), proc_macro2::Span::call_site());
                    quote!(renderer.styled(#runtime_path::SemanticRole::#role_ident, &#text))
                }
                MessageNode::FieldStyled {
                    role: Some(role),
                    field,
                    ..
                } => {
                    let role_ident =
                        Ident::new(role.runtime_variant_name(), proc_macro2::Span::call_site());
                    quote!(renderer.styled(#runtime_path::SemanticRole::#role_ident, #field))
                }
                MessageNode::FieldStyled {
                    role: None,
                    field,
                    format: ValueFormat::Display,
                } => quote!(renderer.value(#field)),
                MessageNode::FieldStyled {
                    role: None,
                    field,
                    format: ValueFormat::Debug,
                } => quote!(renderer.debug_value(#field)),
            };
            quote!(#step?)
        })
        .collect()
}

/// Collect one variant's `#[cfg(...)]` attributes.
///
/// They are copied onto the generated match arm, so a feature that removes the
/// variant removes its arm too.
fn cfg_attributes(variant: &Variant) -> Vec<&Attribute> {
    variant
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("cfg"))
        .collect()
}

/// Generate one `EngineMessage` match arm for a transparent variant.
fn build_transparent_arm(runtime_path: &Path, variant: &Variant) -> syn::Result<TokenStream2> {
    let ident = &variant.ident;
    let cfg_attributes = cfg_attributes(variant);
    match &variant.fields {
        Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => Ok(
            quote!(#(#cfg_attributes)* Self::#ident(source) => #runtime_path::EngineMessage::render_message(source, renderer),),
        ),
        Fields::Named(named) if named.named.len() == 1 => {
            let field = named
                .named
                .first()
                .and_then(|field| field.ident.as_ref())
                .ok_or_else(|| {
                    syn::Error::new_spanned(named, "transparent variant field needs a name")
                })?;
            Ok(
                quote!(#(#cfg_attributes)* Self::#ident { #field, .. } => #runtime_path::EngineMessage::render_message(#field, renderer),),
            )
        }
        _ => Err(syn::Error::new_spanned(
            variant,
            "`#[transparent]` requires exactly one wrapped error field",
        )),
    }
}

/// Collect the traits the enum already derives.
fn existing_derives(error_enum: &ItemEnum) -> HashSet<String> {
    let mut found = HashSet::new();
    for attribute in error_enum
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("derive"))
    {
        let Ok(paths) = attribute.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)
        else {
            continue;
        };
        for path in paths {
            if let Some(segment) = path.segments.last() {
                found.insert(segment.ident.to_string());
            }
        }
    }
    found
}

// =============================================================================
// Entry Point
// =============================================================================

/// Generates the `thiserror` display text, the `miette` diagnostic codes and
/// the `EngineMessage` rendering for one error enum.
///
/// Every variant carries its message as a `#[message(...)]` definition. A
/// variant that wraps another error uses `#[transparent]` instead.
///
/// # Errors
///
/// The macro reports a compile error when:
///
/// - `namespace` is missing or an argument name is unknown.
/// - A variant has neither `#[message(...)]` nor `#[transparent]`.
/// - A variant mixes `#[message(...)]` with `#[transparent]`.
/// - A variant repeats `#[message(...)]`.
/// - A `#[message(...)]` is empty, references an unknown field, or styles a
///   field of an unnamed variant.
/// - A `#[transparent]` variant does not wrap exactly one error field.
///
/// # Debug aid
///
/// Set `ENGINE_ERROR_MACRO_DEBUG=1` to write the expansion of each enum to its
/// own file (`engine_error_<Enum>.rs`) and print that path to the build log.
/// The file goes to the directory named by `ENGINE_ERROR_MACRO_DEBUG_DIR` when
/// that variable is set, and to the system temp directory otherwise. It is
/// never written to the compiler's working directory.
///
/// # Examples
///
/// ```ignore
/// #[engine_error(namespace = host::build)]
/// pub enum BuildError {
///     #[message("failed to link {general_style(crate_name)}")]
///     LinkFailed { crate_name: String },
///
///     #[transparent]
///     Toolchain(#[from] ToolchainError),
/// }
/// ```
#[proc_macro_attribute]
pub fn engine_error(attribute: TokenStream, item_tokens: TokenStream) -> TokenStream {
    let macro_arguments = parse_macro_input!(attribute as MacroArguments);
    let mut error_enum = parse_macro_input!(item_tokens as ItemEnum);
    expand_engine_error(macro_arguments, &mut error_enum)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Runs the expansion behind the `engine_error` attribute entry point.
///
/// It rewrites every variant: the helper attributes the macro consumes are
/// removed and the generated `#[error(...)]` and `#[diagnostic(...)]`
/// attributes are added. It checks the message definitions, then emits the enum
/// together with its `EngineMessage` implementation.
fn expand_engine_error(
    macro_arguments: MacroArguments,
    error_enum: &mut ItemEnum,
) -> syn::Result<TokenStream2> {
    let namespace = namespace_string(&macro_arguments.namespace_path);

    // Step 1: Rewrite every variant and build the match arms that render it.
    let mut variants = Vec::new();
    let mut arms: Vec<TokenStream2> = Vec::new();
    for variant in &error_enum.variants {
        let variant_info = classify_variant(variant)?;
        if variant_info.message.is_none() && !variant_info.is_transparent {
            return Err(syn::Error::new_spanned(
                variant,
                "error variant must have either `#[message(...)]` or `#[transparent]`",
            ));
        }
        if variant_info.message.is_some() && variant_info.is_transparent {
            return Err(syn::Error::new_spanned(
                variant,
                "error variant cannot be both `#[message(...)]` and `#[transparent]`",
            ));
        }

        let mut generated_variant = variant.clone();
        generated_variant.attrs = variant_info.attributes;

        if variant_info.is_transparent {
            generated_variant
                .attrs
                .push(syn::parse_quote!(#[error(transparent)]));
            generated_variant
                .attrs
                .push(syn::parse_quote!(#[diagnostic(transparent)]));
            arms.push(build_transparent_arm(
                &macro_arguments.runtime_path,
                variant,
            )?);
        } else {
            let nodes = variant_info.message.expect("validated above");
            if nodes.is_empty() {
                return Err(syn::Error::new_spanned(
                    variant,
                    "`#[message(...)]` must not be empty",
                ));
            }
            let referenced_fields = referenced_fields(variant, &nodes)?;
            let format_string = build_format_string(&nodes);
            generated_variant
                .attrs
                .push(syn::parse_quote!(#[error(#format_string)]));

            // Build the diagnostic code from the namespace and the variant
            // name. `#[code(...)]` replaces the variant name part, and a code
            // written on the author's `#[diagnostic(...)]` replaces it whole.
            if !variant_info.user_declares_code {
                let code_suffix = variant_info
                    .code_override
                    .as_ref()
                    .map_or_else(|| to_snake_case(&variant.ident), |ident| ident.to_string());
                let diagnostic_code = format!("{namespace}::{code_suffix}");
                match &variant_info.user_diagnostic {
                    Some(attribute) => {
                        // Merge the generated code into the user's diagnostic
                        // arguments so the derive sees a single attribute.
                        if let Meta::List(list) = &attribute.meta {
                            let inner = &list.tokens;
                            generated_variant.attrs.push(syn::parse_quote!(
                                #[diagnostic(code(#diagnostic_code), #inner)]
                            ));
                        } else {
                            generated_variant
                                .attrs
                                .push(syn::parse_quote!(#[diagnostic(code(#diagnostic_code))]));
                            generated_variant.attrs.push(attribute.clone());
                        }
                    }
                    None => {
                        generated_variant
                            .attrs
                            .push(syn::parse_quote!(#[diagnostic(code(#diagnostic_code))]));
                    }
                }
            } else if let Some(attribute) = &variant_info.user_diagnostic {
                generated_variant.attrs.push(attribute.clone());
            }

            arms.push(build_message_arm(
                &macro_arguments.runtime_path,
                variant,
                &nodes,
                &referenced_fields,
            ));
        }

        variants.push(generated_variant);
    }
    error_enum.variants = Punctuated::from_iter(variants);

    // Step 2: Add the derives the macro needs, skipping the ones the author
    // already wrote.
    let derives = existing_derives(error_enum);
    if !derives.contains("Error") {
        error_enum
            .attrs
            .push(syn::parse_quote!(#[derive(::thiserror::Error)]));
    }
    if !derives.contains("Diagnostic") {
        error_enum
            .attrs
            .push(syn::parse_quote!(#[derive(::miette::Diagnostic)]));
    }
    if !derives.contains("Debug") {
        error_enum
            .attrs
            .push(syn::parse_quote!(#[derive(::core::fmt::Debug)]));
    }

    // Step 3: Emit the enum plus its semantic rendering implementation.
    let enum_ident = &error_enum.ident;
    let (impl_generics, type_generics, where_clause) = error_enum.generics.split_for_impl();
    let runtime_path = &macro_arguments.runtime_path;

    let expanded_enum = quote! {
        #error_enum

        impl #impl_generics #runtime_path::EngineMessage for #enum_ident #type_generics #where_clause {
            fn render_message(
                &self,
                renderer: &mut dyn #runtime_path::MessageRenderer,
            ) -> ::std::fmt::Result {
                match self {
                    #(#arms)*
                }
            }
        }
    };

    // Debug aid: when `ENGINE_ERROR_MACRO_DEBUG` is set, write this enum's
    // expansion to a file so it can be read outside the build. The file goes
    // to `ENGINE_ERROR_MACRO_DEBUG_DIR` or the system temp directory, never to
    // the compiler's working directory, where an untracked `.rs` next to the
    // workspace manifest would be a surprise. The path is printed because a
    // proc macro has no other way to say anything.
    if std::env::var_os("ENGINE_ERROR_MACRO_DEBUG").is_some() {
        let directory = std::env::var_os("ENGINE_ERROR_MACRO_DEBUG_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dump_path = directory.join(format!("engine_error_{}.rs", enum_ident));
        match std::fs::write(&dump_path, expanded_enum.to_token_stream().to_string()) {
            Ok(()) => eprintln!("engine_error: wrote {}", dump_path.display()),
            Err(error) => eprintln!(
                "engine_error: could not write {}: {error}",
                dump_path.display()
            ),
        }
    }

    Ok(expanded_enum.into_token_stream())
}
