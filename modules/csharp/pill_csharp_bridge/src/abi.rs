//! C-compatible data structures and the callback table shared with the
//! `csharp_runtime` crate.
//!
//! Field order, field widths, and calling conventions in this module are part
//! of the native/managed ABI and must remain synchronized with `EngineApi.cs`.
//!
//! # Responsibilities
//!
//! - Define the C-compatible function table copied by `csharp_runtime`.
//! - Publish the mirror-method table and keep the replaced ones alive.
//! - Track the C# profiling zones of an invocation and close them when it ends.
//! - Describe component blobs, chunks, and scheduler access for managed code.
//!
//! # Design
//!
//! [`CsEngineApi`] is the single entry point the managed runtime copies at
//! startup. Every slot holds one `extern "C"` callback, written here or in this
//! crate's query, deferred-command, context, frame-state, parallel and
//! mirror-call modules. [`NativeComponentBlob`] describes a component value the
//! managed side pins for one call, [`ComponentChunk`] describes an engine-owned
//! column whose validity ends with the managed invocation that received it, and
//! [`NativeSystemAccess`] carries one scheduler permission per queried component.

// Standard library
use std::cell::RefCell;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_engine::{ComponentTicks, Entity};

// Current crate
use super::commands::{
    ffi_queue_add_component, ffi_queue_create, ffi_queue_destroy, ffi_queue_remove_component,
    ffi_reserve_entity,
};
use super::queries::{
    ffi_entity_count, ffi_entity_stable_id, ffi_get_archetype_chunk, ffi_get_component_chunk,
    ffi_get_entity_chunk,
};
use super::ResolvedMirrorMethod;
use crate::INTEROP_CONTRACT_VERSION;

// =============================================================================
// Constants
// =============================================================================

thread_local! {
    /// C# profiling zones are opened and closed on the same scheduler thread.
    /// Keeping the guards in a thread-local avoids moving a `TracyZone` (which
    /// is not `Send`) to another thread, and [`end_unfinished_zones`] closes
    /// any zone a managed invocation left open when it returns.
    static CSHARP_ZONES: RefCell<Vec<(u64, pill_core::profiling::TracyZone)>> =
        const { RefCell::new(Vec::new()) };
}
static NEXT_CSHARP_ZONE_TOKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Host-side table of mirrored Rust methods handed to the managed runtime.
///
/// [`publish_mirror_methods`] fills it when the C# runtime starts and again
/// after every module reload. The two FFI callbacks below read it. A publish
/// replaces the table and keeps the old one in the graveyard, because the copy
/// callback hands pointers into its strings to managed code before the lock is
/// released. Every row owns its strings and no raw pointer is stored, which
/// makes the table `Send` and lets it live in a `static`.
static MIRROR_METHOD_TABLE: std::sync::Mutex<Option<MirrorMethodTable>> =
    std::sync::Mutex::new(None);

/// Epoch of the mirror-method table.
///
/// Increased by one after every republish, so managed code can tell that the
/// addresses it holds were resolved against a table that no longer exists. It
/// can then resolve them again without waiting for the assembly swap that
/// would normally carry a rebind.
static MIRROR_EPOCH: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// How many retired mirror-method tables stay alive after a republish.
///
/// The copy callback hands managed code pointers into the table's strings and
/// releases the lock before those pointers are read, so a republish must not
/// free the table it replaced. Keeping the most recent few is enough: pointers
/// from an older copy are long read by then, and holding every table a session
/// ever published would grow without bound.
const RETIRED_TABLES_KEPT: usize = 16;

/// Tables a republish replaced, newest last, oldest dropped past
/// [`RETIRED_TABLES_KEPT`].
static MIRROR_METHOD_GRAVEYARD: std::sync::Mutex<Vec<MirrorMethodTable>> =
    std::sync::Mutex::new(Vec::new());

// =============================================================================
// Types
// =============================================================================

/// One row of the mirror-method table.
struct MirrorMethodRow {
    /// Fully-qualified Rust type name, NUL-terminated.
    type_name: std::ffi::CString,
    /// Rust method name, NUL-terminated.
    method_name: std::ffi::CString,
    /// Address of the exported `#[no_mangle]` C-ABI trampoline.
    address: usize,
}

