//! The registries for hot-patching, one copy per DLL: `#[pill_hot]`
//! functions, `#[pill_hot_fn]` slots, and the address inventory a crate's
//! build script writes.
//!
//! # Responsibilities
//!
//! - Re-export the engine core's hot-patch machinery
//!   (`pill_engine_core::hot_reload::hot_patch`) under this path, so one import
//!   gives a caller the core's slots and the registries declared here.
//! - Declare the registries the macros and the build-script inventory submit
//!   into ([`PillHotSlotDescriptor`], [`PillFunctionAddress`],
//!   [`PillHotFunctionDescriptor`]) and the functions that read them.
//! - Hold the two helpers the generated resolver exports share:
//!   [`with_qualified_name`], which decodes the export's C-string argument,
//!   and [`write_signature_out`], which writes a signature through the
//!   export's out-parameters.
//!
//! # Design
//!
//! These registries answer questions about one DLL, such as where a function's
//! address is in this artifact. That is why they live here, in the crate every
//! DLL compiles in, and not in the engine core, of which there is one copy per
//! process. Each DLL's copy of the functions here reads only the lists that
//! DLL submitted.

// External crates
/// The core's hot-patch machinery, which is the same in every DLL: dispatch
/// slots, the engine-owned patch registry, signature hashing, prologue
/// patching, and the errors all of them report.
pub use pill_engine_core::hot_reload::hot_patch::*;

// =============================================================================
// Registry of `#[pill_hot_fn]` slots
// =============================================================================

/// Describes one `#[pill_hot_fn]` this artifact declares.
///
/// The attribute macro submits one per annotated function, in a build with
/// debug assertions on, which is the only build that has the dispatcher the
/// slot feeds.
pub struct PillHotSlotDescriptor {
    /// Fully-qualified path, as `module_path!() + "::" + fn name`.
    pub qualified_name: &'static str,
    /// The slot the generated dispatcher reads, so a host can redirect calls.
    pub slot: &'static PlainSlot,
    /// The function's signature as written; the install gate compares it.
    ///
    /// Text rather than a `TypeId`, because a patch is compiled separately:
    /// both builds derive the value from the same source text, and text is the
    /// form the two can agree on.
    pub signature: &'static str,
    /// Address of the function's real body, when this artifact can name one.
    ///
    /// A function pointer rather than a `usize`, because casting a function to
    /// an integer is not permitted in the constant context that builds this
    /// struct.
    ///
    /// This is the body, not the dispatcher. A patch hands this address to
    /// every artifact that holds a copy of the function, and the dispatcher is
    /// already reached through `slot`, so naming it here would add a hop that
    /// no patch ever installs.
    ///
    /// `None` for an inherent method declared in a running artifact: a
    /// method's body cannot be moved into a function of its own, because an
    /// item declared inside a method body may not name `Self`
    /// (`error[E0401]`), and the moved body would have to name the receiver
    /// type. The body therefore stays inline in the dispatcher and has no
    /// symbol to point at. Only a patch needs an address, and a patch names
    /// the receiver type concretely, so it always supplies one.
    pub implementation_address: Option<fn() -> usize>,
}

inventory::collect!(PillHotSlotDescriptor);

impl PillHotSlotDescriptor {
    /// The descriptor of the `#[pill_hot_fn]` this artifact declares under
    /// `qualified_name`.
    pub fn find(qualified_name: &str) -> Option<&'static Self> {
        inventory::iter::<PillHotSlotDescriptor>
            .into_iter()
            .find(|descriptor| descriptor.qualified_name == qualified_name)
    }
}

