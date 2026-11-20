//! The native calls the managed side of the mirror ABI uses in addition to the
//! generated trampolines.
//!
//! # Responsibilities
//!
//! - Take the text out of the two per-thread channels `pill_engine::mirror`
//!   keeps (the last error and the returned string) and give managed code a
//!   pointer to it.
//! - Give a mirrored call the address of a Rust-owned resource
//!   (`ResMut<AssetManager>`), under the running invocation's declared access.
//! - Read a file through the engine's asset store for `AssetLoader.Load()`.
//! - Build the mirror-method table of a binary that links its modules
//!   statically, from its own inventory, for the shipping posture.
//!
//! # Design
//!
//! Nothing here names a renderer or audio type. Each extension's C# surface is
//! generated from its own mirror attributes and reaches its own trampolines by
//! address. This file holds only engine-wide pieces: the world's resources, the
//! asset store, and the channels every trampoline writes to.
//!
//! Text and bytes handed out here stay valid on this thread until the next
//! call of the same kind hands new ones out. This file keeps them in a
//! thread-local (one value per thread), and managed code copies them before it
//! does anything else.

// Standard library
use std::cell::RefCell;
use std::ffi::c_void;

// External crates
use pill_engine::component_registry::mirror_method_descriptors;
use pill_engine::{AssetLoader, ResourceId};

// Current crate
use super::components::StableComponentId;
use super::context::{
    guard_managed_callback, native_resource_access_is_authorized, with_active_world,
};
use super::ResolvedMirrorMethod;

// =============================================================================
// Status Codes
// =============================================================================

/// The call succeeded.
const STATUS_OK: u8 = 0;
/// The call asked for something that does not exist: no text in the channel,
/// or no value under the resource's name in the world.
const STATUS_MISSING: u8 = 1;
/// The running system did not declare this access.
const STATUS_ACCESS_NOT_DECLARED: u8 = 2;
/// No managed invocation is running on this thread.
const STATUS_NO_ACTIVE_SCOPE: u8 = 3;
/// A panic was caught inside the call.
const STATUS_PANICKED: u8 = 4;
/// One of the output locations the call writes was null.
const STATUS_NULL_OUTPUT: u8 = 5;

/// `ffi_take_mirror_text` kind for the last error.
const TEXT_LAST_ERROR: u8 = 0;
/// `ffi_take_mirror_text` kind for the returned string.
const TEXT_RETURN_STRING: u8 = 1;

