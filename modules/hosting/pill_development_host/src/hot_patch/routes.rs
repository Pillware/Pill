//! How a compiled replacement reaches every running copy of a function.
//!
//! # Responsibilities
//!
//! - Find a function's address and signature inside a loaded patch image.
//! - Visit every image that may hold a copy: the loaded artifacts first, then
//!   the patches already in the process.
//! - Install, reset or prologue-patch that function in each of them.
//!
//! # Design
//!
//! A crate linked into several artifacts is compiled into each of them, and a
//! generated patch links its own copy of everything its body calls. A
//! replacement that reached only the crate's own DLL would leave every other
//! copy running the old code. That is why all three routes send the change to
//! both kinds of image, through one shared traversal ([`every_loaded_copy`])
//! instead of each one visiting the images itself.
//!
//! If that traversal skipped the patches, a chain of hot functions would stop
//! updating: a patched caller would keep calling the callee as it was when the
//! caller was compiled.

// External crates
use libloading::{Library, Symbol};
use pill_core::debug;
use pill_core::telemetry::telemetry_target;

// Current crate
use super::{PATCH_PLAIN_EXPORT, PATCH_RESOLVER_EXPORT};
use crate::native_library::NativeLibrary;

// =============================================================================
// Constants
// =============================================================================

/// Export a generated prologue patch carries, through which the host finds the
/// patch's replacement body.
///
/// The patch declares this one export rather than a registry: it replaces
/// exactly one function, so there is nothing to look up by name.
const PATCH_ADDRESS_EXPORT: &[u8] = b"pill_patch_address";

/// Export a generated patch uses to report the address and signature of the
/// function it replaces. An artifact exports a different name for the same
/// question.
const PATCH_ADDRESS_RESOLVER: &[u8] = b"pill_patch_resolve_address";

/// Export through which a generated patch redirects its own copy of a function.
const PATCH_INSTALL_RESOLVER: &[u8] = b"pill_patch_resolve_install";

/// Export through which a generated patch returns its own copy of a function to
/// the body it was compiled with.
const PATCH_RESET_RESOLVER: &[u8] = b"pill_patch_resolve_reset";

// =============================================================================
// Types
// =============================================================================

/// One loaded image that may carry a copy of the function being patched.
///
/// A crate linked into several artifacts is compiled into each of them, and a
/// generated patch links its own copy of whatever its body calls. Both kinds
/// therefore hold code a replacement has to reach. Treating the two the same
/// way here is what lets one traversal ([`every_loaded_copy`]) serve every
/// route.
enum PatchCopy<'a> {
    /// A module or project DLL the host loaded.
    Artifact(&'a NativeLibrary),
    /// A patch image compiled earlier in this session.
    Patch(&'a LoadedPatch),
}

impl PatchCopy<'_> {
    /// Points this copy's redirect slot at `address`.
    ///
    /// A copy's redirect slot is the pointer its compiled stub calls through.
    /// `Ok(false)` means this image has nothing to redirect, which is not a
    /// failure.
    ///
    /// # Errors
    ///
    /// Fails for the same reasons as the wrapped image: it cannot be asked for
    /// its installer, or it refuses the replacement.
    fn install_plain_function(
        &self,
        qualified: &str,
        address: usize,
        signature: &str,
    ) -> Result<bool, String> {
        match self {
            Self::Artifact(artifact) => {
                artifact.install_plain_function(qualified, address, signature)
            }
            Self::Patch(patch) => patch.install_plain_function(qualified, address, signature),
        }
    }

    /// Empties this copy's redirect slot, so it runs its own body again.
    ///
    /// Both arms pass the error text through unchanged. Like an artifact, a
    /// patch reports a missing resolver export as an error, so a copy that
    /// cannot be asked stops the rollback instead of being skipped as a copy
    /// that declares nothing.
    ///
    /// # Errors
    ///
    /// Fails when the image cannot be asked for its reset export, or refuses
    /// for a reason of its own.
    fn reset_plain_function(&self, qualified: &str) -> Result<bool, String> {
        match self {
            Self::Artifact(artifact) => artifact.reset_plain_function(qualified),
            Self::Patch(patch) => patch.reset_plain_function(qualified),
        }
    }

    /// Returns where this copy's own compiled `qualified` lives, and the
    /// declaration it was built with.
    ///
    /// The declaration is `None` when the image cannot report one. The prologue
    /// gate refuses that case: a missing declaration must never pass as
    /// "nothing to check".
    fn function_address(&self, qualified: &str) -> Option<(usize, Option<String>)> {
        match self {
            Self::Artifact(artifact) => artifact.function_address(qualified),
            Self::Patch(patch) => patch.function_address(qualified),
        }
    }

    /// Returns how many of this copy's functions have a known extent (their
    /// length, as the exception directory records it), when it can answer.
    ///
    /// Only an artifact has the diagnostic export; a patch answers `None`, so a
    /// prologue refusal quotes coverage for artifacts alone.
    fn extent_coverage(&self) -> Option<(usize, usize)> {
        match self {
            Self::Artifact(artifact) => artifact.extent_coverage(),
            Self::Patch(_) => None,
        }
    }
}

