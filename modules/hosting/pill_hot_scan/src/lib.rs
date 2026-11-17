//! Minimal source scanning for patch classification and generation.
//!
//! # Responsibilities
//!
//! - Locate one function's signature and body by name.
//! - Enumerate every addressable function, with the type and trait that own it.
//! - Produce the normalized declaration the host and the build scripts compare.
//! - Collect a file's top-level `use` statements.
//! - Strip named function bodies, so a body-only edit can be told from any other.
//!
//! # Design
//!
//! The host and each crate's build script must agree exactly on where a
//! function starts and what its declaration says. The host uses that answer to
//! decide whether an edit can be patched; the build script uses it to record
//! the addresses and signatures the host later asks for. Sharing one
//! implementation keeps the two answers identical.
//!
//! The scanner reads bytes instead of building a syntax tree. Matching braces
//! over a mask of real-code bytes answers the only two questions asked: where
//! a function starts and ends, and what changed outside the bodies. A syntax
//! tree would add a full parse to every save-triggered classification and
//! would still need byte offsets to copy text unchanged.
//!
//! When the scanner cannot identify something with confidence, classification
//! reports "not body-only" and pays a full module reload rather than risk a
//! wrong patch. The crate has no dependencies, because a build script runs
//! before everything else and every dependency would be paid on every clean
//! build.

// Standard library
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// =============================================================================
// Constants
// =============================================================================

/// Attributes that turn a function into the module ABI's entry point.
///
/// The host calls such a function through the loaded library's export table at
/// load time, never through a redirect, so an address registered for it could
/// never be used by a patch.
///
/// Matched as a suffix so both `#[pill_module]` and a path-qualified
/// `#[pill_engine::pill_module]` are recognized.
const ABI_ENTRY_POINT_ATTRIBUTES: &[&str] = &["pill_module]", "pill_project]"];

// =============================================================================
// Code mask
// =============================================================================

/// Marks which bytes of `source` are real code.
///
/// The result is `false` inside string literals and comments, so a brace in a
/// string or a `//` inside one can never confuse brace matching. Character
/// literals are not recognized at all: every byte of one stays code, including
/// a brace written inside it.
pub fn code_mask(source: &str) -> Vec<bool> {
    let bytes = source.as_bytes();
    let mut mask = vec![true; bytes.len()];
    let mut index = 0usize;

    while index < bytes.len() {
        match bytes[index] {
            // Line comment: masked to end of line.
            b'/' if index + 1 < bytes.len() && bytes[index + 1] == b'/' => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    mask[index] = false;
                    index += 1;
                }
            }
            // Block comment: every byte up to the closing `*/` is masked. Rust
            // allows block comments to nest, so a depth counter is used instead
            // of stopping at the first `*/`.
            b'/' if index + 1 < bytes.len() && bytes[index + 1] == b'*' => {
                let mut depth = 1usize;
                mask[index] = false;
                mask[index + 1] = false;
                index += 2;
                while index < bytes.len() && depth > 0 {
                    if index + 1 < bytes.len() && bytes[index] == b'/' && bytes[index + 1] == b'*' {
                        depth += 1;
                        mask[index] = false;
                        mask[index + 1] = false;
                        index += 2;
                        continue;
                    }
                    if index + 1 < bytes.len() && bytes[index] == b'*' && bytes[index + 1] == b'/' {
                        depth -= 1;
                        mask[index] = false;
                        mask[index + 1] = false;
                        index += 2;
                        continue;
                    }
                    mask[index] = false;
                    index += 1;
                }
            }
            // String literal, including the raw forms `r"..."` and `r#"..."#`.
            b'r' if index + 1 < bytes.len()
                && (bytes[index + 1] == b'"' || bytes[index + 1] == b'#') =>
            {
                let hash_start = index + 1;
                let mut hashes = 0usize;
                while hash_start + hashes < bytes.len() && bytes[hash_start + hashes] == b'#' {
                    hashes += 1;
                }
                if hash_start + hashes >= bytes.len() || bytes[hash_start + hashes] != b'"' {
                    // Just an identifier beginning with `r`.
                    index += 1;
                    continue;
                }
                mask[index] = false;
                index = hash_start + hashes + 1;
                // Scan for the matching `"` followed by the same hash count.
                while index < bytes.len() {
                    if bytes[index] == b'"' {
                        let mut closing = 0usize;
                        while index + 1 + closing < bytes.len()
                            && bytes[index + 1 + closing] == b'#'
                            && closing < hashes
                        {
                            closing += 1;
                        }
                        if closing == hashes {
                            for offset in 0..=hashes {
                                if index + offset < bytes.len() {
                                    mask[index + offset] = false;
                                }
                            }
                            index += hashes + 1;
                            break;
                        }
                    }
                    mask[index] = false;
                    index += 1;
                }
            }
            b'"' => {
                mask[index] = false;
                index += 1;
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        mask[index] = false;
                        if index + 1 < bytes.len() {
                            mask[index + 1] = false;
                        }
                        index += 2;
                        continue;
                    }
                    let closing = bytes[index] == b'"';
                    mask[index] = false;
                    index += 1;
                    if closing {
                        break;
                    }
                }
            }
            _ => index += 1,
        }
    }
    mask
}