/// Owned backing store for the mirror-method table exposed to managed code.
struct MirrorMethodTable {
    /// Rows the copy callback writes out as C-ABI entries.
    rows: Vec<MirrorMethodRow>,
}

/// One mirrored Rust method the managed runtime can call.
///
/// Field order and widths are part of the native/managed ABI and must stay
/// synchronized with `EngineApi.cs`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct MirrorMethodEntry {
    /// Fully-qualified Rust type name (`pill_spline::OmoMO`), NUL-terminated.
    pub(super) type_name: *const std::ffi::c_char,
    /// Rust method name (`get_sum`), NUL-terminated.
    pub(super) method: *const std::ffi::c_char,
    /// Address of the exported `#[no_mangle]` C-ABI trampoline.
    pub(super) address: usize,
}

/// Function table copied by `csharp_runtime` during managed initialization.
///
/// Every slot holds one `extern "C"` callback, defined here or in the query,
/// deferred-command, context, frame-state, parallel and mirror-call modules.
///
/// The first two slots carry the contract version and the table's byte size, so
/// the managed side can refuse a table it was not built against before it
/// copies a single callback. The host checks the runtime's version through
/// `pill_interop_version` before resolving any export; this check is the other
/// direction. A table from a different contract, or one whose slots changed
/// without a version bump, is refused instead of called through a misread
/// pointer.
///
/// A new slot goes at the end, never between existing ones: the managed struct
/// mirror reproduces this field order, so an inserted slot would misalign every
/// slot after it.
#[repr(C)]
pub(super) struct CsEngineApi {
    /// [`INTEROP_CONTRACT_VERSION`] of the host that built this table.
    abi_version: u32,
    /// `size_of::<CsEngineApi>()` of the host that built this table.
    table_size: u32,
    /// Write the number of live entities in the active world.
    ///
    /// `0` wrote the count, `3` no managed system is scheduled, and `5` the
    /// caller passed no output buffer.
    entity_count: extern "C" fn(*mut u32) -> u8,
    /// Fill a [`ComponentChunk`] for one archetype/component pair.
    get_component_chunk: extern "C" fn(u64, u64, u8, u32, *mut ComponentChunk) -> u8,
    /// Fill a [`ComponentChunk`] for one component of an already-known archetype.
    ///
    /// `mode` `0`/`1` request component storage; `mode` `2` requests the
    /// archetype's entity column, which ignores the component key.
    get_archetype_chunk: extern "C" fn(u64, u64, u64, u64, u8, *mut ComponentChunk) -> u8,
    /// Fill a [`ComponentChunk`] describing the entity column.
    get_entity_chunk: extern "C" fn(u32, *mut ComponentChunk) -> u8,
    /// Reserve a generation-checked entity handle without inserting it.
    reserve_entity: extern "C" fn(*mut Entity) -> u8,
    /// Queue creation of an entity with up to `MAX_COMPONENTS_PER_CREATE` blobs.
    queue_create: extern "C" fn(*const Entity, *const NativeComponentBlob, u32) -> u8,
    /// Queue destruction of an entity.
    queue_destroy: extern "C" fn(*const Entity) -> u8,
    /// Queue addition of one component to an entity.
    queue_add_component: extern "C" fn(*const Entity, u64, u64, *const u8, u32) -> u8,
    /// Queue removal of one component from an entity.
    queue_remove_component: extern "C" fn(*const Entity, u64, u64) -> u8,
    /// Report how many mirrored Rust methods are registered.
    mirror_method_count: extern "C" fn() -> u32,
    /// Copy the mirrored-method rows into a managed-owned buffer.
    copy_mirror_methods: extern "C" fn(*mut MirrorMethodEntry, u32) -> u32,
    /// Token of the managed invocation active on the calling thread, or zero.
    current_scope_token: extern "C" fn() -> u32,
    /// Report the epoch of the mirror-method table.
    mirror_epoch: extern "C" fn() -> u32,
    /// Fill a [`ResourceView`] for one resource the active system declared.
    ///
    /// Arguments are the stable identity's low and high halves, the requested
    /// mode (`0` read, `1` write) and the output view.
    get_resource_view: extern "C" fn(u64, u64, u8, *mut ResourceView) -> u8,
    /// Emit a managed log event through the shared tracing subscriber.
    csharp_log: extern "C" fn(u8, *const u8, u32, *const u8, u32),
    /// Open a dynamic managed Tracy zone and return its token.
    csharp_zone_begin: extern "C" fn(*const u8, u32) -> u64,
    /// Close the dynamic managed Tracy zone the token names.
    csharp_zone_end: extern "C" fn(u64),
    /// Whether a key, by its [`KeyCode`](pill_engine::KeyCode) discriminant,
    /// is held this frame.
    input_key_held: extern "C" fn(u8) -> u8,
    /// Whether a mouse button, by discriminant, is held this frame.
    input_mouse_button_held: extern "C" fn(u8) -> u8,
    /// Write this frame's mouse motion in the frontend's raw units to the two
    /// outputs; `1` when written.
    input_mouse_delta: extern "C" fn(*mut f32, *mut f32) -> u8,
    /// The frame's clamped delta in seconds; `0.0` outside a managed
    /// invocation.
    time_delta_seconds: extern "C" fn() -> f32,
    /// Seconds since the engine started; `0.0` outside a managed invocation.
    time_elapsed_seconds: extern "C" fn() -> f32,
    /// Run the managed trampoline for every index in `0..count` on the shared
    /// Rayon pool, joining before returning. Status `0` ran every index, `3`
    /// no managed system is scheduled on the calling thread, `4` the call
    /// came from inside a parallel callback, `5` a panic escaped the dispatch.
    /// `state` is opaque and travels back to `callback` untouched.
    parallel_for:
        extern "C" fn(super::parallel::ParallelCallback, *mut std::ffi::c_void, u32) -> u8,
    /// Take the last error (`0`) or the returned string (`1`) a mirrored call
    /// left on this thread; see `mirror_calls::ffi_take_mirror_text`.
    take_mirror_text: extern "C" fn(u8, *mut *const u8, *mut u32) -> u8,
    /// Write the address of a Rust-owned resource, by its shared name's
    /// identity, under the running invocation's declared access; see
    /// `mirror_calls::ffi_get_native_resource`.
    get_native_resource: extern "C" fn(u64, u64, u8, *mut *mut std::ffi::c_void) -> u8,
    /// Read a file through the mounted asset store for `AssetLoader.Load()`;
    /// see `mirror_calls::ffi_asset_loader_read`.
    asset_loader_read: extern "C" fn(*const u8, u32, *mut *const u8, *mut u32) -> u8,
    /// Write a live entity's stable id as its low and high halves for
    /// `Engine.TryGetStableId`; `0` wrote it, `1` the entity is not live, `3`
    /// no managed system is scheduled, `5` the handle or an output half was
    /// null.
    entity_stable_id: extern "C" fn(*const Entity, *mut u64, *mut u64) -> u8,
}