/// Redirects every call to one `#[pill_hot_fn]` this artifact declares.
///
/// `address` is the replacement to call, and `signature` is the replacement's
/// signature text, which must match the text recorded at build time. A
/// replacement is called through the slot until [`reset_plain_function`]
/// empties it.
///
/// # Errors
///
/// Returns [`HotPatchError::UnknownSystem`] if this artifact declares no such
/// function, [`HotPatchError::NullAddress`] if `address` is zero, and
/// [`HotPatchError::SignatureMismatch`] if `signature` differs from the
/// recorded text. The signature check is what keeps a reshaped function from
/// being installed behind call sites compiled for the old shape.
pub fn install_plain_function(
    qualified_name: &str,
    address: usize,
    signature: &str,
) -> Result<(), HotPatchError> {
    let descriptor = PillHotSlotDescriptor::find(qualified_name).ok_or_else(|| {
        HotPatchError::UnknownSystem {
            name: qualified_name.to_string(),
        }
    })?;

    if address == 0 {
        return Err(HotPatchError::NullAddress {
            name: qualified_name.to_string(),
        });
    }
    if descriptor.signature != signature {
        return Err(HotPatchError::SignatureMismatch {
            name: qualified_name.to_string(),
            expected: text_hash(descriptor.signature),
            found: text_hash(signature),
        });
    }
    descriptor.slot.install(address);
    Ok(())
}

/// Returns one `#[pill_hot_fn]` to the body compiled into this artifact.
///
/// This empties the slot, so calls go back to this artifact's own body. Every
/// artifact holding a copy of the function has its own slot and must be reset
/// separately.
///
/// # Errors
///
/// Returns [`HotPatchError::UnknownSystem`] if this artifact declares no such
/// function.
pub fn reset_plain_function(qualified_name: &str) -> Result<(), HotPatchError> {
    let descriptor = PillHotSlotDescriptor::find(qualified_name).ok_or_else(|| {
        HotPatchError::UnknownSystem {
            name: qualified_name.to_string(),
        }
    })?;
    descriptor.slot.reset();
    Ok(())
}

/// Returns the name of every `#[pill_hot_fn]` this artifact declares.
///
/// These come from the `PillHotSlotDescriptor` registry. The `#[pill_hot]`
/// systems sit in a different registry and are listed by
/// [`hot_function_names`].
pub fn plain_function_names() -> impl Iterator<Item = &'static str> {
    inventory::iter::<PillHotSlotDescriptor>
        .into_iter()
        .map(|descriptor| descriptor.qualified_name)
}

/// Returns the signature text this artifact recorded for a `#[pill_hot_fn]`.
///
/// Pass this text back to [`install_plain_function`] instead of writing it out
/// again. The exact spelling comes from `stringify!` inside the macro, so a
/// hand-written copy fails the install gate over a single stray space. A patch
/// derives its own copy from the same source through the same macro, which is
/// what makes the two texts comparable.
pub fn plain_function_signature(qualified_name: &str) -> Option<&'static str> {
    PillHotSlotDescriptor::find(qualified_name).map(|descriptor| descriptor.signature)
}

/// Returns the address and signature of a `#[pill_hot_fn]` this artifact
/// declares.
///
/// A host needs both to point another artifact's copy of the same function at
/// this one: the address to jump to, and the signature text that copy compares
/// against its own before accepting it.
pub fn plain_function_entry(qualified_name: &str) -> Option<(usize, &'static str)> {
    let descriptor = PillHotSlotDescriptor::find(qualified_name)?;
    // A `None` address means this artifact declares the function but cannot
    // point at its body, which is true of every inherent method outside a
    // patch. Reporting nothing is correct here: the caller is a patch asking
    // where its own replacement lives.
    let address = descriptor.implementation_address?;
    Some((address(), descriptor.signature))
}

/// Hash of a signature string, used only to report a mismatch.
fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

// =============================================================================
// Macro-free function inventory
// =============================================================================

/// One function this artifact can report the address of, contributed by a
/// crate's build script rather than by an attribute.
///
/// This is the route that needs no annotation: a build script scans the
/// crate's sources, finds every function it can address, and emits one entry
/// per function. Because `inventory` collects per artifact, a DLL ends up with
/// an entry for every addressable function of every crate linked into it,
/// which is the fan-out a patch needs across artifacts.
pub struct PillFunctionAddress {
    /// The function's fully-qualified path: the crate, the module path, then
    /// the function's name - or `Type::method`, or `<Type as Trait>::method`
    /// for a method.
    pub qualified_name: &'static str,
    /// The function's address in THIS artifact.
    ///
    /// A function pointer rather than a `usize` because casting a function to
    /// an integer is not permitted in the constant context that builds this.
    pub address: fn() -> usize,
    /// The declaration as written, from `fn` up to the body, with whitespace
    /// collapsed.
    ///
    /// This is the compatibility gate for the prologue route, the only one that
    /// route has: overwriting a function's first bytes cannot check anything
    /// about the replacement, so the check has to happen before the write. The
    /// host compares this text with the one it read from the edited source and
    /// refuses the patch when they differ.
    pub signature: &'static str,
}