/// The two operations applied to one copy of the function: install and reset.
///
/// The trait exists so the refusal handling below can be tested. In production
/// both the copy that accepts a replacement and the copy that refuses it are
/// loaded images, and a unit test can build neither. [`PatchCopy`] is the only
/// implementation that ships.
trait RedirectSlot {
    /// Points this copy's redirect slot at `address`.
    ///
    /// # Errors
    ///
    /// Fails when the image cannot be asked, or refuses the replacement.
    fn install(&self, qualified: &str, address: usize, signature: &str) -> Result<bool, String>;

    /// Empties this copy's redirect slot, so it runs its own body again.
    ///
    /// # Errors
    ///
    /// Fails when the image cannot be asked, or refuses the reset.
    fn reset(&self, qualified: &str) -> Result<bool, String>;
}

impl RedirectSlot for PatchCopy<'_> {
    fn install(&self, qualified: &str, address: usize, signature: &str) -> Result<bool, String> {
        self.install_plain_function(qualified, address, signature)
    }

    fn reset(&self, qualified: &str) -> Result<bool, String> {
        self.reset_plain_function(qualified)
    }
}

/// A patch loaded earlier, which a later patch also has to reach.
///
/// A patch links its own copy of everything its body calls, and those copies go
/// stale as soon as a callee is patched. This target is what keeps a chain of
/// hot functions moving: patch a caller of `pill_spline::math::hermite`, then
/// patch `hermite` itself, and without this the caller's patch keeps running
/// the `hermite` it was compiled with.
pub(crate) struct LoadedPatch {
    /// The qualified name of the function this patch replaced. It also labels
    /// the patch in error messages.
    pub function: String,
    /// The generation number under which this patch was recorded.
    pub generation: u32,
    /// The loaded image. It is never unloaded, because a slot or a jump may
    /// point into it for the rest of the process and nothing moves those
    /// addresses.
    pub library: Library,
}

impl LoadedPatch {
    /// Offers a replacement to this patch's own copy of one `#[pill_hot_fn]`.
    ///
    /// A patch links its own copy of whatever its body calls, slots included,
    /// and those slots start empty. If this copy were skipped, a patched caller
    /// would keep calling the version of a callee that is patched afterwards.
    ///
    /// # Errors
    ///
    /// Fails when the patch exports no installer, and when the patch refuses the
    /// replacement because `qualified` changed shape.
    pub(super) fn install_plain_function(
        &self,
        qualified: &str,
        address: usize,
        signature: &str,
    ) -> Result<bool, String> {
        type InstallFn =
            unsafe extern "C" fn(*const std::ffi::c_char, usize, *const std::ffi::c_char) -> u32;
        // SAFETY: the export is generated by `pill_hot_resolver!` with exactly
        // this C ABI signature, so reading it as `InstallFn` is valid.
        //
        // A missing export is an error, not `Ok(false)`. `Ok(false)` means
        // "asked, and this image does not link the crate", and every caller
        // skips those copies. An image that cannot be asked must not be folded
        // into that answer: its copies would keep running the old code while
        // the log reports the install as complete, so the caller gets an error
        // and falls back to a reload.
        let install =
            unsafe { self.library.get::<InstallFn>(PATCH_INSTALL_RESOLVER) }.map_err(|error| {
                format!(
                    "the {} patch exports no {} to install through ({error})",
                    self.function,
                    String::from_utf8_lossy(PATCH_INSTALL_RESOLVER),
                )
            })?;
        let Ok(name) = std::ffi::CString::new(qualified) else {
            return Ok(false);
        };
        let Ok(signature) = std::ffi::CString::new(signature) else {
            return Ok(false);
        };
        // SAFETY: both strings are NUL-terminated and live until the call
        // returns, and `address` points into an image that is never unloaded.
        let status = unsafe { install(name.as_ptr(), address, signature.as_ptr()) };
        // The generated installer returns 0 when it installed the replacement,
        // 1 when this patch does not declare the function, and 2 when the
        // signature no longer matches.
        match status {
            0 => Ok(true),
            1 => Ok(false),
            _ => Err(format!(
                "`{qualified}` changed shape inside the {} patch",
                self.function
            )),
        }
    }