impl CsEngineApi {
    /// Builds the function table the managed runtime copies at startup.
    ///
    /// Every slot points at the matching `ffi_*` callback, so the managed
    /// runtime never sees Rust-internal representation details. The resolved
    /// `#[pill_mirror_method]` trampolines are published first, through
    /// [`publish_mirror_methods`], and the table then exposes them through the
    /// two mirror-method slots.
    pub(super) fn new(mirror_methods: &[ResolvedMirrorMethod]) -> Self {
        publish_mirror_methods(mirror_methods);

        Self {
            abi_version: INTEROP_CONTRACT_VERSION,
            // A table of a few dozen pointers: far below `u32::MAX`.
            table_size: std::mem::size_of::<Self>() as u32,
            entity_count: ffi_entity_count,
            get_component_chunk: ffi_get_component_chunk,
            get_archetype_chunk: ffi_get_archetype_chunk,
            get_entity_chunk: ffi_get_entity_chunk,
            reserve_entity: ffi_reserve_entity,
            queue_create: ffi_queue_create,
            queue_destroy: ffi_queue_destroy,
            queue_add_component: ffi_queue_add_component,
            queue_remove_component: ffi_queue_remove_component,
            mirror_method_count: ffi_mirror_method_count,
            copy_mirror_methods: ffi_copy_mirror_methods,
            current_scope_token: super::context::ffi_current_scope_token,
            mirror_epoch: ffi_mirror_epoch,
            get_resource_view: super::resources::ffi_get_resource_view,
            csharp_log: ffi_csharp_log,
            csharp_zone_begin: ffi_csharp_zone_begin,
            csharp_zone_end: ffi_csharp_zone_end,
            input_key_held: super::frame_state::ffi_input_key_held,
            input_mouse_button_held: super::frame_state::ffi_input_mouse_button_held,
            input_mouse_delta: super::frame_state::ffi_input_mouse_delta,
            time_delta_seconds: super::frame_state::ffi_time_delta_seconds,
            time_elapsed_seconds: super::frame_state::ffi_time_elapsed_seconds,
            parallel_for: super::parallel::ffi_parallel_for,
            take_mirror_text: super::mirror_calls::ffi_take_mirror_text,
            get_native_resource: super::mirror_calls::ffi_get_native_resource,
            asset_loader_read: super::mirror_calls::ffi_asset_loader_read,
            entity_stable_id: ffi_entity_stable_id,
        }
    }
}