/// The matching `}` for the `{` at `open`, searching only real code.
fn matching_brace(bytes: &[u8], mask: &[bool], open: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut index = open + 1;
    while index < bytes.len() {
        if mask[index] {
            match bytes[index] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(index);
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    None
}

/// The first real-code `{` at or after `from`.
fn next_open_brace(bytes: &[u8], mask: &[bool], from: usize) -> Option<usize> {
    (from..bytes.len()).find(|&index| mask[index] && bytes[index] == b'{')
}

// =============================================================================
// Function lookup
// =============================================================================

/// One function located in a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionText {
    /// Byte offset of `fn`, after any attributes.
    pub start: usize,
    /// Byte offset just past the closing brace.
    pub end: usize,
    /// The full declaration, from `fn` through the closing brace.
    pub text: String,
    /// Just the body, braces excluded.
    pub body: String,
}

/// Finds one method's full text, scoped to the `impl` block that owns it.
///
/// [`find_function`] searches by bare name and returns the first match. That
/// cannot tell `impl Default for Alpha` from `impl Default for Beta`, nor an
/// inherent `draw` from a trait `draw`, so generating a patch from it would
/// install the wrong body with no error - the one outcome this crate must
/// avoid.
///
/// `trait_name` of `None` selects the inherent block, so an inherent and a trait
/// method of the same name stay distinguishable.
pub fn find_method(
    source: &str,
    name: &str,
    self_type: &str,
    trait_name: Option<&str>,
) -> Option<FunctionText> {
    let mask = code_mask(source);
    let block = impl_blocks_for_concrete_types(source, &mask)
        .into_iter()
        .find(|block| block.type_name == self_type && block.trait_name.as_deref() == trait_name)?;

    // Search only inside the block's body, then translate the offsets back so
    // the caller receives positions in the original source.
    let (open, close) = block.body_byte_range;
    let found = find_function(&source[open..=close], name)?;
    Some(FunctionText {
        start: found.start + open,
        end: found.end + open,
        text: found.text,
        body: found.body,
    })
}

/// Finds the first `fn <name>` declaration and returns its full text.
///
/// Searches the whole file, so a function nested inside another matches too. A
/// caller that needs a method from its own `impl` block asks [`find_method`].
///
/// Returns `None` when the name does not appear as a function declaration, or
/// when its braces are unbalanced - both of which make the caller fall back to
/// a full reload rather than guess.
pub fn find_function(source: &str, name: &str) -> Option<FunctionText> {
    let mask = code_mask(source);
    find_function_in(source, &mask, name)
}

/// [`find_function`] against a code mask the caller already computed.
///
/// Building the mask scans and allocates for the whole file. Passing one in
/// lets a caller share a single build between several lookups, which
/// classification needs: it asks about both revisions on every save.
///
/// `mask` MUST be `code_mask(source)` for this exact `source`, because it is
/// indexed by the same byte offsets. A mask built from different text silently
/// reports the wrong function bounds.
///
/// # Panics
///
/// Panics in a debug build when `mask` and `source` have different lengths.
pub fn find_function_in(source: &str, mask: &[bool], name: &str) -> Option<FunctionText> {
    let bytes = source.as_bytes();
    debug_assert_eq!(
        mask.len(),
        bytes.len(),
        "the mask must describe this source"
    );
    let needle = format!("fn {name}");
    let needle_bytes = needle.as_bytes();

    let mut search = 0usize;
    while search + needle_bytes.len() <= bytes.len() {
        let candidate = &bytes[search..search + needle_bytes.len()];
        let is_code = mask[search..search + needle_bytes.len()]
            .iter()
            .all(|byte_is_code| *byte_is_code);

        if candidate == needle_bytes && is_code {
            // The character before must be a boundary, so `fn movement` does not
            // match inside `fn movement_extra`.
            let preceded_ok = search == 0
                || matches!(
                    bytes[search - 1],
                    b'\n' | b'\r' | b'\t' | b' ' | b'}' | b';'
                );
            let after = search + needle_bytes.len();
            let followed_ok = after >= bytes.len()
                || matches!(bytes[after], b'(' | b' ' | b'<' | b'\n' | b'\r' | b'\t');

            if preceded_ok && followed_ok {
                let open = next_open_brace(bytes, mask, after)?;
                let close = matching_brace(bytes, mask, open)?;
                return Some(FunctionText {
                    start: search,
                    end: close + 1,
                    text: source[search..=close].to_string(),
                    body: source[open + 1..close].to_string(),
                });
            }
        }
        search += 1;
    }
    None
}

// =============================================================================
// Imports
// =============================================================================

/// Collects the file's top-level `use` statements - the ones outside every
/// block - each copied from `use` through its semicolon.
///
/// A generated patch replays these so the copied function body resolves the
/// same names the original did. A `use` inside a function travels with the body
/// it belongs to.
pub fn top_level_use_statements(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mask = code_mask(source);
    let mut statements = Vec::new();
    let mut depth = 0i32;
    let mut index = 0usize;

    while index < bytes.len() {
        if mask[index] {
            match bytes[index] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                b'u' if depth == 0 => {
                    let statement_start = index;
                    let preceded_ok =
                        index == 0 || matches!(bytes[index - 1], b'\n' | b'\r' | b'\t' | b' ');
                    if preceded_ok && source[index..].starts_with("use ") {
                        // A `use` statement ends at its terminating semicolon.
                        if let Some(offset) = source[index..].find(';') {
                            let end = index + offset + 1;
                            statements.push(source[statement_start..end].trim().to_string());
                            index = end;
                            continue;
                        }
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    statements
}

// =============================================================================
// Hot function discovery
// =============================================================================

/// Which hot-patching route a function's attribute selects.
///
/// The two are redirected through different machinery, so a patch has to know
/// which one it is replacing before it can be generated or installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotFunctionKind {
    /// `#[pill_hot]` - an ECS system, redirected through the hot-function
    /// registry of the artifact that declares it.
    System,
    /// `#[pill_hot_fn]` - a plain function or inherent method, redirected
    /// through its dispatch slot, one per artifact that links the crate. An
    /// un-annotated function carries this kind too, without a slot.
    PlainFunction,
}

/// One function a source file marks as hot-patchable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotFunction {
    /// The function's own name, which is also the last segment of the name the
    /// running artifact registered it under.
    pub name: String,
    /// Which route the annotation selected; `PlainFunction` when nothing is
    /// annotated.
    pub kind: HotFunctionKind,
    /// The type whose `impl` block encloses it, for a method.
    ///
    /// `None` for a free function, and for a method in a block the scanner could
    /// not attribute to one concrete type - a generic `impl`, or a header that
    /// is not a plain path. A patch for a method must name the receiver type, so
    /// such a method is refused rather than generated against a guessed type.
    pub self_type: Option<String>,
    /// The trait whose `impl` block encloses it, for a trait method.
    ///
    /// `None` for a free function and for an inherent method. Two traits may
    /// define the same method name for one type, and a type may have both an
    /// inherent and a trait method of the same name, so a trait method is only
    /// unambiguous when named as `<Type as Trait>::method`.
    pub trait_name: Option<String>,
    /// Whether the declaration takes a receiver.
    pub takes_receiver: bool,
    /// The declaration as written, from `fn` to the body, whitespace collapsed.
    ///
    /// Compared against what the running artifact was built with before a
    /// prologue patch overwrites anything. Both sides normalize the same way, so
    /// reformatting is not mistaken for a change of shape.
    pub signature: String,
    /// Whether the declaration asks to be inlined at every call site.
    ///
    /// `#[inline(always)]` is honoured even at `opt-level = 0`, so such a
    /// function may have no body left to redirect: every caller holds its own
    /// copy, and a prologue patch would reach none of them. The scanner can
    /// detect this case, so the inventory leaves those functions out instead of
    /// promising a patch it cannot deliver.
    pub inline_always: bool,
    /// Whether this function is the module ABI's entry point.
    ///
    /// The host calls it through the loaded library's export table at load time,
    /// never through a redirect, so it is left out of the inventory. Decided by
    /// the attribute that generates the export, not by the function's name,
    /// which is only a convention that a module may break.
    pub is_abi_entry_point: bool,
    /// Whether a `#[cfg(…)]` decides if this declaration exists at all.
    ///
    /// A byte scanner cannot evaluate `cfg`, so a conditionally compiled
    /// function may be absent from the very build being scanned. Registering
    /// one would emit an address for an item that does not exist, which fails
    /// to compile - so they are recorded and then skipped.
    pub cfg_gated: bool,
    /// Whether the developer marked it with a `#[pill_hot…]` attribute.
    ///
    /// The attribute decides how the function is patched; the build script's
    /// address inventory decides whether it can be patched at all. An annotated
    /// function has a dispatch slot, which is preferred because it is a plain
    /// atomic store. An un-annotated function is redirected by overwriting its
    /// prologue, which needs writable code pages.
    pub is_annotated: bool,
}

/// One `impl` block whose methods can be attributed to a named type.
pub struct ImplBlock {
    /// Byte range of the block's body, braces included.
    pub body_byte_range: (usize, usize),
    /// The type the block implements methods on.
    pub type_name: String,
    /// The trait being implemented, for `impl Trait for Type`.
    ///
    /// `None` for an inherent block. Two traits may define the same method name
    /// for one type, so a trait method is only unambiguous when named through
    /// the trait - which is why this is carried rather than discarded.
    pub trait_name: Option<String>,
}

/// Names of the functions this file marks with `#[pill_hot]` or
/// `#[pill_hot_fn]`.
///
/// This is the annotated set alone. What classification treats as replaceable
/// is every function the host can address ([`all_functions`]); everything else
/// in the file counts as a structural change and forces a full reload.
pub fn hot_function_names(source: &str) -> Vec<String> {
    hot_functions(source)
        .into_iter()
        .map(|function| function.name)
        .collect()
}

/// Every hot function in this file, with the attribute that marked it and the
/// type whose `impl` block encloses it.
pub fn hot_functions(source: &str) -> Vec<HotFunction> {
    let mask = code_mask(source);
    let blocks = impl_blocks_for_concrete_types(source, &mask);
    hot_functions_in(source, &mask, &blocks)
}

/// [`hot_functions`] against a code mask and impl blocks a caller already has.
///
/// Building both scans the whole file, so the all-functions scan - which needs
/// the annotated set too - passes its own rather than paying for them twice.
///
/// Two attributes mark a function, and they are not the ones a person writes
/// for the same reason: `#[pill_hot]`/`#[pill_hot_fn]` say "patch this", and
/// `#[system]` says "this is a system", which the engine gives a dispatch slot
/// in every development build. A `#[system]` body is therefore patchable with
/// no marker of its own.
fn hot_functions_in(source: &str, mask: &[bool], blocks: &[ImplBlock]) -> Vec<HotFunction> {
    let bytes = source.as_bytes();
    let mut found_functions: Vec<HotFunction> = Vec::new();
    let mut index = 0usize;

    while index < bytes.len() {
        // Whichever of the two attribute spellings comes first in the rest of
        // the file is the one this pass looks at.
        let next_pill_hot = source[index..]
            .find("#[pill_hot")
            .map(|offset| index + offset);
        let next_system = source[index..]
            .find("#[system")
            .map(|offset| index + offset);
        let attribute_start = match (next_pill_hot, next_system) {
            (Some(left), Some(right)) => left.min(right),
            (Some(offset), None) | (None, Some(offset)) => offset,
            (None, None) => break,
        };
        if !mask[attribute_start] {
            index = attribute_start + 1;
            continue;
        }
        // `#[pill_hot_fn]` shares its prefix with `#[pill_hot]`, so the longer
        // name is tested first; anything else starting with the prefix (a
        // `#[pill_hot(name = "...")]` override, for instance) is a system.
        let kind = if source[attribute_start..].starts_with("#[pill_hot_fn") {
            HotFunctionKind::PlainFunction
        } else if source[attribute_start..].starts_with("#[system") {
            // The name has to end at the attribute, or a longer identifier such
            // as `#[systematic]` would classify as a declaration.
            let after_attribute_name = &source[attribute_start + "#[system".len()..];
            if !after_attribute_name.starts_with(']') && !after_attribute_name.starts_with('(') {
                index = attribute_start + 1;
                continue;
            }
            HotFunctionKind::System
        } else {
            HotFunctionKind::System
        };

        // The declaration follows the attribute (and any further attributes).
        // Scan forward for the next real-code `fn ` and take its name.
        let mut cursor = attribute_start;
        let mut name = None;
        let mut declaration_start = attribute_start;
        while cursor + 3 <= bytes.len() {
            if mask[cursor] && source[cursor..].starts_with("fn ") {
                declaration_start = cursor;
                let name_start = cursor + 3;
                let name_end = source[name_start..]
                    .find(|character: char| !character.is_alphanumeric() && character != '_')
                    .map(|end| name_start + end)
                    .unwrap_or(source.len());
                if name_end > name_start {
                    name = Some(source[name_start..name_end].to_string());
                }
                break;
            }
            // An opening brace before any `fn` means the attribute belonged to
            // something else; stop rather than guess.
            if mask[cursor] && bytes[cursor] == b'{' {
                break;
            }
            cursor += 1;
        }

        if let Some(name) = name {
            if !found_functions.iter().any(|existing| existing.name == name) {
                // The innermost `impl` body containing this declaration, which
                // is the type a generated patch must name.
                let enclosing_impl_block = blocks
                    .iter()
                    .filter(|block| {
                        block.body_byte_range.0 < declaration_start
                            && declaration_start < block.body_byte_range.1
                    })
                    .min_by_key(|block| block.body_byte_range.1 - block.body_byte_range.0);
                let self_type = enclosing_impl_block.map(|block| block.type_name.clone());
                let trait_name = enclosing_impl_block.and_then(|block| block.trait_name.clone());
                found_functions.push(descriptor_at(
                    source,
                    mask,
                    declaration_start,
                    name,
                    kind,
                    (self_type, trait_name),
                    true,
                ));
            }
        }
        index = attribute_start + 1;
    }
    found_functions
}

/// Every function in the file the host could address, annotated or not.
///
/// This is how a patch finds functions with no annotation in the source. It
/// deliberately matches what each crate's build script treats as addressable:
/// module-level functions, and inherent and non-generic trait methods named
/// through their type. Anything the build script skips - a function nested
/// inside another, or one in a generic `impl` - is skipped here too, because
/// the running artifact would have no entry for it and the lookup would fail.
///
/// The running artifact stays the source of truth: the host asks it for an
/// address by name and refuses the patch when it has none. A disagreement
/// between this scanner and a build script is therefore a clean refusal rather
/// than a wrong patch.
pub fn all_functions(source: &str) -> Vec<HotFunction> {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let blocks = impl_blocks_for_concrete_types(source, &mask);
    let annotated_kinds_by_name: HashMap<String, HotFunctionKind> =
        hot_functions_in(source, &mask, &blocks)
            .into_iter()
            .map(|function| (function.name, function.kind))
            .collect();

    let mut found_functions: Vec<HotFunction> = Vec::new();
    let mut depth: u32 = 0;
    let mut index = 0usize;

    while index < bytes.len() {
        if mask[index] {
            match bytes[index] {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            // Compared on bytes, not on a `&str` slice: `index` moves one
            // byte at a time, so it can sit in the middle of a multi-byte
            // character (a `'…'` literal is code the mask keeps). A `&str`
            // slice at such a position would panic instead of skipping the
            // byte.
            if bytes[index..].starts_with(b"fn ")
                && (index == 0 || !is_identifier_byte(bytes[index - 1]))
            {
                let name_start = index + 3;
                let name_end = source[name_start..]
                    .find(|character: char| !character.is_alphanumeric() && character != '_')
                    .map(|end| name_start + end)
                    .unwrap_or(source.len());
                if name_end > name_start {
                    let name = source[name_start..name_end].to_string();
                    let enclosing_impl_block = blocks
                        .iter()
                        .filter(|block| {
                            block.body_byte_range.0 < index && index < block.body_byte_range.1
                        })
                        .min_by_key(|block| block.body_byte_range.1 - block.body_byte_range.0);
                    let self_type = enclosing_impl_block.map(|block| block.type_name.clone());
                    let trait_name =
                        enclosing_impl_block.and_then(|block| block.trait_name.clone());
                    // Only module-level functions and methods directly inside
                    // an `impl` are addressable.
                    let addressable = depth == 0 || (self_type.is_some() && depth == 1);
                    // Keyed by the whole path, not the bare name: a type may
                    // carry an inherent `draw` and a trait `draw`, and two
                    // traits may each define `draw` for it. Keying by name alone
                    // would silently drop all but the first.
                    let already_found = found_functions.iter().any(|existing| {
                        existing.name == name
                            && existing.self_type == self_type
                            && existing.trait_name == trait_name
                    });
                    if addressable && !already_found {
                        let kind = annotated_kinds_by_name
                            .get(&name)
                            .copied()
                            .unwrap_or(HotFunctionKind::PlainFunction);
                        let is_annotated = annotated_kinds_by_name.contains_key(&name);
                        found_functions.push(descriptor_at(
                            source,
                            &mask,
                            index,
                            name,
                            kind,
                            (self_type, trait_name),
                            is_annotated,
                        ));
                    }
                }
            }
        }
        index += 1;
    }
    found_functions
}

/// Build one descriptor from a declaration byte offset.
///
/// Both scans work out the same per-declaration facts the same way - receiver,
/// signature, cfg gate, `#[inline(always)]` and ABI-entry status - so this is
/// the one place that does it, rather than the same constructor written twice.
///
// Eight arguments read better here than a temporary struct that each scan would
// build only for this function to take apart.
#[allow(clippy::too_many_arguments)]
fn descriptor_at(
    source: &str,
    mask: &[bool],
    declaration_start: usize,
    name: String,
    kind: HotFunctionKind,
    enclosing_type_and_trait: (Option<String>, Option<String>),
    annotated: bool,
) -> HotFunction {
    HotFunction {
        name,
        kind,
        self_type: enclosing_type_and_trait.0,
        trait_name: enclosing_type_and_trait.1,
        takes_receiver: declaration_takes_receiver(source, mask, declaration_start),
        signature: normalized_signature(source, mask, declaration_start),
        cfg_gated: declaration_is_cfg_gated(source, mask, declaration_start),
        inline_always: declaration_has_attribute(
            source,
            mask,
            declaration_start,
            "#[inline(always)]",
        ),
        is_abi_entry_point: declaration_is_abi_entry_point(source, mask, declaration_start),
        is_annotated: annotated,
    }
}

/// Every `impl` block in the file that names one concrete type.
///
/// Both inherent blocks (`impl Spline`) and non-generic trait blocks
/// (`impl Drawable for Spline`) qualify. Each has exactly one receiver type and
/// one address per method, which is what a patch needs.
///
/// Generic blocks (`impl<T> Foo<T>`, `impl Trait for Foo<T>`) are skipped: a
/// generic has one instantiation per set of type arguments, so there is no
/// single address to redirect. A method inside one has no address a patch could
/// name, so the all-functions scan leaves it out too.
///
/// `mask` MUST be `code_mask(source)`, since it is indexed by the same byte
/// offsets.
pub fn impl_blocks_for_concrete_types(source: &str, mask: &[bool]) -> Vec<ImplBlock> {
    let bytes = source.as_bytes();
    let mut blocks = Vec::new();
    let mut index = 0usize;

    while let Some(offset) = source[index..].find("impl") {
        let start = index + offset;
        index = start + 4;
        if !mask[start] {
            continue;
        }
        // Must be the whole word, not the tail of an identifier.
        if start > 0 && is_identifier_byte(bytes[start - 1]) {
            continue;
        }
        if start + 4 < bytes.len() && is_identifier_byte(bytes[start + 4]) {
            continue;
        }
        let Some(open) = next_open_brace(bytes, mask, start + 4) else {
            continue;
        };
        let Some(close) = matching_brace(bytes, mask, open) else {
            continue;
        };
        // The header between `impl` and the opening brace names the type, and
        // for a trait block the trait as well.
        let header = &source[start + 4..open];
        if let Some((trait_name, type_name)) = impl_header_names(header) {
            blocks.push(ImplBlock {
                body_byte_range: (open, close),
                type_name,
                trait_name,
            });
        }
    }
    blocks
}

/// The trait and type an `impl` header names, if it names exactly one of each.
///
/// Returns `(None, Type)` for an inherent block and `(Some(Trait), Type)` for a
/// non-generic trait block. Returns `None` when the header has no single
/// concrete receiver type, which is the case that cannot be patched at all.
fn impl_header_names(header: &str) -> Option<(Option<String>, String)> {
    let header = header.trim();
    // `impl<T>` introduces type parameters, so every method inside has one
    // instantiation per set of type arguments and no single address. A `where`
    // clause means the same thing.
    if header.is_empty() || header.starts_with('<') || header.contains("where") {
        return None;
    }

    // Split a trait block into its two halves. ` for ` cannot appear inside a
    // simple path, so a plain find is sufficient for the headers that reach
    // here - anything with type arguments is rejected by `simple_path` below.
    let (trait_name, type_name) = match header.find(" for ") {
        Some(position) => (
            Some(simple_path(&header[..position])?),
            simple_path(&header[position + 5..])?,
        ),
        None => (None, simple_path(header)?),
    };
    Some((trait_name, type_name))
}

/// One path with no type arguments, or `None`.
///
/// Angle brackets anywhere mean a generic, which has no single address. A header
/// that is not a plain path at all - a reference, a tuple - is refused by the
/// same character test.
fn simple_path(candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    if candidate.is_empty()
        || !candidate
            .chars()
            .all(|character| character.is_alphanumeric() || character == '_' || character == ':')
    {
        return None;
    }
    Some(candidate.to_string())
}

/// The declaration from `fn` up to the body, with whitespace collapsed.
///
/// Must match, byte for byte, what a crate's build script records for the same
/// function - the two are compared to decide whether a patch may overwrite live
/// code. Both start at the `fn` keyword and collapse whitespace, so `fn  f( a :
/// u8 )` and `fn f(a: u8)` agree while `fn f(a: u16)` does not.
///
/// `mask` MUST be `code_mask(source)` for this exact `source`, since the end of
/// the declaration is found through it.
pub fn normalized_signature(source: &str, mask: &[bool], declaration_start: usize) -> String {
    let bytes = source.as_bytes();
    let mut end = declaration_start;
    while end < bytes.len() {
        if mask[end] && (bytes[end] == b'{' || bytes[end] == b';') {
            break;
        }
        end += 1;
    }
    source[declaration_start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether a `#[cfg(…)]` sits in the attribute block above this declaration.
///
/// The attribute block runs back to the end of the previous item, which is a
/// `}` or `;` in code - exactly where attributes and doc comments live.
///
/// `#[test]` counts as a gate: it expands to `#[cfg(test)]`, so a test
/// function is absent from every artifact the host loads. A module that keeps
/// its tests in a file of its own (rather than an inline `mod tests`) puts no
/// `cfg` in front of them, and registering their addresses would not compile.
///
/// `mask` MUST be `code_mask(source)` for this exact `source`, since the end of
/// the previous item is found through it.
pub fn declaration_is_cfg_gated(source: &str, mask: &[bool], declaration_start: usize) -> bool {
    declaration_has_attribute(source, mask, declaration_start, "#[cfg")
        || declaration_has_attribute(source, mask, declaration_start, "#[test]")
}

/// Whether `attribute` appears in the block above this declaration.
///
/// The search starts at the end of the previous item, a `}` or `;` in code, so
/// only the attributes and doc comments between the two items are read.
///
/// `mask` MUST be `code_mask(source)` for this exact `source`, since the end of
/// the previous item is found through it.
pub fn declaration_has_attribute(
    source: &str,
    mask: &[bool],
    declaration_start: usize,
    attribute: &str,
) -> bool {
    let bytes = source.as_bytes();
    let mut start = declaration_start;
    while start > 0 {
        let candidate = start - 1;
        if mask[candidate] && (bytes[candidate] == b'}' || bytes[candidate] == b';') {
            break;
        }
        start = candidate;
    }
    let mut index = start;
    while index + attribute.len() <= declaration_start {
        if mask[index] && source[index..].starts_with(attribute) {
            return true;
        }
        index += 1;
    }
    false
}

/// Whether the declaration beginning at `fn` takes a receiver.
///
/// `mask` MUST be `code_mask(source)` for this exact `source`, since the
/// parameter list is found through it.
pub fn declaration_takes_receiver(source: &str, mask: &[bool], declaration_start: usize) -> bool {
    let bytes = source.as_bytes();
    let mut cursor = declaration_start;
    // Find the opening parenthesis of the parameter list.
    while cursor < bytes.len() {
        if mask[cursor] && bytes[cursor] == b'(' {
            break;
        }
        if mask[cursor] && bytes[cursor] == b'{' {
            return false;
        }
        cursor += 1;
    }
    let rest = &source[cursor.saturating_add(1)..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('&').unwrap_or(rest).trim_start();
    // A lifetime on the receiver, as in `&'a self`.
    let rest = if let Some(after) = rest.strip_prefix('\'') {
        after
            .trim_start_matches(|character: char| character.is_alphanumeric() || character == '_')
            .trim_start()
    } else {
        rest
    };
    let rest = rest.strip_prefix("mut ").unwrap_or(rest).trim_start();
    rest.starts_with("self")
}

/// Whether a byte is an ASCII identifier character: a letter, a digit or `_`.
fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

// =============================================================================
// Body stripping
// =============================================================================

/// Whether a scan continues inside a function it just matched.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SkipStrategy {
    /// Resume after the function's closing brace, so nested declarations are
    /// part of the body rather than matches of their own. Body stripping needs
    /// this: a nested function's body is already inside the body it blanked.
    PastBody,
    /// Resume just after the matched declaration, so nested functions are
    /// matched too - which is what [`find_function`] does, since it searches
    /// the whole file for a name.
    IntoBody,
}

/// Replace the bodies of the named functions with a placeholder.
///
/// Comparing two stripped revisions answers the only question classification
/// needs: did anything OUTSIDE these bodies change? Signatures, types,
/// constants, imports, attributes and every other function all survive the
/// strip, so any edit to them shows up as a difference.
pub fn strip_function_bodies(source: &str, function_names: &HashSet<String>) -> String {
    let mask = code_mask(source);
    let mut stripped = String::with_capacity(source.len());
    let mut cursor = 0usize;

    // One forward pass over the file, blanking each named body as it is
    // reached. Asking `find_function` once per name would turn the scan into
    // O(functions x names) full-file passes, and classification strips both
    // revisions on every save.
    for (_, declaration) in
        scan_named_functions(source, &mask, function_names, SkipStrategy::PastBody)
    {
        // Keep everything up to and including the opening brace, drop the body.
        let body_open = declaration.start + (declaration.text.len() - declaration.body.len() - 2);
        stripped.push_str(&source[cursor..=body_open]);
        stripped.push_str("/*body*/");
        stripped.push('}');
        cursor = declaration.end;
    }

    stripped.push_str(&source[cursor..]);
    stripped
}

/// The body text of each named function that this source declares.
///
/// Classification compares one revision's bodies against another's to decide
/// which function actually changed, and answers every name in one pass rather
/// than scanning the file once per name.
///
/// A name the source does not declare is simply absent from the map, which is
/// the same answer [`find_function`] gives by returning `None`. Like
/// [`find_function`], the FIRST declaration of a name wins, so a nested
/// function shadows a later top-level one.
pub fn function_bodies(source: &str, function_names: &HashSet<String>) -> HashMap<String, String> {
    let mask = code_mask(source);
    let mut bodies = HashMap::new();
    for (name, declaration) in
        scan_named_functions(source, &mask, function_names, SkipStrategy::IntoBody)
    {
        bodies.entry(name).or_insert(declaration.body);
    }
    bodies
}

/// Every declaration of a named function, in source order, in one pass.
///
/// Shared by [`strip_function_bodies`] and [`function_bodies`] so both answer
/// with exactly the bounds [`find_function`] would report for the same name:
/// the same `fn <name>` spelling, the same boundary characters either side, and
/// the same brace matching over `mask`.
///
/// `mask` MUST be `code_mask(source)`, so a `fn` inside a string or a comment
/// is never matched.
fn scan_named_functions(
    source: &str,
    mask: &[bool],
    names: &HashSet<String>,
    skip: SkipStrategy,
) -> Vec<(String, FunctionText)> {
    const KEYWORD: &[u8] = b"fn ";
    let bytes = source.as_bytes();
    let mut found_declarations = Vec::new();
    let mut index = 0usize;

    while index + KEYWORD.len() < bytes.len() {
        // `fn ` in real code, preceded by a boundary - the same test
        // `find_function` applies, so `r#fn ` or `xfn ` never matches.
        let is_keyword = &bytes[index..index + KEYWORD.len()] == KEYWORD
            && mask[index..index + KEYWORD.len()]
                .iter()
                .all(|byte_is_code| *byte_is_code);
        let preceded_ok =
            index == 0 || matches!(bytes[index - 1], b'\n' | b'\r' | b'\t' | b' ' | b'}' | b';');
        if !is_keyword || !preceded_ok {
            index += 1;
            continue;
        }

        // The identifier that follows, which is what decides whether this
        // declaration is one of the ones being asked about.
        let name_start = index + KEYWORD.len();
        let mut name_end = name_start;
        while name_end < bytes.len() && is_identifier_byte(bytes[name_end]) && mask[name_end] {
            name_end += 1;
        }
        let followed_ok = name_end >= bytes.len()
            || matches!(bytes[name_end], b'(' | b' ' | b'<' | b'\n' | b'\r' | b'\t');
        if name_end == name_start || !followed_ok {
            index += 1;
            continue;
        }
        let name = &source[name_start..name_end];
        if !names.contains(name) {
            index = name_end;
            continue;
        }

        // A declaration whose braces do not resolve is skipped, not fatal:
        // `find_function` answers `None` for that one name and its caller goes
        // on asking about the others, so abandoning the whole scan here would
        // silently stop stripping every function further down the file.
        let Some(open) = next_open_brace(bytes, mask, name_end) else {
            index = name_end;
            continue;
        };
        let Some(close) = matching_brace(bytes, mask, open) else {
            index = name_end;
            continue;
        };
        found_declarations.push((
            name.to_string(),
            FunctionText {
                start: index,
                end: close + 1,
                text: source[index..=close].to_string(),
                body: source[open + 1..close].to_string(),
            },
        ));
        index = match skip {
            SkipStrategy::PastBody => close + 1,
            SkipStrategy::IntoBody => name_end,
        };
    }
    found_declarations
}

// =============================================================================
// Build-script entry point
// =============================================================================

/// Whether a declaration carries an attribute that makes it the module ABI's
/// entry point.
///
/// A module is free to call its entry point anything. What makes it one is
/// `#[pill_module]` or `#[pill_project]`, the attribute that generates the
/// `#[no_mangle]` export the host calls at load time.
///
/// `mask` MUST be `code_mask(source)` for this exact `source`, since the
/// attribute block is found through it.
pub fn declaration_is_abi_entry_point(
    source: &str,
    mask: &[bool],
    declaration_start: usize,
) -> bool {
    ABI_ENTRY_POINT_ATTRIBUTES
        .iter()
        .any(|attribute| declaration_has_attribute(source, mask, declaration_start, attribute))
}

/// Absolute path of the file Cargo compiles as this crate's root.
///
/// Defaults to `src/lib.rs`; a `[lib] path = "..."` in the manifest moves it
/// elsewhere. The crate root contributes no module segment while every other
/// file contributes one. Treating a renamed root as a module would prefix every
/// emitted path with a module that does not exist, and the generated inventory
/// would not compile.
///
/// A crate renames its root so the file has a name unique across the dependency
/// graph. A debugger that matches a breakpoint by file name - which it must for
/// a module the host loads at runtime, where the file is not mapped when the
/// breakpoint is set - would otherwise see hundreds of candidates.
///
/// The manifest is scanned by hand rather than parsed. This crate deliberately
/// has no dependencies, because the host links it as well as every build
/// script, and a TOML parser here would be pulled into both.
///
/// Public because the host derives the same module paths when it compiles a
/// patch. A patch registered under a path the inventory does not hold never
/// dispatches.
pub fn crate_root_file(manifest_directory: &Path) -> PathBuf {
    let default_root = manifest_directory.join("src").join("lib.rs");
    let Ok(manifest) = std::fs::read_to_string(manifest_directory.join("Cargo.toml")) else {
        return default_root;
    };
    let mut inside_lib_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        // Section headers switch the scan on and off; a comment or a blank line
        // between them does not, so only `[`-prefixed lines are considered.
        if line.starts_with('[') {
            inside_lib_section = line == "[lib]";
            continue;
        }
        if !inside_lib_section || line.starts_with('#') {
            continue;
        }
        let Some(assignment) = line.strip_prefix("path") else {
            continue;
        };
        let Some(path_value) = assignment.trim_start().strip_prefix('=') else {
            continue;
        };
        // Manifest paths are always `/`-separated; join the components one at a
        // time so the result uses the platform separator and compares equal to
        // the paths `rust_files` produces.
        let path_value = path_value.trim().trim_matches('"');
        if path_value.is_empty() {
            continue;
        }
        let mut root = manifest_directory.to_path_buf();
        for component in path_value.split('/') {
            root.push(component);
        }
        return root;
    }
    default_root
}

/// Generate this crate's function-address inventory into `OUT_DIR`.
///
/// The whole of a participating crate's build script:
///
/// ```no_run
/// pill_hot_scan::generate_function_inventory();
/// ```
///
/// and one line in the crate root:
///
/// ```ignore
/// include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));
/// ```
///
/// Every addressable function a patch can reach - module-level, an inherent
/// method, or a trait method whose trait the crate root can name - is registered
/// with its qualified path, its address, and the declaration it was built with,
/// so the host can redirect any of them with nothing in the source annotated.
/// The host decides what is patchable with the same scanner, and the two must
/// agree byte for byte.
///
/// # Panics
///
/// Panics when run outside a build script, which supplies `CARGO_MANIFEST_DIR`,
/// `OUT_DIR` and `CARGO_PKG_NAME`, and when the generated file cannot be
/// written; that would leave the crate compiling against a stale or absent
/// inventory.
pub fn generate_function_inventory() {
    // Step 1: Read the build script's environment and the crate layout it names.
    let manifest_directory = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("build scripts always have CARGO_MANIFEST_DIR"),
    );
    let source_directory = manifest_directory.join("src");
    let crate_root = crate_root_file(&manifest_directory);
    let output_directory =
        PathBuf::from(std::env::var("OUT_DIR").expect("build scripts always have OUT_DIR"));
    let crate_name = std::env::var("CARGO_PKG_NAME")
        .expect("build scripts always have CARGO_PKG_NAME")
        .replace('-', "_");
    let inventory_path = output_directory.join("function_inventory.rs");

    println!("cargo:rerun-if-changed={}", source_directory.display());

    // Step 2: Emit the inventory only at `opt-level = 0`. The inventory exists
    // so the host can overwrite a function's first bytes, and that is only
    // sound without optimization: LLVM folds functions with identical bodies
    // into one symbol, so patching a merged twin rewrites the survivor into a
    // jump to itself. `/OPT:NOICF` does not prevent it, because the merging is
    // LLVM's, not the linker's.
    //
    // Above opt-level 0 the inventory is therefore empty, so prologue patching
    // is disabled by the file's own content rather than by a promise. The build
    // still succeeds: a release artifact has no reason to carry this.
    let optimization_level = std::env::var("OPT_LEVEL").unwrap_or_else(|_| "0".to_string());
    if optimization_level != "0" {
        write_inventory(
            &inventory_path,
            "// Auto-generated by pill_hot_scan - deliberately empty.\n\
             //\n\
             // No inventory is emitted above opt-level 0: identical code folding\n\
             // merges same-bodied functions, and overwriting a merged function's\n\
             // prologue rewrites the survivor into a jump to itself.\n",
        );
        return;
    }

    // Step 3: Walk the crate's sources and emit one entry per addressable
    // function.
    let mut generated_source = String::from(
        "// Auto-generated by pill_hot_scan - do not edit.\n\
         //\n\
         // One entry per function in this crate, submitted into the artifact-wide\n\
         // inventory so the host can resolve any address by qualified path. No\n\
         // source in this crate is annotated; discovery is entirely mechanical.\n",
    );
    let mut index = 0usize;

    for source_file in rust_files(&source_directory) {
        let Ok(contents) = std::fs::read_to_string(&source_file) else {
            continue;
        };
        println!("cargo:rerun-if-changed={}", source_file.display());

        // Module path of this file, so two functions sharing a bare name in
        // different modules stay distinct keys.
        let relative_path = source_file
            .strip_prefix(&source_directory)
            .unwrap_or(&source_file);
        let mut segments: Vec<String> = relative_path
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/")
            .split('/')
            .map(str::to_string)
            .collect();
        // The crate root names no module of its own, whatever it is called;
        // `mod.rs` is named by the directory holding it. Comparing the whole
        // path rather than the file stem keeps a submodule that happens to
        // share the root's name from being mistaken for it.
        if source_file == crate_root {
            segments.clear();
        } else if segments.last().is_some_and(|last| last == "mod") {
            segments.pop();
        }

        for function in all_functions(&contents) {
            // The module ABI's own entry point, which the host calls through
            // the export table at load time rather than through any redirect.
            // Recognized by the attribute that makes it one, not by its name:
            // the name is a convention a module is free to break, while the
            // attribute is what actually generates the export.
            if function.is_abi_entry_point {
                continue;
            }
            // A `cfg` this scanner cannot evaluate may have removed the function
            // from the very build being scanned; registering an address for an
            // item that does not exist would not compile.
            if function.cfg_gated {
                continue;
            }
            // Inlined at every call site, so there may be no body left to
            // redirect. Registering it would promise a patch that silently
            // reaches nobody, which is worse than saying it is not patchable.
            if function.inline_always {
                println!(
                    "cargo:warning=pill: `{}` is #[inline(always)] and is not \
                     registered for hot patching; callers hold their own copy, \
                     so redirecting it would reach none of them",
                    function.name
                );
                continue;
            }
            // A trait method's `impl` may rely on a `use` that is scoped to its
            // own file, and the inventory is included at the crate root. Naming
            // it there would not compile, so it is skipped with a warning.
            if let Some(trait_name) = &function.trait_name {
                if !trait_is_nameable_from_crate_root(trait_name, &segments, &contents) {
                    println!(
                        "cargo:warning=pill: `{}::{}` is not registered for hot patching: the trait `{trait_name}` is named through a `use` in its own module, which the generated inventory at the crate root cannot see. Write the trait's full path in the `impl` header to make it patchable",
                        function.self_type.as_deref().unwrap_or("?"),
                        function.name
                    );
                    continue;
                }
            }

            let qualified = inventory_name(&crate_name, &segments, &function);
            // The same path as an expression this crate can evaluate. A trait
            // method needs the qualified form: with an inherent and a trait
            // method of the same name, a bare `Type::method` resolves to the
            // inherent one, so the wrong address would be recorded.
            let mut owner_segments = segments.clone();
            let local_path = match (&function.self_type, &function.trait_name) {
                (Some(owner), Some(trait_name)) => {
                    owner_segments.push(owner.clone());
                    format!(
                        "<{} as {trait_name}>::{}",
                        owner_segments.join("::"),
                        function.name
                    )
                }
                (Some(owner), None) => {
                    owner_segments.push(owner.clone());
                    owner_segments.push(function.name.clone());
                    owner_segments.join("::")
                }
                _ => {
                    owner_segments.push(function.name.clone());
                    owner_segments.join("::")
                }
            };
            let signature = function.signature;

            generated_source.push_str(&format!(
                "\n#[doc(hidden)]\nfn __pill_address_{index}() -> usize {{\n    \
                 {local_path} as *const () as usize\n}}\n\
                 ::pill_engine::submit! {{\n    \
                 ::pill_engine::hot_patch::PillFunctionAddress {{\n        \
                 qualified_name: \"{qualified}\",\n        \
                 address: __pill_address_{index},\n        \
                 signature: \"{signature}\",\n    }}\n}}\n"
            ));
            index += 1;
        }
    }

    // Step 4: Write the file the crate's `include!` reads.
    write_inventory(&inventory_path, &generated_source);
}

/// The name one function is registered and looked up under.
///
/// Three shapes, one per kind of function:
///
/// - `crate::module::free_function`
/// - `crate::module::Type::method` for an inherent method
/// - `crate::module::<Type as Trait>::method` for a trait method
///
/// The trait form prevents collisions. A type may carry an inherent `draw` and
/// a trait `draw`, and two traits may each define `draw` for it, so naming a
/// trait method through its type alone would collide with the others.
///
/// This is the single definition both sides use: the build script records
/// addresses under these names, and the host asks for them under these names.
pub fn inventory_name(
    crate_name: &str,
    module_segments: &[String],
    function: &HotFunction,
) -> String {
    let mut prefix = vec![crate_name.to_string()];
    prefix.extend(module_segments.iter().cloned());
    format!("{}::{}", prefix.join("::"), receiver_path(function))
}

/// The trailing part of an inventory name: everything after the module path.
fn receiver_path(function: &HotFunction) -> String {
    match (&function.self_type, &function.trait_name) {
        (Some(owner), Some(trait_name)) => {
            format!("<{owner} as {trait_name}>::{}", function.name)
        }
        (Some(owner), None) => format!("{owner}::{}", function.name),
        _ => function.name.clone(),
    }
}

/// Whether a trait method's `impl` can be named from the crate root, where the
/// generated inventory is included.
///
/// `impl Display for Spline` in a submodule relies on that file's
/// `use std::fmt::Display;`, and a `use` only applies inside its own module.
/// Writing `<sub::Spline as Display>::fmt` in `lib.rs` would therefore not
/// compile. Three cases can be named:
///
/// - the `impl` sits in the crate root file, where the same `use` lines are in
///   scope;
/// - the trait path is already qualified, as in `crate::Drawable` or
///   `pill_engine::Resource`;
/// - the trait is declared in the same file, so its own module path reaches it.
///
/// Anything else is skipped rather than guessed.
fn trait_is_nameable_from_crate_root(
    trait_name: &str,
    module_segments: &[String],
    source: &str,
) -> bool {
    module_segments.is_empty() || trait_name.contains("::") || declares_trait(source, trait_name)
}

/// Whether this file declares a trait by that name.
fn declares_trait(source: &str, trait_name: &str) -> bool {
    let mask = code_mask(source);
    let bytes = source.as_bytes();
    let needle = format!("trait {trait_name}");
    let mut index = 0usize;
    while let Some(offset) = source[index..].find(&needle) {
        let start = index + offset;
        index = start + needle.len();
        if !mask[start] {
            continue;
        }
        // `trait` must be a whole word, and the name must end where it ends -
        // otherwise `trait DrawableExt` matches a search for `Drawable`.
        if start > 0 && is_identifier_byte(bytes[start - 1]) {
            continue;
        }
        let after = start + needle.len();
        if after < bytes.len() && is_identifier_byte(bytes[after]) {
            continue;
        }
        return true;
    }
    false
}

/// Every `.rs` file under `root`, in stable order so the generated file does not
/// churn between builds.
fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Write the generated inventory, failing loudly rather than leaving a stale one.
///
/// # Panics
///
/// Panics when the file cannot be written.
fn write_inventory(path: &Path, contents: &str) {
    std::fs::write(path, contents.as_bytes())
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A `#[system]` declaration is hot without a marker of its own: the engine
    /// gives every system a dispatch slot in a development build, so a body
    /// edit is a patch and not a reload.
    #[test]
    fn a_system_declaration_is_classified_without_a_marker() {
        let source = "\
#[system(name = \"physics\", order = 900)]
fn physics_system(mut query: Query<&mut PhysicsState>) {}

#[system]
fn unnamed(id: Entity) {}

#[system(
    name = \"rendering\",
    run_phase = \"post_project\",
    run_in_edit_mode = true
)]
fn rendering_system() {}
";
        let functions = hot_functions(source);
        let names: Vec<&str> = functions
            .iter()
            .map(|function| function.name.as_str())
            .collect();
        assert_eq!(names, ["physics_system", "unnamed", "rendering_system"]);
        assert!(
            functions
                .iter()
                .all(|function| function.kind == HotFunctionKind::System),
            "a declared system takes the system route"
        );

        let all = all_functions(source);
        assert!(
            all.iter()
                .filter(|function| names.contains(&function.name.as_str()))
                .all(|function| function.is_annotated),
            "a declared system has a dispatch slot, so it counts as annotated"
        );
    }

    /// An attribute whose name only starts with `system` is not a declaration.
    #[test]
    fn an_attribute_named_like_a_system_but_longer_is_ignored() {
        let source = "\
#[systematic]
fn not_a_system() {}
";
        assert!(
            hot_functions(source).is_empty(),
            "only `#[system]` and `#[system(...)]` mark a declaration"
        );
    }

    /// A lifecycle method's `#[order(n)]` marker sits outside every function
    /// body, so stripping the bodies leaves the edit visible and classification
    /// refuses it as a reload. The engine reads the order at registration,
    /// which is what makes a changed value something only a reload can apply.
    #[test]
    fn an_order_marker_survives_body_stripping() {
        let before = "\
#[script]
impl ScriptComponent for Spin {
    #[order(10)]
    fn on_update(&mut self, _script_context: &mut ScriptContext) {
        self.spin();
    }
}
";
        let after = before.replace("#[order(10)]", "#[order(20)]");
        let names: HashSet<String> = std::iter::once(String::from("on_update")).collect();

        assert_ne!(
            strip_function_bodies(before, &names),
            strip_function_bodies(&after, &names),
            "an edited order must show up outside the stripped bodies"
        );
        assert_eq!(
            strip_function_bodies(before, &names),
            strip_function_bodies(before, &names),
            "the same source strips to the same text"
        );
    }

    const SAMPLE: &str = r#"
use pill_engine::*;
use project::PhysicsState;

const GRAVITY: f32 = 800.0;

fn helper(value: f32) -> f32 {
    value * 2.0
}

#[pill_hot]
fn movement_system(mut query: Query<&mut PhysicsState>) -> Result<(), SystemError> {
    // a brace in a comment: {
    let label = "and one in a string: }";
    for mut physics in query.iter_mut() {
        physics.position_x += 1.0;
    }
    Ok(())
}

fn movement_system_extra() {}
"#;

    /// A `#[test]` function is not in any artifact the host loads.
    ///
    /// Tests can live in a file of their own (the editor shell has
    /// `shell/tests.rs`) and carry no `cfg`: the `#[test]` attribute is the
    /// gate. Registering one would make the generated inventory fail to compile
    /// outside `cargo test`.
    #[test]
    fn a_test_function_counts_as_cfg_gated() {
        let source = "\
#[test]
fn a_row_click_moves_the_selection() {}

#[test_case(1)]
fn a_decorated_case(value: u32) {}
";
        let functions = all_functions(source);
        let by_name = |name: &str| {
            functions
                .iter()
                .find(|function| function.name == name)
                .unwrap_or_else(|| panic!("`{name}` must be scanned: {functions:?}"))
        };
        assert!(by_name("a_row_click_moves_the_selection").cfg_gated);
        // A custom attribute that merely starts with `test` is not `#[test]`.
        assert!(!by_name("a_decorated_case").cfg_gated);
    }

    /// The ABI entry point is recognized by the attribute that generates it,
    /// not by being called `register`.
    ///
    /// A module may name its entry point anything. What makes it one is
    /// `#[pill_module]`. Keying off the name would register a function the host
    /// only ever calls through the export table, and would exclude any ordinary
    /// function that happens to be called `register`.
    #[test]
    fn the_abi_entry_point_is_found_by_its_attribute() {
        let source = "\
#[pill_module]
fn set_up(engine: &mut Engine) -> u32 { 0 }

#[pill_engine::pill_project]
fn boot(engine: &mut Engine) -> u32 { 0 }

fn register(value: u32) -> u32 { value }
";
        let functions = all_functions(source);
        let by_name = |name: &str| {
            functions
                .iter()
                .find(|function| function.name == name)
                .unwrap_or_else(|| panic!("`{name}` must be scanned: {functions:?}"))
        };

        // Both attribute spellings mark an entry point, path-qualified included.
        assert!(by_name("set_up").is_abi_entry_point);
        assert!(by_name("boot").is_abi_entry_point);
        // An ordinary function called `register` stays patchable.
        assert!(!by_name("register").is_abi_entry_point);
    }

    /// A non-generic trait `impl` names one concrete type and one trait, so
    /// each of its methods has a single address - which is all a patch needs.
    #[test]
    fn a_non_generic_trait_impl_is_scanned() {
        let source = "\
impl Default for Spline {
    fn default() -> Self { Spline::new() }
}
";
        let functions = all_functions(source);
        assert_eq!(functions.len(), 1);
        assert_eq!(functions[0].name, "default");
        assert_eq!(functions[0].self_type.as_deref(), Some("Spline"));
        assert_eq!(functions[0].trait_name.as_deref(), Some("Default"));
    }

    /// A generic block has no single address, and that is not a naming problem:
    /// there is one instantiation per set of type arguments.
    #[test]
    fn generic_impl_blocks_are_still_refused() {
        for source in [
            "impl<T> Holder<T> {\n    fn get(&self) -> u32 { 1 }\n}\n",
            "impl Draw for Holder<u32> {\n    fn draw(&self) -> u32 { 1 }\n}\n",
            "impl<T> Draw for Holder<T> {\n    fn draw(&self) -> u32 { 1 }\n}\n",
        ] {
            let functions = all_functions(source);
            assert!(
                functions
                    .iter()
                    .all(|function| function.self_type.is_none()),
                "a generic impl must not be attributed to a concrete type: {source}"
            );
        }
    }

    /// A trait method carries its trait because of this exact collision. Without
    /// the trait name, the two methods below would share one name.
    #[test]
    fn an_inherent_and_a_trait_method_of_one_name_stay_distinct() {
        let source = "\
impl Spline {
    fn draw(&self) -> u32 { 1 }
}
impl Renderer for Spline {
    fn draw(&self) -> u32 { 2 }
}
";
        let functions = all_functions(source);
        assert_eq!(functions.len(), 2, "both must be reported: {functions:?}");

        let inherent = &functions[0];
        let via_trait = &functions[1];
        assert_eq!(inherent.trait_name, None);
        assert_eq!(via_trait.trait_name.as_deref(), Some("Renderer"));
        assert_ne!(
            inventory_name("pill_spline", &[], inherent),
            inventory_name("pill_spline", &[], via_trait),
            "the two must not collide in the inventory"
        );
        assert_eq!(
            inventory_name("pill_spline", &[], via_trait),
            "pill_spline::<Spline as Renderer>::draw"
        );
    }

    /// Two types implementing the same trait method: taking the first `fn` of
    /// that name in the file would patch the wrong body.
    #[test]
    fn a_method_is_located_through_its_own_impl_block() {
        let source = "\
impl Default for Alpha {
    fn default() -> Self { Alpha(1) }
}
impl Default for Beta {
    fn default() -> Self { Beta(2) }
}
";
        let alpha =
            find_method(source, "default", "Alpha", Some("Default")).expect("Alpha's method");
        let beta = find_method(source, "default", "Beta", Some("Default")).expect("Beta's method");
        assert!(alpha.text.contains("Alpha(1)"), "got: {}", alpha.text);
        assert!(beta.text.contains("Beta(2)"), "got: {}", beta.text);

        // The bare-name search is what this exists to avoid.
        let first = find_function(source, "default").expect("some method");
        assert!(first.text.contains("Alpha(1)"));
    }

    /// An inherent block is selected by asking for no trait, so the two blocks
    /// on one type do not shadow each other.
    #[test]
    fn find_method_separates_inherent_from_trait() {
        let source = "\
impl Spline {
    fn draw(&self) -> u32 { 11 }
}
impl Renderer for Spline {
    fn draw(&self) -> u32 { 22 }
}
";
        let inherent = find_method(source, "draw", "Spline", None).expect("inherent");
        let via_trait =
            find_method(source, "draw", "Spline", Some("Renderer")).expect("trait method");
        assert!(inherent.text.contains("11"), "got: {}", inherent.text);
        assert!(via_trait.text.contains("22"), "got: {}", via_trait.text);
    }

    /// A trait reached through a `use` in a submodule cannot be named from the
    /// crate root, where the generated inventory is included.
    #[test]
    fn a_trait_is_only_registered_when_the_crate_root_can_name_it() {
        let source = "use std::fmt::Display;\nimpl Display for Spline {}\n";
        // The crate root itself: the same `use` is in scope where the inventory
        // is included.
        assert!(trait_is_nameable_from_crate_root("Display", &[], source));
        // A submodule: the `use` is scoped to that file and does not reach the
        // crate root.
        assert!(!trait_is_nameable_from_crate_root(
            "Display",
            &["spline".to_string()],
            source
        ));
        // Already qualified, so it needs nothing from any module's scope.
        assert!(trait_is_nameable_from_crate_root(
            "pill_engine::Resource",
            &["spline".to_string()],
            source
        ));
        // Declared here, so it can be reached through this file's module path.
        assert!(trait_is_nameable_from_crate_root(
            "Renderer",
            &["spline".to_string()],
            "pub trait Renderer { fn draw(&self); }\n"
        ));
        // A near-miss must not count as a declaration.
        assert!(!trait_is_nameable_from_crate_root(
            "Render",
            &["spline".to_string()],
            "pub trait Renderer { fn draw(&self); }\n"
        ));
    }

    #[test]
    fn finds_a_function_and_its_body() {
        let found = find_function(SAMPLE, "movement_system").expect("found");
        assert!(found.text.starts_with("fn movement_system("));
        assert!(found.text.ends_with('}'));
        assert!(found.body.contains("physics.position_x += 1.0;"));
        // The body must end at the real closing brace, not one inside a string
        // or comment.
        assert!(found.body.contains("Ok(())"));
        assert!(!found.body.contains("fn movement_system_extra"));
    }

    /// A prefix match must not be mistaken for the function being patched.
    #[test]
    fn does_not_match_a_longer_name() {
        let found = find_function(SAMPLE, "movement_system").expect("found");
        let extra = find_function(SAMPLE, "movement_system_extra").expect("found");
        assert_ne!(found.start, extra.start);
    }

    #[test]
    fn reports_absence_for_an_unknown_function() {
        assert!(find_function(SAMPLE, "no_such_function").is_none());
    }

    #[test]
    fn collects_top_level_imports() {
        let imports = top_level_use_statements(SAMPLE);
        assert_eq!(
            imports,
            vec![
                "use pill_engine::*;".to_string(),
                "use project::PhysicsState;".to_string()
            ]
        );
    }

    /// The strip must hide body edits and expose everything else.
    #[test]
    fn stripping_hides_body_edits_only() {
        let names: HashSet<String> = ["movement_system".to_string()].into_iter().collect();

        let edited_body =
            SAMPLE.replace("physics.position_x += 1.0;", "physics.position_x += 9.0;");
        assert_eq!(
            strip_function_bodies(SAMPLE, &names),
            strip_function_bodies(&edited_body, &names),
            "a body-only edit must vanish from the stripped form"
        );

        let edited_constant = SAMPLE.replace("800.0", "900.0");
        assert_ne!(
            strip_function_bodies(SAMPLE, &names),
            strip_function_bodies(&edited_constant, &names),
            "a constant change must survive the strip"
        );

        let edited_signature =
            SAMPLE.replace("fn movement_system(mut query", "fn movement_system(mut q2");
        assert_ne!(
            strip_function_bodies(SAMPLE, &names),
            strip_function_bodies(&edited_signature, &names),
            "a signature change must survive the strip"
        );

        // An edit to a DIFFERENT function's body must also survive, because that
        // function is not in the hot set.
        let edited_helper = SAMPLE.replace("value * 2.0", "value * 3.0");
        assert_ne!(
            strip_function_bodies(SAMPLE, &names),
            strip_function_bodies(&edited_helper, &names),
            "an edit outside the hot set must not be classified as body-only"
        );
    }

    #[test]
    fn finds_hot_functions_by_attribute() {
        assert_eq!(
            hot_function_names(SAMPLE),
            vec!["movement_system".to_string()]
        );
    }

    /// The signature a prologue patch is gated on ignores formatting but not
    /// types, and starts at `fn` so visibility cannot look like a change.
    ///
    /// A crate's build script records the same string with the same rules; if
    /// these two ever disagree the gate would reject every patch, so the shape
    /// of the normalization is pinned here.
    #[test]
    fn signatures_normalize_formatting_but_not_types() {
        let source = "pub fn  mix( &self ,  other : Tint )  -> Tint {
    other
}
";
        let mask = code_mask(source);
        let start = source.find("fn ").expect("declaration");
        assert_eq!(
            normalized_signature(source, &mask, start),
            "fn mix( &self , other : Tint ) -> Tint",
            "whitespace collapses, and the declaration starts at `fn`"
        );

        let widened = source.replace("other : Tint", "other : Tone");
        let mask = code_mask(&widened);
        let start = widened.find("fn ").expect("declaration");
        assert_ne!(
            normalized_signature(&widened, &mask, start),
            "fn mix( &self , other : Tint ) -> Tint",
            "a changed parameter type must change the signature"
        );
    }

    /// A declaration with no body, as a trait method, still ends at the `;`.
    #[test]
    fn a_signature_stops_at_a_semicolon() {
        let source = "fn sample(value: f32) -> f32;
";
        let mask = code_mask(source);
        assert_eq!(
            normalized_signature(source, &mask, 0),
            "fn sample(value: f32) -> f32"
        );
    }

    /// Attributes between `#[pill_hot]` and the declaration must not confuse the
    /// scan, and an unannotated function must not be picked up.
    #[test]
    fn hot_discovery_handles_stacked_attributes() {
        let source = r#"
#[pill_hot]
#[allow(clippy::needless_range_loop)]
#[inline(never)]
fn annotated(value: i32) -> i32 { value }

fn not_annotated() {}

// #[pill_hot] in a comment must be ignored
fn also_not_annotated() {}
"#;
        assert_eq!(hot_function_names(source), vec!["annotated".to_string()]);
    }

    #[test]
    fn code_mask_marks_strings_and_comments() {
        let source = r#"let a = "{"; // }
"#;
        let mask = code_mask(source);
        let bytes = source.as_bytes();
        // The brace inside the string literal must not count as code.
        let string_brace = source.find("\"{\"").unwrap() + 1;
        assert!(!mask[string_brace], "brace in a string must be masked");
        // The brace in the trailing comment likewise.
        let comment_brace = source.rfind('}').unwrap();
        assert!(!mask[comment_brace], "brace in a comment must be masked");
        assert!(mask[0] && bytes[0] == b'l', "plain code stays unmasked");
    }

    // =========================================================================
    // Single-pass scanning
    // =========================================================================

    /// A deliberately naive oracle for the single-pass scanner.
    ///
    /// One independent `find_function` per name, so its correctness is plain
    /// to check and its cost is a full-file scan per name. The tests below
    /// assert the fast scanner agrees with it.
    fn reference_strip(source: &str, names: &HashSet<String>) -> String {
        let mut stripped = String::with_capacity(source.len());
        let mut cursor = 0usize;
        loop {
            let next = names
                .iter()
                .filter_map(|name| find_function(&source[cursor..], name))
                .min_by_key(|found| found.start);
            let Some(found) = next else {
                break;
            };
            let absolute_start = cursor + found.start;
            let body_open = absolute_start + (found.text.len() - found.body.len() - 2);
            stripped.push_str(&source[cursor..=body_open]);
            stripped.push_str("/*body*/");
            stripped.push('}');
            cursor += found.end;
        }
        stripped.push_str(&source[cursor..]);
        stripped
    }

    /// A file that exercises every shape the scanner has to survive.
    ///
    /// The body-less trait declaration is the one that matters: a scan that
    /// gives up when a declaration's braces do not resolve stops stripping
    /// every function BELOW it, and an edit to any of them is then reported as
    /// a change outside a hot body, falling back to a full module reload.
    fn awkward_source() -> &'static str {
        r#"