    /// Returns this patch's copy of one `#[pill_hot_fn]` to its own body.
    ///
    /// A missing export is an error for the same reason as in
    /// [`Self::install_plain_function`]: "cannot ask" must not be read as
    /// "asked, and it declares nothing". Otherwise a rollback would leave this
    /// copy running a patched body while reporting that everything was
    /// restored.
    ///
    /// # Errors
    ///
    /// Fails when the patch exports no reset symbol.
    pub(super) fn reset_plain_function(&self, qualified: &str) -> Result<bool, String> {
        type ResetFn = unsafe extern "C" fn(*const std::ffi::c_char) -> u32;
        // SAFETY: the export is generated by `pill_hot_resolver!` with exactly
        // this C ABI signature, so reading it as `ResetFn` is valid.
        let reset =
            unsafe { self.library.get::<ResetFn>(PATCH_RESET_RESOLVER) }.map_err(|error| {
                format!(
                    "the {} patch exports no {} to reset through ({error})",
                    self.function,
                    String::from_utf8_lossy(PATCH_RESET_RESOLVER),
                )
            })?;
        let Ok(name) = std::ffi::CString::new(qualified) else {
            return Ok(false);
        };
        // SAFETY: the name is NUL-terminated and stays alive until the call
        // returns.
        Ok(unsafe { reset(name.as_ptr()) == 0 })
    }

    /// Returns the address and recorded declaration of one function inside this
    /// patch.
    ///
    /// The declaration is `None` when the patch recorded none. That absence
    /// stays visible instead of being flattened into an empty string, which the
    /// prologue gate would read as "nothing to check" - the one reading a
    /// missing declaration must never get.
    fn function_address(&self, qualified: &str) -> Option<(usize, Option<String>)> {
        type AddressFn =
            unsafe extern "C" fn(*const std::ffi::c_char, *mut *const u8, *mut usize) -> usize;
        // SAFETY: the export is generated by `pill_hot_resolver!` with exactly
        // this C ABI signature, so reading it as `AddressFn` is valid.
        let resolve: Symbol<AddressFn> =
            unsafe { self.library.get(PATCH_ADDRESS_RESOLVER) }.ok()?;
        let name = std::ffi::CString::new(qualified).ok()?;
        let mut signature_pointer: *const u8 = std::ptr::null();
        let mut signature_length: usize = 0;
        // SAFETY: `name` is a NUL-terminated string and both out-parameters
        // point at writable slots, as the export requires. A patch is never
        // unloaded, so anything it reports stays valid for the process.
        let address =
            unsafe { resolve(name.as_ptr(), &mut signature_pointer, &mut signature_length) };
        if address == 0 {
            return None;
        }
        let signature = if signature_pointer.is_null() {
            None
        } else {
            // SAFETY: the export wrote a pointer and a length describing a
            // `&'static str` inside the loaded patch, so those bytes are
            // readable for that length.
            let bytes = unsafe { std::slice::from_raw_parts(signature_pointer, signature_length) };
            Some(String::from_utf8_lossy(bytes).into_owned())
        };
        Some((address, signature))
    }
}