/// One pinned managed component value supplied to a deferred command.
///
/// Managed code pins the data buffer for the complete duration of the call
/// that consumes the blob.
#[repr(C)]
pub(super) struct NativeComponentBlob {
    /// Low half of the stable component key that identifies the component
    /// type.
    ///
    /// [`components::StableComponentId`](super::components::StableComponentId)
    /// owns the key and how it is derived. The key arrives as two halves
    /// because the C ABI has no 128-bit integer.
    pub(super) component_key: u64,
    /// High half of that key, restored into position by `from_halves`.
    pub(super) component_key_high: u64,
    /// Pointer to the managed-pinned component bytes.
    pub(super) data: *const u8,
    /// Byte length of the pinned buffer pointed to by `data`.
    pub(super) size: u32,
}

/// Description of one contiguous ECS column returned to managed code.
///
/// # Managed-side obligations
///
/// - `data`, `entities` and `ticks` point into engine-owned storage. They stay
///   valid only while the scheduled managed invocation that received them runs,
///   and must never be cached or read in a later invocation.
/// - Managed code may write through `data` only for components the system
///   declared with write access, and must stay within `len * element_size`
///   bytes.
/// - Entity columns carry their rows in `entities`, a const pointer, and leave
///   `data` null. Nothing outside the engine may write entity rows.
/// - `ticks` is null only for entity columns. A component column always carries
///   one tick per row, whether the system declared read or write access.
///
/// # Layout limits
///
/// `len` is `u32` by ABI design, so a column holding more than about 4.29
/// billion entities is unsupported and must not be relied upon to wrap.
#[repr(C)]
pub(super) struct ComponentChunk {
    /// Low half of the archetype identifier the column belongs to.
    pub(super) archetype_low: u64,
    /// High half of the archetype identifier the column belongs to.
    pub(super) archetype_high: u64,
    /// Pointer to the first element of the contiguous column storage.
    ///
    /// Null for entity columns, whose rows arrive in `entities` instead.
    pub(super) data: *mut std::ffi::c_void,
    /// Pointer to the first `Entity` of the archetype's entity column.
    ///
    /// The pointer is const and is null for component columns. Declaring it
    /// const makes the read-only contract part of the ABI, so managed code
    /// cannot write through an entity column by accident.
    pub(super) entities: *const std::ffi::c_void,
    /// Number of elements stored in the column.
    pub(super) len: u32,
    /// Byte size of a single column element.
    pub(super) element_size: u32,
    /// Per-element change ticks; null only for entity columns.
    pub(super) ticks: *mut ComponentTicks,
    /// The world's change tick when the chunk was handed out.
    pub(super) change_tick: u32,
    /// Token of the managed invocation this chunk was issued to.
    ///
    /// [`ActiveSystemGuard`](super::context::ActiveSystemGuard) records the
    /// token that was current when the chunk was handed out. A managed debug
    /// build compares it with the live token before reading through the chunk,
    /// so a chunk kept past its invocation fails with a named error instead of
    /// reading memory that has since moved.
    ///
    /// The field fits in the struct's trailing padding on a 64-bit target, so
    /// it changes neither the size nor the alignment.
    pub(super) scope_token: u32,
}