//! A doc comment mentioning fn disguised in prose.

pub trait Shape {
    /// No body at all, so its braces never resolve.
    fn area(&self) -> f32;
    fn name(&self) -> &str;
}

pub struct Probe;

impl Probe {
    pub const fn new() -> Self {
        Self
    }

    pub fn install(&self, value: usize) {
        let _ = value;
    }
}

impl Default for Probe {
    fn default() -> Self {
        Self::new()
    }
}

fn outer() -> u32 {
    fn inner() -> u32 {
        7
    }
    inner()
}

fn quoted() -> &'static str {
    // fn commented_out() { }
    "fn in_a_string() {"
}
"#
    }

    /// The fast scanner and the per-name oracle strip the same bytes.
    #[test]
    fn stripping_matches_the_per_name_algorithm() {
        let source = awkward_source();
        let names: HashSet<String> = all_functions(source)
            .into_iter()
            .map(|function| function.name)
            .collect();
        assert!(
            names.contains("install") && names.contains("default"),
            "the sample must contain functions below the body-less declaration"
        );
        assert_eq!(
            strip_function_bodies(source, &names),
            reference_strip(source, &names)
        );
    }

    /// A declaration whose braces do not resolve must not stop the scan.
    #[test]
    fn a_body_less_declaration_does_not_end_the_scan() {
        let source = awkward_source();
        let names: HashSet<String> = all_functions(source)
            .into_iter()
            .map(|function| function.name)
            .collect();
        let stripped = strip_function_bodies(source, &names);
        for below in ["install", "default", "outer"] {
            assert!(
                !stripped.contains(&format!("fn {below}(")) || stripped.contains("/*body*/"),
                "{below} must still be reachable"
            );
        }
        assert_eq!(
            stripped.matches("/*body*/").count(),
            reference_strip(source, &names).matches("/*body*/").count(),
            "the same number of bodies must be blanked"
        );
    }

    /// `function_bodies` answers exactly what `find_function` would, per name.
    #[test]
    fn function_bodies_match_find_function() {
        let source = awkward_source();
        let names: HashSet<String> = all_functions(source)
            .into_iter()
            .map(|function| function.name)
            .collect();
        let expected: HashMap<String, String> = names
            .iter()
            .filter_map(|name| find_function(source, name).map(|found| (name.clone(), found.body)))
            .collect();
        assert_eq!(function_bodies(source, &names), expected);
    }

    /// A multi-byte character outside a string must not stop the scan: the
    /// walk steps one byte at a time, so it reaches the continuation bytes of
    /// a `'…'` literal, which the mask keeps as code.
    #[test]
    fn multi_byte_characters_do_not_break_the_scan() {
        let source = "\
/// A name cut with an ellipsis.
pub fn elided() -> bool {
    let ellipsis = '…';
    ellipsis.is_ascii()
}

pub fn after() {}
";
        let names: HashSet<String> = all_functions(source)
            .into_iter()
            .map(|function| function.name)
            .collect();
        assert!(names.contains("elided"), "{names:?}");
        assert!(names.contains("after"), "{names:?}");
    }
}