thread_local! {
    /// The text the last `ffi_take_mirror_text` on this thread handed out.
    static HELD_MIRROR_TEXT: RefCell<String> = const { RefCell::new(String::new()) };
    /// The bytes the last `ffi_asset_loader_read` on this thread handed out.
    static HELD_ASSET_BYTES: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

// =============================================================================
// Text Channels
// =============================================================================

/// Takes the last error or the returned string of the mirrored call that just
/// ran on this thread.
///
/// `text_kind` is `TEXT_LAST_ERROR` (`0`) for the last error or
/// `TEXT_RETURN_STRING` (`1`) for the returned string; any other value takes
/// nothing. On success the text stays valid on this thread until the next call
/// that hands text out, and both outputs point at it.
///
/// # Safety
///
/// Both outputs must be null or point at a writable value of their type, which
/// the caller keeps for the duration of the call.
///
/// # Errors
///
/// Returns `STATUS_NULL_OUTPUT` (`5`) when either output is null,
/// `STATUS_MISSING` (`1`) when the requested channel holds no text, and
/// `STATUS_PANICKED` (`4`) when the call panicked. It returns `STATUS_OK`
/// (`0`) after writing both outputs.
pub(super) extern "C" fn ffi_take_mirror_text(
    text_kind: u8,
    out_data: *mut *const u8,
    out_length: *mut u32,
) -> u8 {
    // Step 1: refuse a call with nowhere to write the answer.
    if out_data.is_null() || out_length.is_null() {
        return STATUS_NULL_OUTPUT;
    }
    guard_managed_callback("take_mirror_text", STATUS_PANICKED, || {
        // Step 2: take the requested text out of its channel.
        let text = match text_kind {
            TEXT_LAST_ERROR => pill_engine::mirror::take_last_error(),
            TEXT_RETURN_STRING => pill_engine::mirror::take_return_string(),
            _ => None,
        };
        let Some(text) = text else {
            return STATUS_MISSING;
        };
        // Step 3: keep the text on this thread and write out its address.
        HELD_MIRROR_TEXT.with(|held| {
            let mut held = held.borrow_mut();
            *held = text;
            // SAFETY: both outputs were checked non-null above, and the
            // managed caller passes writable locals.
            unsafe {
                *out_data = held.as_ptr();
                *out_length = u32::try_from(held.len()).unwrap_or(u32::MAX);
            }
        });
        STATUS_OK
    })
}

// =============================================================================
// Native Resources
// =============================================================================

/// Writes the address of the Rust-owned resource registered under the shared
/// name whose 128-bit hash is `stable_id_low` and `stable_id_high`.
///
/// `mode` is `0` for a shared borrow (`&T`) and `1` for a mutable one
/// (`&mut T`). A startup holds the world exclusively, so it is always
/// authorized.
///
/// # Safety
///
/// `output` must be null or point at a writable pointer the caller keeps for
/// the duration of the call.
///
/// # Errors
///
/// Returns `STATUS_NULL_OUTPUT` (`5`) when the output is null,
/// `STATUS_NO_ACTIVE_SCOPE` (`3`) when no managed invocation is running,
/// `STATUS_ACCESS_NOT_DECLARED` (`2`) when the running system did not declare
/// this access, `STATUS_MISSING` (`1`) when the world holds no Rust-owned value
/// under that name, and `STATUS_PANICKED` (`4`) when the call panicked. It
/// returns `STATUS_OK` (`0`) after writing the address.
pub(super) extern "C" fn ffi_get_native_resource(
    stable_id_low: u64,
    stable_id_high: u64,
    mode: u8,
    output: *mut *mut c_void,
) -> u8 {
    // Step 1: refuse a call with nowhere to write the address.
    if output.is_null() {
        return STATUS_NULL_OUTPUT;
    }
    guard_managed_callback("get_native_resource", STATUS_PANICKED, || {
        let key = StableComponentId::from_halves(stable_id_low, stable_id_high);
        // Step 2: check that the running system declared this access.
        match native_resource_access_is_authorized(key, mode) {
            None => return STATUS_NO_ACTIVE_SCOPE,
            Some(false) => return STATUS_ACCESS_NOT_DECLARED,
            Some(true) => {}
        }
        // Step 3: find the value in the active world and write its address.
        let pointer = with_active_world(|world| {
            world.native_resource_pointer(ResourceId::Shared(key.0), mode == 1)
        });
        match pointer {
            None => STATUS_NO_ACTIVE_SCOPE,
            Some(None) => STATUS_MISSING,
            Some(Some(pointer)) => {
                // SAFETY: checked non-null above; the managed caller passes a
                // writable local.
                unsafe { *output = pointer.cast() };
                STATUS_OK
            }
        }
    })
}

// =============================================================================
// Asset Store
// =============================================================================

/// Reads the file at `path` for `AssetLoader.Load()`.
///
/// A relative path is looked up in the mounted packs first, then below the
/// project's asset directory and the module roots; an absolute path is read
/// from the filesystem as it is. A shipped build reads only the packs, the
/// mounted directories and the module roots. On success both outputs point at
/// the bytes, which stay valid on this thread until the next call that hands
/// bytes out.
///
/// # Safety
///
/// `path` must be null or point at `path_length` readable bytes. Both outputs
/// must be null or point at a writable value of their type, which the caller
/// keeps for the duration of the call.
///
/// # Errors
///
/// Returns `STATUS_NULL_OUTPUT` (`5`) when either output is null,
/// `STATUS_MISSING` (`1`) when the read failed, with its message in the
/// last-error channel, and `STATUS_PANICKED` (`4`) when the call panicked. It
/// returns `STATUS_OK` (`0`) after writing both outputs.
pub(super) extern "C" fn ffi_asset_loader_read(
    path: *const u8,
    path_length: u32,
    out_data: *mut *const u8,
    out_length: *mut u32,
) -> u8 {
    // Step 1: refuse a call with nowhere to write the bytes.
    if out_data.is_null() || out_length.is_null() {
        return STATUS_NULL_OUTPUT;
    }
    guard_managed_callback("asset_loader_read", STATUS_PANICKED, || {
        // Step 2: copy the path the managed side passed.
        let path = if path_length == 0 || path.is_null() {
            String::new()
        } else {
            // SAFETY: the managed caller pins `path_length` bytes for the call.
            let bytes = unsafe { std::slice::from_raw_parts(path, path_length as usize) };
            String::from_utf8_lossy(bytes).into_owned()
        };
        // Step 3: load the file through the asset store.
        match AssetLoader::Path(path.into()).load() {
            Ok(bytes) => {
                HELD_ASSET_BYTES.with(|held| {
                    let mut held = held.borrow_mut();
                    *held = bytes;
                    // SAFETY: both outputs were checked non-null above.
                    unsafe {
                        *out_data = held.as_ptr();
                        *out_length = u32::try_from(held.len()).unwrap_or(u32::MAX);
                    }
                });
                STATUS_OK
            }
            Err(error) => {
                pill_engine::mirror::set_last_error(error.to_string());
                STATUS_MISSING
            }
        }
    })
}

// =============================================================================
// Static Mirror Table
// =============================================================================

/// Builds the mirror-method table of this binary from its own inventory.
///
/// A shipping build links every extension into one image. The method
/// descriptors those extensions registered therefore belong to this artifact
/// and carry their trampolines' addresses, so no module has to be loaded to
/// read them. The development host instead builds the same rows from each
/// loaded module's copy.
pub(super) fn statically_linked_mirror_methods() -> Vec<ResolvedMirrorMethod> {
    mirror_method_descriptors()
        .into_iter()
        .map(|descriptor| ResolvedMirrorMethod {
            type_name: descriptor.type_name.to_string(),
            method_name: descriptor.name.to_string(),
            return_tag: descriptor.return_tag.to_string(),
            arg_tags: descriptor
                .arg_tags
                .iter()
                .map(|tag| tag.to_string())
                .collect(),
            arg_names: descriptor
                .arg_names
                .iter()
                .map(|name| name.to_string())
                .collect(),
            address: descriptor.address.0 as usize,
            is_free_function: descriptor.is_free_function,
            crate_name: descriptor.crate_name.to_string(),
            receiver: descriptor.receiver.to_string(),
            owner_kind: descriptor.owner_kind.to_string(),
        })
        .collect()
}

/// Reports whether managed code can call a resolved row.
///
/// An enum's or a resource's `__type` row only describes the type, so it has
/// no trampoline and its address is zero. An object's `__type` row is its drop
/// function and does have one. Only rows with an address go into the table
/// managed code calls through.
pub(super) fn is_callable_by_managed_code(row: &ResolvedMirrorMethod) -> bool {
    row.address != 0
}