/// One reflected scheduler access, where `0` is read and `1` is write.
///
/// Managed systems declare one entry per queried component before the native
/// side derives scheduler metadata.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct NativeSystemAccess {
    /// Low half of the component key the access entry refers to.
    ///
    /// Same derivation chain as `NativeComponentBlob`'s key: see
    /// [`components::StableComponentId`](super::components::StableComponentId).
    pub(super) component_key: u64,
    /// High half of that key, restored into position by `from_halves`.
    pub(super) component_key_high: u64,
    /// Access mode: `0` is read-only and `1` is read-write.
    pub(super) mode: u8,
    /// What the key names: `0` a component, `1` a managed resource, `2` a
    /// Rust-owned resource reached by its shared name.
    ///
    /// The discriminator is needed because every key is a 128-bit hash of a
    /// name drawn from the same space. Without it, a resource read could be
    /// authorized by a component entry that happened to hash alike, and the
    /// scheduler would record a component access where a resource access was
    /// meant. Two systems writing one resource would then be allowed to run at
    /// the same time. The two resource kinds resolve through different tables:
    /// a managed resource through the manifest's bindings, a native one
    /// straight to `ResourceId::Shared` of its name.
    pub(super) kind: u8,
}

/// Borrowed view of one resource's bytes, handed to managed code.
///
/// This is the resource counterpart of [`ComponentChunk`], and it is much
/// smaller: a resource is a single value, so there is no column to iterate, no
/// archetype to identify and no per-row tick column. It carries only the
/// bytes, their width and the invocation the view was issued to.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct ResourceView {
    /// Pointer to the resource's first byte in engine storage.
    pub(super) data: *mut u8,
    /// Width of the stored value, for the managed side to check its struct against.
    pub(super) length: u32,
    /// Token of the managed invocation this view was issued to.
    pub(super) scope_token: u32,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Copies `managed_buffer_length` bytes of a managed buffer into an owned
/// vector.
///
/// A null buffer, or a length of zero, gives an empty vector.
fn copy_managed_bytes(managed_buffer: *const u8, managed_buffer_length: u32) -> Vec<u8> {
    if managed_buffer.is_null() || managed_buffer_length == 0 {
        return Vec::new();
    }
    // The managed side keeps the buffer pinned for the whole callback. The
    // early return above handles a null buffer, so this reads that many real
    // bytes.
    // SAFETY: `managed_buffer` is non-null and points at `managed_buffer_length`
    // bytes the managed caller keeps pinned until this callback returns; the
    // bytes are copied out here.
    unsafe { std::slice::from_raw_parts(managed_buffer, managed_buffer_length as usize).to_vec() }
}

/// Forwards one managed log line to the engine's tracing subscriber.
///
/// The line lands on [`telemetry_target::CSHARP`] at the managed level, with
/// the managed logger's own category as the `managed_target` field. The
/// message is forwarded text written by managed code, so it is passed through
/// as the message rather than worded here. A level outside the five known
/// ones logs at `warn`, so an unexpected value is visible without reading as
/// a failure.
extern "C" fn ffi_csharp_log(
    level: u8,
    target: *const u8,
    target_len: u32,
    message: *const u8,
    message_len: u32,
) {
    let target_bytes = copy_managed_bytes(target, target_len);
    let message_bytes = copy_managed_bytes(message, message_len);
    let target = String::from_utf8_lossy(&target_bytes).into_owned();
    let message = String::from_utf8_lossy(&message_bytes).into_owned();
    match level {
        0 => {
            tracing::trace!(target: telemetry_target::CSHARP, managed_target = %target, "{message}")
        }
        1 => {
            tracing::debug!(target: telemetry_target::CSHARP, managed_target = %target, "{message}")
        }
        2 => {
            tracing::info!(target: telemetry_target::CSHARP, managed_target = %target, "{message}")
        }
        3 => {
            tracing::warn!(target: telemetry_target::CSHARP, managed_target = %target, "{message}")
        }
        4 => {
            tracing::error!(target: telemetry_target::CSHARP, managed_target = %target, "{message}")
        }
        _ => tracing::warn!(
            target: telemetry_target::CSHARP,
            managed_target = %target,
            managed_level = level,
            "{message}"
        ),
    }
}

/// Opens a dynamic managed Tracy zone and returns the token that names it.
extern "C" fn ffi_csharp_zone_begin(name: *const u8, name_len: u32) -> u64 {
    let name = String::from_utf8_lossy(&copy_managed_bytes(name, name_len)).into_owned();
    let token = NEXT_CSHARP_ZONE_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let zone = pill_core::profiling::TracyZone::new_dynamic(&name, "csharp", "managed", 0);
    CSHARP_ZONES.with(|zones| zones.borrow_mut().push((token, zone)));
    token
}