inventory::collect!(PillFunctionAddress);

impl PillFunctionAddress {
    /// The build-script entry this artifact holds for `qualified_name`.
    pub fn find(qualified_name: &str) -> Option<&'static Self> {
        inventory::iter::<PillFunctionAddress>
            .into_iter()
            .find(|entry| entry.qualified_name == qualified_name)
    }
}

/// Returns the address of one build-script-scanned function in this artifact.
///
/// These come from the `PillFunctionAddress` registry. For a `#[pill_hot_fn]`
/// slot use [`plain_function_entry`], and for a `#[pill_hot]` system use
/// [`resolve_hot_function`].
pub fn scanned_function_address(qualified_name: &str) -> Option<usize> {
    PillFunctionAddress::find(qualified_name).map(|entry| (entry.address)())
}

/// Returns the declaration the build-script scan recorded for one function.
///
/// It comes from the `PillFunctionAddress` registry.
pub fn scanned_function_signature(qualified_name: &str) -> Option<&'static str> {
    PillFunctionAddress::find(qualified_name).map(|entry| entry.signature)
}

/// Lists every function this artifact can report the address of, for
/// diagnostics.
pub fn function_addresses() -> impl Iterator<Item = (&'static str, usize)> {
    inventory::iter::<PillFunctionAddress>
        .into_iter()
        .map(|entry| (entry.qualified_name, (entry.address)()))
}

/// Returns how many of this artifact's registered functions have a known
/// extent, with the total, as `(known, total)`.
///
/// This is diagnostic. A prologue patch can only overwrite a function whose
/// length the exception directory records, so this pair says how much of an
/// artifact that route can reach at all. Off Windows x86-64 nothing records a
/// length, so `known` is always zero there.
pub fn functions_with_known_extent() -> (usize, usize) {
    let mut known = 0usize;
    let mut total = 0usize;
    for (_, address) in function_addresses() {
        total += 1;
        if function_extent(address).is_some() {
            known += 1;
        }
    }
    (known, total)
}

// =============================================================================
// Compile-time registry of hot-patchable functions
// =============================================================================

/// Describes one function declared with `#[pill_hot]`.
///
/// The attribute macro submits one into this artifact's registry. Every field
/// can be written in a `const`, so the descriptor can live in a `static`, the
/// same way [`PillComponentDescriptor`](crate::component_registry::PillComponentDescriptor)
/// can.
///
/// The registry is **per linked artifact**: the host executable, each
/// extension DLL and the project DLL each carry exactly the descriptors their
/// own sources declared. That is what keeps it correct across a reload: a
/// generation that stops declaring a function stops submitting its descriptor,
/// and an unloaded DLL takes its descriptors with it.
pub struct PillHotFunctionDescriptor {
    /// Fully-qualified path, as `module_path!() + "::" + fn name`.
    pub qualified_name: &'static str,
    /// Resolves this function's dispatch address and the hash that identifies
    /// its signature.
    ///
    /// A function rather than two constants because both values come from the
    /// generic machinery in [`local_implementation_address`] and
    /// [`signature_hash_of`], which cannot run in a `const`.
    pub resolve: fn() -> (usize, u64),
}

inventory::collect!(PillHotFunctionDescriptor);

impl PillHotFunctionDescriptor {
    /// The descriptor of the `#[pill_hot]` system this artifact declares under
    /// `qualified_name`.
    pub fn find(qualified_name: &str) -> Option<&'static Self> {
        inventory::iter::<PillHotFunctionDescriptor>
            .into_iter()
            .find(|descriptor| descriptor.qualified_name == qualified_name)
    }
}

/// Returns the dispatch address and signature hash of one `#[pill_hot]`
/// function this artifact declares.
///
/// Returns `None` when no `#[pill_hot]` function carries that qualified name.
/// A host reaches the same answer through the artifact's exported resolver,
/// not through this function directly.
pub fn resolve_hot_function(qualified_name: &str) -> Option<(usize, u64)> {
    PillHotFunctionDescriptor::find(qualified_name).map(|descriptor| (descriptor.resolve)())
}