/// One artifact's copy of a prologue-patched function, with the bytes it held
/// before the patch.
///
/// The bytes are saved so a rollback can put the function back exactly as the
/// artifact was built; overwriting code in place has no other way back. A later
/// rollback writes to the address, not to the artifact name: a reloaded
/// artifact is a new image, so its old addresses are dropped instead of being
/// used again.
#[derive(Debug, Clone)]
pub(crate) struct PrologueRestore {
    /// The label of the artifact the patched copy lives in, used in error
    /// messages.
    pub artifact: String,
    /// Address of the function whose first bytes were overwritten.
    pub address: usize,
    /// The bytes that were at that address before the patch.
    pub original: Vec<u8>,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Asks a loaded patch for one function's address and signature hash.
///
/// # Errors
///
/// Fails when the patch exports no resolver, when `lookup_name` contains an
/// interior NUL byte, or when the patch does not define that function.
pub(super) fn resolve_in(library: &Library, lookup_name: &str) -> Result<(usize, u64), String> {
    type ResolveFn = unsafe extern "C" fn(*const std::ffi::c_char, *mut u64) -> usize;

    // SAFETY: the export is generated by `pill_hot_resolver!` with exactly this
    // C ABI signature, so reading it as `ResolveFn` is valid.
    let resolve: Symbol<ResolveFn> =
        unsafe { library.get(PATCH_RESOLVER_EXPORT) }.map_err(|_| {
            format!(
                "the patch does not export `{}`",
                String::from_utf8_lossy(PATCH_RESOLVER_EXPORT)
            )
        })?;

    let encoded = std::ffi::CString::new(lookup_name)
        .map_err(|_| "the function name contains a NUL byte".to_string())?;
    let mut signature_hash: u64 = 0;
    // SAFETY: `encoded` is a NUL-terminated string and `signature_hash` is a
    // writable `u64`, as the export requires.
    let address = unsafe { resolve(encoded.as_ptr(), &mut signature_hash) };

    if address == 0 {
        return Err(format!("the patch does not define `{lookup_name}`"));
    }
    Ok((address, signature_hash))
}

/// Asks a loaded patch where its replacement for a plain function lives.
///
/// Returns the address to jump to and the signature text the patch compiled it
/// with, which every receiving artifact checks against its own before accepting
/// the replacement.
///
/// # Errors
///
/// Fails when the patch exports no plain-function resolver, when `lookup_name`
/// contains an interior NUL byte, when the patch defines no such function or
/// reports no signature for it, and when that signature is not UTF-8.
pub(super) fn resolve_plain_in(
    library: &Library,
    lookup_name: &str,
) -> Result<(usize, String), String> {
    type ResolvePlainFn =
        unsafe extern "C" fn(*const std::ffi::c_char, *mut *const u8, *mut usize) -> usize;

    // SAFETY: the export is generated by `pill_hot_resolver!` with exactly this
    // C ABI signature, so reading it as `ResolvePlainFn` is valid.
    let resolve: Symbol<ResolvePlainFn> =
        unsafe { library.get(PATCH_PLAIN_EXPORT) }.map_err(|_| {
            format!(
                "the patch does not export `{}`",
                String::from_utf8_lossy(PATCH_PLAIN_EXPORT)
            )
        })?;

    let encoded = std::ffi::CString::new(lookup_name)
        .map_err(|_| "the function name contains a NUL byte".to_string())?;
    let mut signature_pointer: *const u8 = std::ptr::null();
    let mut signature_length: usize = 0;
    // SAFETY: `encoded` is a NUL-terminated string and both out-parameters point
    // at writable slots, as the export requires. On success the export writes a
    // pointer into the patch's own static storage, which stays mapped because
    // patches are never unloaded.
    let address = unsafe {
        resolve(
            encoded.as_ptr(),
            &mut signature_pointer,
            &mut signature_length,
        )
    };

    if address == 0 {
        return Err(format!("the patch does not define `{lookup_name}`"));
    }
    if signature_pointer.is_null() {
        return Err(format!(
            "the patch reported no signature for `{lookup_name}`"
        ));
    }
    // SAFETY: the export wrote a pointer and a length describing a `&'static
    // str` inside the patch image, which stays loaded, so the bytes are
    // readable for that length.
    let signature = unsafe { std::slice::from_raw_parts(signature_pointer, signature_length) };
    let signature = std::str::from_utf8(signature)
        .map_err(|_| "the patch reported a signature that is not UTF-8".to_string())?;
    Ok((address, signature.to_string()))
}

/// Returns every image that could carry a copy of a function, each with a label
/// for diagnostics.
///
/// Artifacts come first, then patches. A route that visited only one kind would
/// leave the other running the body it was compiled with, so the two kinds are
/// deliberately kept in one iterator.
fn every_loaded_copy<'a>(
    targets: &'a [(&'a str, &'a NativeLibrary)],
    patches: &'a [LoadedPatch],
) -> impl Iterator<Item = (String, PatchCopy<'a>)> + 'a {
    targets
        .iter()
        .map(|(label, artifact)| ((*label).to_string(), PatchCopy::Artifact(artifact)))
        .chain(patches.iter().map(|patch| {
            (
                format!("{} patch gen {}", patch.function, patch.generation),
                PatchCopy::Patch(patch),
            )
        }))
}

/// Reports which copies took the change, or explains that none was reached.
///
/// All three routes end this way. Reaching no copy is an error rather than a
/// silent success, because it means the replacement is not running anywhere.
/// The wording of that error differs per route, so the caller supplies it.
///
/// # Errors
///
/// Fails when `reached_labels` is empty, with the message `nothing_reached`
/// builds.
fn report_reached_copies(
    reached_labels: &[String],
    function: &str,
    nothing_reached: impl FnOnce() -> String,
    message: &'static str,
) -> Result<(), String> {
    if reached_labels.is_empty() {
        return Err(nothing_reached());
    }
    debug!(
        target: telemetry_target::HOT_RELOAD,
        function,
        artifacts = reached_labels.join(", ").as_str(),
        message
    );
    Ok(())
}

/// Offers one replacement to every loaded image, and reports how many took it.
///
/// A crate linked into several artifacts is compiled into each of them, so the
/// project DLL runs its own embedded copy of a module it depends on. Installing
/// only into the module's own DLL would leave that copy running the old code,
/// which is what would otherwise force a cascading project reload.
///
/// `previous` is what those copies are running now. If one copy refuses, the
/// copies already redirected are put back on it. `None` means generation zero,
/// so each copy belongs on its own body.
///
/// # Errors
///
/// Fails when a copy refuses the replacement, and when no copy declares the
/// function at all. The message names the refusing copy and the copies already
/// redirected.
pub(super) fn install_everywhere(
    targets: &[(&str, &NativeLibrary)],
    patches: &[LoadedPatch],
    qualified: &str,
    address: usize,
    signature: &str,
    previous: Option<(usize, &str)>,
) -> Result<usize, String> {
    let copies: Vec<(String, PatchCopy<'_>)> = every_loaded_copy(targets, patches).collect();
    install_across(&copies, qualified, address, signature, previous)
}

/// Offers one replacement to every copy in `copies`, and reports how many took
/// it.
///
/// # Errors
///
/// Fails at the first refusal, after putting the copies already redirected back
/// on what they were running. If that repair fails too, its error is appended
/// to the refusal instead of hiding it.
fn install_across<C: RedirectSlot>(
    copies: &[(String, C)],
    qualified: &str,
    address: usize,
    signature: &str,
    previous: Option<(usize, &str)>,
) -> Result<usize, String> {
    let mut installed_labels = Vec::new();
    let mut redirected_copies = Vec::new();
    for (label, copy) in copies {
        match copy.install(qualified, address, signature) {
            Ok(true) => {
                installed_labels.push(label.clone());
                redirected_copies.push(copy);
            }
            // Nothing to redirect here: this image does not link the crate, or
            // this patch declares nothing.
            Ok(false) => {}
            // A refusal is reported, not tolerated: leaving some artifacts
            // patched and others not would be worse than not patching at all,
            // and the caller falls back to a full reload. The copies that
            // already took the replacement are put back on what they were
            // running, so a half-finished install leaves no artifact on the new
            // body while its siblings run the old one, and no copy silently
            // loses a patch it already had.
            Err(detail) => {
                let unwind_note =
                    match restore_previous_install(&redirected_copies, qualified, previous) {
                        Ok(()) => String::new(),
                        Err(unwind_detail) => {
                            format!("; the partial install could not be unwound: {unwind_detail}")
                        }
                    };
                let reached_text = if installed_labels.is_empty() {
                    String::from("none")
                } else {
                    installed_labels.join(", ")
                };
                return Err(format!(
                    "{label}: {detail}; copies already redirected: {reached_text}{unwind_note}"
                ));
            }
        }
    }
    report_reached_copies(
        &installed_labels,
        qualified,
        || {
            format!(
                "no loaded artifact declares `{qualified}`; it may live in a crate \
                 nothing has loaded yet"
            )
        },
        "plain function redirected in every artifact that links it",
    )?;
    // The count feeds the analytics line's `copies=` field, so the number of
    // copies is measured, not assumed.
    Ok(installed_labels.len())
}

/// Puts the copies a refused install already redirected back on what they were
/// running.
///
/// Generation zero - the body each artifact was built with - is only the right
/// answer when nothing was installed before. A copy that was already running an
/// earlier generation has to be pointed at that generation again; otherwise the
/// refusal undoes a working patch while the session still records it as live.
///
/// # Errors
///
/// Fails when a copy cannot be redirected again: it no longer exports the
/// installer, or its own shape changed.
fn restore_previous_install<C: RedirectSlot>(
    redirected_copies: &[&C],
    qualified: &str,
    previous: Option<(usize, &str)>,
) -> Result<(), String> {
    for copy in redirected_copies {
        match previous {
            Some((address, signature)) => copy.install(qualified, address, signature),
            None => copy.reset(qualified),
        }?;
    }
    Ok(())
}

/// Asks a loaded prologue patch where its replacement body lives.
///
/// # Errors
///
/// Fails when the patch exports no address symbol, or reports a null address.
pub(super) fn resolve_patch_address(library: &Library) -> Result<usize, String> {
    type AddressFn = unsafe extern "C" fn() -> usize;
    // SAFETY: every prologue patch the host generates exports this symbol with
    // exactly this C ABI signature, so reading it as `AddressFn` is valid.
    let resolve: Symbol<AddressFn> =
        unsafe { library.get(PATCH_ADDRESS_EXPORT) }.map_err(|_| {
            format!(
                "the patch does not export `{}`",
                String::from_utf8_lossy(PATCH_ADDRESS_EXPORT)
            )
        })?;
    // SAFETY: the export takes no arguments and returns the address of a
    // function inside the patch image, which is never unloaded.
    let address = unsafe { resolve() };
    if address == 0 {
        return Err("the patch reported a null address".to_string());
    }
    Ok(address)
}

/// The check one reachable copy must pass before its prologue is overwritten.
///
/// A pure function, so the policy can be pinned by tests without a patch image:
/// a copy that cannot report a declaration is refused, not exempted, and a copy
/// whose declaration differs from the edit's is refused with both shapes named.
///
/// # Errors
///
/// Returns the refusal naming the copy, the function and, for a mismatch, both
/// declarations.
fn check_recorded_declaration(
    label: &str,
    qualified: &str,
    recorded_declaration: Option<&str>,
    expected_declaration: &str,
) -> Result<(), String> {
    let Some(recorded_declaration) = recorded_declaration else {
        return Err(format!(
            "{label}: `{qualified}` carries no recorded declaration, so its shape \
             cannot be checked before its prologue is overwritten; rebuild {label} \
             so its build-script inventory records one"
        ));
    };
    if recorded_declaration != expected_declaration {
        return Err(format!(
            "{label}: `{qualified}` was built as `{recorded_declaration}` but the edit \
             declares `{expected_declaration}`; refusing to redirect callers compiled for \
             the old shape"
        ));
    }
    Ok(())
}

/// Redirects every loaded copy of one function by overwriting its prologue.
///
/// This is the macro-free route. Nothing in the running artifact was prepared
/// for the patch: the host asks each artifact for the function's address
/// through its build-script inventory, then writes a jump over the function's
/// first bytes.
///
/// The thread contract is the caller's: run this at a frame boundary, on the
/// thread that runs the frame loop, with no system executing. That is where the
/// fast path is invoked from.
///
/// # Errors
///
/// Fails when a reachable copy's declaration is missing or differs from the
/// edit's, and when writing a prologue fails. Either way nothing is left
/// patched: the first pass writes nothing, and the second undoes what it wrote.
pub(super) fn prologue_patch_everywhere(
    targets: &[(&str, &NativeLibrary)],
    patches: &[LoadedPatch],
    qualified: &str,
    replacement: usize,
    expected_signature: &str,
) -> Result<Vec<PrologueRestore>, String> {
    // Step 1: find every copy and check the declarations before a single
    // prologue is rewritten. A refusal then fires while the process is still
    // untouched. If instead each copy were patched as it was found, the first
    // copy would have to be patched and then unpatched again when a later copy
    // refuses.
    let mut reachable = Vec::new();
    for (label, copy) in every_loaded_copy(targets, patches) {
        let Some((address, signature)) = copy.function_address(qualified) else {
            // Nothing to overwrite here: this image does not link the crate, or
            // this patch declares nothing.
            continue;
        };

        // The gate. Overwriting a function's first bytes does not check what
        // it jumps to, so the declaration has to be checked before the write,
        // against what THIS artifact was built with and not against another
        // copy. A copy that cannot report a declaration is refused rather than
        // exempted: a missing signature that passed silently is exactly how a
        // stale staged artifact would get patched. Classification should
        // already have refused a changed signature; this catches it when it
        // was wrong.
        check_recorded_declaration(&label, qualified, signature.as_deref(), expected_signature)?;
        reachable.push((label, address, copy));
    }

    // Step 2: write the jumps. A failed write is undone, which is the one case
    // the check above cannot rule out.
    let mut restores: Vec<PrologueRestore> = Vec::new();
    for (label, address, copy) in reachable {
        // SAFETY: `address` came from the artifact's own inventory, so it names
        // a real function in a library the host keeps mapped; `replacement`
        // points into a patch image that is never unloaded. No thread is inside
        // the overwritten bytes, because this runs at the frame boundary on the
        // frame-loop thread with no system executing.
        match unsafe { pill_engine::hot_patch::patch_prologue(address, replacement) } {
            Ok(original) => restores.push(PrologueRestore {
                artifact: label,
                address,
                original,
            }),
            Err(detail) => {
                // Report how widespread the cause is, so a refusal says more
                // than "no". Only an artifact has the diagnostic export, so a
                // patch adds no coverage here.
                let coverage = copy
                    .extent_coverage()
                    .map(|(known, total)| {
                        format!(
                            " ({known} of {total} functions in this artifact have a known length)"
                        )
                    })
                    .unwrap_or_default();
                // Undo every write that already happened, so a failure never
                // leaves the process half-patched.
                for restore in restores.iter().rev() {
                    // SAFETY: the same contract as the write above:
                    // `restore.address` is the address this function just
                    // patched, so it still names that function, and
                    // `restore.original` is exactly the bytes read from it.
                    let _ = unsafe {
                        pill_engine::hot_patch::restore_prologue(restore.address, &restore.original)
                    };
                }
                return Err(format!("{label}: {detail}{coverage}"));
            }
        }
    }
    let reached_labels: Vec<String> = restores
        .iter()
        .map(|restore| restore.artifact.clone())
        .collect();
    report_reached_copies(
        &reached_labels,
        qualified,
        || {
            format!(
                "no loaded artifact reports an address for `{qualified}`; its crate may not \
                 generate the function inventory"
            )
        },
        "prologue redirected in every artifact that links it",
    )?;
    Ok(restores)
}

/// Returns one function to its own body in every image that declares it.
///
/// The counterpart of [`install_everywhere`] for a rollback to generation zero.
/// A plain function has one baseline per artifact, so each artifact is asked to
/// empty its own slot instead of being handed an address from another one.
///
/// # Errors
///
/// Fails when a copy cannot be reset, and when no copy declares the function.
pub(super) fn reset_everywhere(
    targets: &[(&str, &NativeLibrary)],
    patches: &[LoadedPatch],
    qualified: &str,
) -> Result<(), String> {
    let mut restored_labels = Vec::new();
    for (label, copy) in every_loaded_copy(targets, patches) {
        match copy.reset_plain_function(qualified) {
            Ok(true) => restored_labels.push(label),
            Ok(false) => {}
            Err(detail) => return Err(format!("{label}: {detail}")),
        }
    }
    report_reached_copies(
        &restored_labels,
        qualified,
        || format!("no loaded artifact declares `{qualified}`"),
        "plain function restored to its original body in every artifact",
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{check_recorded_declaration, install_across, RedirectSlot};

    /// A stand-in for one loaded image's redirect slot.
    ///
    /// The slot holds an address, and the tests use the generation number as
    /// that address, so reading where a copy points also tells what it runs.
    struct FakeCopy {
        generation: Cell<u32>,
        refuses_replacement: bool,
    }

    impl FakeCopy {
        fn new(generation: u32) -> Self {
            Self {
                generation: Cell::new(generation),
                refuses_replacement: false,
            }
        }

        fn refusing() -> Self {
            Self {
                generation: Cell::new(0),
                refuses_replacement: true,
            }
        }
    }

    impl RedirectSlot for FakeCopy {
        fn install(
            &self,
            _qualified: &str,
            address: usize,
            _signature: &str,
        ) -> Result<bool, String> {
            if self.refuses_replacement {
                return Err(String::from("`demo::function` changed shape"));
            }
            self.generation.set(address as u32);
            Ok(true)
        }

        fn reset(&self, _qualified: &str) -> Result<bool, String> {
            self.generation.set(0);
            Ok(true)
        }
    }

    /// An install that fails halfway must leave every copy on the generation it
    /// was running, not on the code its artifact was built with.
    ///
    /// Generation zero is only the right answer when nothing was installed
    /// before: emptying the slot of a copy that was already running generation
    /// one silently reverts that artifact to its compiled-in body while the
    /// session keeps recording generation one as the live one.
    #[test]
    fn a_refused_fan_out_returns_the_copies_it_reached_to_the_generation_they_ran() {
        let copies = vec![
            (String::from("project"), FakeCopy::new(1)),
            (String::from("module"), FakeCopy::refusing()),
        ];

        let error = install_across(&copies, "demo::function", 2, "fn()", Some((1, "fn()")))
            .expect_err("a copy that refuses the replacement fails the fan-out");
        assert!(error.contains("changed shape"), "{error}");
        assert_eq!(
            copies[0].1.generation.get(),
            1,
            "the copy that took the replacement is back on the generation it was running"
        );
        assert_eq!(
            copies[1].1.generation.get(),
            0,
            "the copy that refused was never redirected"
        );
    }

    /// With nothing installed before, a refused install does put the copies it
    /// reached back on the body their artifact was built with.
    #[test]
    fn a_refused_fan_out_with_no_earlier_generation_restores_the_original_bodies() {
        let copies = vec![
            (String::from("project"), FakeCopy::new(0)),
            (String::from("module"), FakeCopy::refusing()),
        ];

        install_across(&copies, "demo::function", 1, "fn()", None)
            .expect_err("a copy that refuses the replacement fails the fan-out");
        assert_eq!(
            copies[0].1.generation.get(),
            0,
            "the copy that took the replacement is back on its own body"
        );
    }

    /// A reachable copy whose declaration is missing is refused, not let
    /// through: the gate is the only check the prologue route can make, so
    /// "no data" has to mean "no write".
    #[test]
    fn the_gate_refuses_a_missing_signature() {
        let refusal =
            check_recorded_declaration("module", "project::movement", None, "fn movement()")
                .expect_err("a copy that cannot say its shape must not be redirected");

        assert!(refusal.contains("module"), "{refusal}");
        assert!(refusal.contains("project::movement"), "{refusal}");
        assert!(
            refusal.contains("no recorded declaration"),
            "the message must name the missing data, not a mismatched shape: {refusal}"
        );
    }

    /// A copy built with a different shape is refused with both shapes named,
    /// and the matching one still passes.
    #[test]
    fn the_gate_refuses_a_changed_signature() {
        let refusal = check_recorded_declaration(
            "module",
            "project::movement",
            Some("fn movement(value: f32)"),
            "fn movement(value: f64)",
        )
        .expect_err("a reshaped function must not be redirected");
        assert!(refusal.contains("fn movement(value: f32)"), "{refusal}");
        assert!(refusal.contains("fn movement(value: f64)"), "{refusal}");

        assert!(
            check_recorded_declaration(
                "module",
                "project::movement",
                Some("fn movement(value: f32)"),
                "fn movement(value: f32)",
            )
            .is_ok(),
            "the declared shape passes the gate"
        );
    }
}