/// Ends the dynamic managed Tracy zone that `token` names.
extern "C" fn ffi_csharp_zone_end(token: u64) {
    CSHARP_ZONES.with(|zones| {
        let mut zones = zones.borrow_mut();
        if let Some(index) = zones.iter().position(|(candidate, _)| *candidate == token) {
            drop(zones.remove(index));
        }
    });
}

/// Closes every zone this invocation opened and never ended.
///
/// The managed side owns a zone's lifetime through `Profiler.Zone`, but a
/// system that forgets to dispose of one, or throws between begin and end, must
/// not leave the guard open. A Tracy zone that outlives the invocation would be
/// recorded against the wrong part of the frame, and the vector that holds it
/// would grow by one entry per frame. The invocation guard calls this while it
/// tears down, at the point where one system's work ends and the next begins.
pub(super) fn end_unfinished_zones() {
    CSHARP_ZONES.with(|zones| {
        // A panic while `ffi_csharp_zone_begin` holds the borrow would poison
        // the cell. The zone it was inserting is already gone in that case, so
        // the clear can be skipped.
        if let Ok(mut zones) = zones.try_borrow_mut() {
            zones.clear();
        }
    });
}

/// Report how many mirrored Rust methods are registered.
extern "C" fn ffi_mirror_method_count() -> u32 {
    MIRROR_METHOD_TABLE
        .lock()
        .map(|table| table.as_ref().map_or(0, |table| table.rows.len() as u32))
        .unwrap_or(0)
}

/// Report the epoch of the mirror-method table.
///
/// Managed code polls this before resolving a mirrored method: a value it has
/// not seen means the addresses it cached belong to an older published table.
extern "C" fn ffi_mirror_epoch() -> u32 {
    MIRROR_EPOCH.load(std::sync::atomic::Ordering::Acquire)
}

/// Copies up to `max` mirror-method rows into `out` and returns the number
/// written.
///
/// # Safety
///
/// `out` must point at `max` writable [`MirrorMethodEntry`] slots owned by the
/// managed runtime for the duration of the call.
extern "C" fn ffi_copy_mirror_methods(out: *mut MirrorMethodEntry, max: u32) -> u32 {
    let Ok(table) = MIRROR_METHOD_TABLE.lock() else {
        return 0;
    };
    let Some(table) = table.as_ref() else {
        return 0;
    };
    let count = (table.rows.len() as u32).min(max);
    for (index, row) in table.rows.iter().take(count as usize).enumerate() {
        let entry = MirrorMethodEntry {
            type_name: row.type_name.as_ptr(),
            method: row.method_name.as_ptr(),
            address: row.address,
        };
        // SAFETY: `index < count <= max`, so `out.add(index)` stays inside the
        // buffer the caller promised, and `MirrorMethodEntry` is `Copy`.
        unsafe {
            out.add(index).write(entry);
        }
    }
    count
}

/// Rebuilds the mirror-method table the managed runtime copies at startup.
///
/// [`CsEngineApi::new`] calls it when the C# backend starts, and the host calls
/// it when an extension reloads, so the C# side always resolves trampoline
/// addresses against the module generations that are currently loaded. A hot
/// reload gives a module a new image, and therefore new addresses, while a
/// reloaded module may have added or removed mirrored methods.
///
/// # Panics
///
/// Panics if the table lock or the graveyard lock was poisoned by a panic on
/// another thread that held it.
pub fn publish_mirror_methods(mirror_methods: &[ResolvedMirrorMethod]) {
    // Step 1: Build the owned rows. Each row owns its two NUL-terminated names,
    // so the pointers the copy callback hands out stay valid for as long as the
    // table lives, and the table stays `Send` because it holds no index into a
    // shared buffer. A name containing a NUL byte cannot become a C string, so
    // that method is skipped.
    let mut rows: Vec<MirrorMethodRow> = Vec::new();
    for method in mirror_methods
        .iter()
        .filter(|method| super::mirror_calls::is_callable_by_managed_code(method))
    {
        let Some(type_name) = std::ffi::CString::new(method.type_name.as_str()).ok() else {
            continue;
        };
        let Some(method_name) = std::ffi::CString::new(method.method_name.as_str()).ok() else {
            continue;
        };
        rows.push(MirrorMethodRow {
            type_name,
            method_name,
            address: method.address,
        });
    }

    // Step 2: Replace the table and keep the old one. The copy callback hands
    // out pointers into that table's strings and releases the lock before
    // managed code reads them, so the replaced table must stay alive instead of
    // being dropped.
    let previous = MIRROR_METHOD_TABLE
        .lock()
        .unwrap()
        .replace(MirrorMethodTable { rows });
    if let Some(previous) = previous {
        let mut graveyard = MIRROR_METHOD_GRAVEYARD.lock().unwrap();
        graveyard.push(previous);
        if graveyard.len() > RETIRED_TABLES_KEPT {
            graveyard.remove(0);
        }
    }

    // Step 3: Raise the epoch after the rows are stored, so a managed refresh
    // triggered by this value copies the new table rather than the one it
    // replaced.
    MIRROR_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Release);
}