/// Returns the name of every `#[pill_hot]` system this artifact declares, for
/// diagnostics.
///
/// These come from the `PillHotFunctionDescriptor` registry. The
/// `#[pill_hot_fn]` slots sit in a different registry and are listed by
/// [`plain_function_names`].
pub fn hot_function_names() -> impl Iterator<Item = &'static str> {
    inventory::iter::<PillHotFunctionDescriptor>
        .into_iter()
        .map(|descriptor| descriptor.qualified_name)
}

// =============================================================================
// Resolver export helpers
// =============================================================================

/// Calls `run_with_name` with the qualified name a host passed across the C
/// ABI, or returns `refused` when there is no usable name.
///
/// "No usable name" means a null pointer or bytes that are not UTF-8. Every
/// export that finds something by name starts this way, so the check lives here
/// once instead of in each expansion. Each export has its own refusal value -
/// `0` for "no such function", `1` for "refused" - which is why the caller
/// passes it in.
///
/// # Safety
///
/// `qualified_name` must be null, or point at a NUL-terminated C string that
/// stays readable for the whole call.
pub unsafe fn with_qualified_name<R>(
    qualified_name: *const std::ffi::c_char,
    refused: R,
    run_with_name: impl FnOnce(&str) -> R,
) -> R {
    if qualified_name.is_null() {
        return refused;
    }
    // SAFETY: the caller guarantees a NUL-terminated string readable for the
    // duration of this call.
    let name = unsafe { std::ffi::CStr::from_ptr(qualified_name) };
    match name.to_str() {
        Ok(name) => run_with_name(name),
        Err(_) => refused,
    }
}

/// Writes a signature's address and length through the two out-parameters the
/// caller passes, and writes nothing when either is null.
///
/// The signature text is embedded at compile time and ends with no NUL byte.
/// That is why it is reported as a pointer and a byte count, not a C string.
///
/// # Safety
///
/// Both pointers must be null or address writable slots, and `signature` must
/// stay readable for as long as the caller uses what it wrote there.
pub unsafe fn write_signature_out(
    signature: &'static [u8],
    out_signature: *mut *const u8,
    out_signature_length: *mut usize,
) {
    if out_signature.is_null() || out_signature_length.is_null() {
        return;
    }
    // SAFETY: both pointers were checked non-null and the caller guarantees
    // they address writable slots.
    unsafe {
        *out_signature = signature.as_ptr();
        *out_signature_length = signature.len();
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Registry
    // -------------------------------------------------------------------------
    //
    // `#[pill_hot]` is exercised downstream, where a project or module crate
    // expands it. These tests submit a descriptor by hand instead, exactly as
    // the macro would, which is also how the `component_registry` tests cover
    // their registry.

    /// Stands in for a `#[pill_hot]` system.
    fn registry_probe_system() {}

    fn registry_probe_descriptor() -> (usize, u64) {
        (
            local_implementation_address(&registry_probe_system),
            signature_hash_of(&registry_probe_system),
        )
    }

    inventory::submit! {
        PillHotFunctionDescriptor {
            qualified_name: "pill_engine::hot_patch::tests::registry_probe_system",
            resolve: registry_probe_descriptor,
        }
    }

    #[test]
    fn registry_resolves_a_submitted_function() {
        let (address, hash) =
            resolve_hot_function("pill_engine::hot_patch::tests::registry_probe_system")
                .expect("the submitted descriptor must be discoverable");

        assert_ne!(address, 0, "a resolved address must be callable");
        assert_eq!(
            address,
            local_implementation_address(&registry_probe_system),
            "the registry must report the same dispatch address the engine registers"
        );
        assert_eq!(hash, signature_hash_of(&registry_probe_system));
    }

    #[test]
    fn registry_reports_nothing_for_an_unknown_name() {
        assert!(resolve_hot_function("nothing::declares::this").is_none());
    }

    #[test]
    fn registry_lists_its_functions() {
        assert!(
            hot_function_names().any(|name| name.ends_with("registry_probe_system")),
            "a submitted function must appear in the listing"
        );
    }
}