/// Open a zone named `name` for a native test, returning its token.
#[cfg(test)]
pub(super) fn begin_managed_zone_for_test(name: &str) -> u64 {
    ffi_csharp_zone_begin(name.as_ptr(), name.len() as u32)
}

/// How many zones are still open on the current thread, for a native test.
#[cfg(test)]
pub(super) fn managed_zone_count_for_test() -> usize {
    CSHARP_ZONES.with(|zones| zones.borrow().len())
}

/// How many tables the graveyard currently holds.
#[cfg(test)]
fn retired_table_count() -> usize {
    MIRROR_METHOD_GRAVEYARD
        .lock()
        .map(|graveyard| graveyard.len())
        .unwrap_or(0)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// One publishable mirror row: a callable method with a trampoline.
    fn resolved_mirror_method(
        type_name: &str,
        method_name: &str,
        address: usize,
    ) -> crate::ResolvedMirrorMethod {
        crate::ResolvedMirrorMethod {
            type_name: type_name.to_owned(),
            method_name: method_name.to_owned(),
            return_tag: String::new(),
            arg_tags: Vec::new(),
            arg_names: Vec::new(),
            address,
            is_free_function: false,
            crate_name: "pill_csharp_bridge".to_owned(),
            receiver: "ref".to_owned(),
            owner_kind: "object".to_owned(),
        }
    }

    /// An entry slot for the copy callback to write into.
    fn empty_entry() -> MirrorMethodEntry {
        MirrorMethodEntry {
            type_name: std::ptr::null(),
            method: std::ptr::null(),
            address: 0,
        }
    }

    #[test]
    fn a_republish_keeps_the_names_a_copy_handed_out_alive_and_stays_bounded() {
        // One test, because the table and its graveyard are process globals: a
        // second test running beside this one would evict the table whose
        // pointers this one reads.
        publish_mirror_methods(&[resolved_mirror_method("alpha::Probe", "read", 0x1000)]);
        let mut entries = [empty_entry(); 4];
        let written = ffi_copy_mirror_methods(entries.as_mut_ptr(), entries.len() as u32);
        assert_eq!(written, 1, "the one callable row is copied");

        // A module reload republishes while managed code still holds the
        // pointers the copy above returned.
        for round in 0..RETIRED_TABLES_KEPT - 1 {
            publish_mirror_methods(&[resolved_mirror_method(
                "beta::Probe",
                &format!("read{round}"),
                0x2000,
            )]);
        }

        // SAFETY: the graveyard still holds the replaced tables, which is what
        // this test proves. The pointers therefore still name the strings the
        // copy handed out.
        let type_name = unsafe { std::ffi::CStr::from_ptr(entries[0].type_name) };
        // SAFETY: the same lifetime argument as above.
        let method_name = unsafe { std::ffi::CStr::from_ptr(entries[0].method) };
        assert_eq!(type_name.to_str().unwrap(), "alpha::Probe");
        assert_eq!(method_name.to_str().unwrap(), "read");
        assert_eq!(entries[0].address, 0x1000);

        // Past the bound the oldest tables go, so a session that reloads for
        // hours does not accumulate every table it ever published.
        for round in 0..8 {
            publish_mirror_methods(&[resolved_mirror_method(
                "gamma::Probe",
                &format!("read{round}"),
                0x3000,
            )]);
        }
        assert_eq!(
            retired_table_count(),
            RETIRED_TABLES_KEPT,
            "the graveyard keeps the recent tables and drops the oldest"
        );
    }
}
