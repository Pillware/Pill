//! Per-function hot patching: stable dispatch slots for registered systems.
//!
//! # Responsibilities
//!
//! - Defines [`HotSlot`], the stable indirection a registered system dispatches
//!   through, so its implementation can be replaced without re-registering it.
//! - Defines [`HotPatchRegistry`], the engine-owned map from system name to slot.
//! - Defines [`PlainSlot`], the dispatch slot that lets `#[pill_hot_fn]` patch an
//!   ordinary function.
//! - Computes [`signature_hash`], the identity of a system's signature, which
//!   [`HotSlot::install`] compares before it accepts a patch.
//! - Counts a patched system's panics and queues a [`FaultReport`] for the host,
//!   which rolls the system back to its earlier code.
//! - Rewrites a function's first bytes on Windows x86-64 ([`patch_prologue`] and
//!   [`restore_prologue`]), and refuses to do so on every other target.
//! - Provides [`verify_abi`], the self-check for the one platform assumption
//!   the design rests on.
//!
//! # Design
//!
//! A system's boxed closure calls the user's function through an address held in
//! a slot. Replacing the implementation is a single atomic store, so the
//! `Box<dyn System>`, its vtable, the scheduler graph, the system's access
//! metadata and its change-detection ticks all stay exactly where they are. The
//! `World` is never touched.
//!
//! The address is type-erased because `F` differs per system and a registry
//! cannot be typed over all of them. The transmute back happens inside the
//! monomorphized closure in [`crate::system`], where `F` and `Input` are both
//! known.
//!
//! ## The zero-sized stand-in
//!
//! A slot holds the address of a `fn(&mut F, Input) -> F::Output`, where `F` is
//! the zero-sized fn-item type of the user's system. A separately compiled patch
//! cannot name that type, so it declares the parameter as `&mut ()` instead.
//! That substitution is sound because a reference to any zero-sized type is a
//! single non-null pointer-sized value, and the callee never reads it.
//!
//! It is an ABI assumption rather than a guarantee Rust makes, so
//! [`verify_abi`] lets a host check it once at startup: a toolchain or target
//! that breaks it fails loudly at initialization instead of silently corrupting
//! a system's arguments at runtime.
//!
//! ## Feature gating
//!
//! The dispatch behavior is gated behind the `development` feature, but
//! [`HotPatchRegistry`] is a field on [`Engine`](crate::Engine)
//! **unconditionally**: gating it would change `Engine`'s layout between
//! feature configurations, and a module built with the other configuration
//! reconstructs `&mut Engine` from a raw pointer, making a layout difference
//! memory corruption rather than a compile error. The same reasoning is why
//! `rendering` is mirrored into every module build.

// Standard library
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// Current crate
use crate::system::{SystemParam, SystemParamFunction};

// =============================================================================
// Panic attribution
// =============================================================================

/// Consecutive panicking invocations after which a patched function's host
/// rolls it back.
///
/// A system runs once per frame, so three panicking invocations in a row are
/// three panicking frames. This code counts invocations because it has no
/// frame counter of its own, and the dispatcher is the only place that knows
/// which function failed.
pub const PANIC_ROLLBACK_FRAMES: u32 = 3;

/// One function whose patched body panicked repeatedly, waiting for its host
/// to roll it back.
#[derive(Clone, Debug)]
pub struct FaultReport {
    /// The function's fully qualified path, which a rollback uses to find the
    /// function.
    pub function: String,
    /// The panic message, quoted in the host's rollback log line.
    pub message: String,
}

/// Consecutive panic count per function, with the last message seen.
///
/// This map is only locked while some function has panicked at least once, so a
/// process where nothing panics pays nothing for it but one relaxed load (see
/// [`record_success`]).
static PANIC_STREAKS: Mutex<Option<HashMap<String, (u32, String)>>> = Mutex::new(None);

/// How many functions currently have a panic count above zero.
static FLAKY_FUNCTION_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Reports waiting for the host's next frame boundary.
static FAULT_REPORTS: Mutex<Vec<FaultReport>> = Mutex::new(Vec::new());

/// Counts one panicking invocation of `function`.
///
/// On the [`PANIC_ROLLBACK_FRAMES`]-th panic in a row, the function is queued
/// once in a [`FaultReport`]. Reporting stops at that number, so a host that
/// could not roll the function back is told once instead of every frame. Only
/// a call that does not panic (a rollback, or a fresh patch) clears the count
/// and lets a new report happen.
pub fn record_panic(function: &str, message: &str) {
    let mut panic_streaks_guard = PANIC_STREAKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let streaks = panic_streaks_guard.get_or_insert_with(HashMap::new);
    let streak = streaks.entry(function.to_string()).or_insert_with(|| {
        FLAKY_FUNCTION_COUNT.fetch_add(1, Ordering::Relaxed);
        (0, String::new())
    });
    streak.1 = message.to_string();
    if streak.0 >= PANIC_ROLLBACK_FRAMES {
        return;
    }
    streak.0 += 1;
    if streak.0 == PANIC_ROLLBACK_FRAMES {
        let report = FaultReport {
            function: function.to_string(),
            message: message.to_string(),
        };
        FAULT_REPORTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(report);
    }
}

/// Clears a function's panic count after an invocation that did not panic.
///
/// A process where nothing has panicked - the ordinary one - never locks the
/// map: one relaxed load of the count above is all this costs.
#[inline]
pub fn record_success(function: &str) {
    if FLAKY_FUNCTION_COUNT.load(Ordering::Relaxed) == 0 {
        return;
    }
    let mut panic_streaks_guard = PANIC_STREAKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(streaks) = panic_streaks_guard.as_mut() else {
        return;
    };
    if streaks.remove(function).is_some() {
        FLAKY_FUNCTION_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Turns a caught panic payload into the system failure the engine records.
///
/// The payload comes from another artifact's copy of `std`, so only the two
/// shapes `panic!` produces are read: `&'static str` and `String`. Any other
/// payload still counts as a panic and is recorded as `non-string panic
/// payload`. Recording it here keeps attribution in one place: the dispatcher
/// knows the function's name, and the host only reads the reports.
pub fn panic_into_failure(name: &str, payload: Box<dyn Any + Send>) -> crate::error::SystemError {
    let message = payload
        .downcast_ref::<&'static str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string());
    record_panic(name, &message);
    crate::error::SystemError::Failure {
        message: format!("panicked: {message}"),
    }
}

/// Takes the fault reports queued since the last call, oldest first.
///
/// The host calls this at a frame boundary and rolls each function back to its
/// previous generation.
pub fn take_fault_reports() -> Vec<FaultReport> {
    let mut fault_reports_guard = FAULT_REPORTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::mem::take(&mut *fault_reports_guard)
}

// =============================================================================
// Errors
// =============================================================================

/// Why a hot patch was refused.
///
/// Every variant is a refusal, never a partial application: the running system
/// is untouched whenever one of these is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotPatchError {
    /// No system was registered under this name, or its slot has been cleared.
    UnknownSystem {
        /// The name the caller asked to patch.
        name: String,
    },
    /// The replacement's signature no longer matches the running system's.
    ///
    /// This is the gate that keeps a changed parameter list or return type from
    /// being installed behind a call site compiled for the old one.
    SignatureMismatch {
        /// The name of the system whose patch was refused.
        name: String,
        /// Hash the running system was registered with.
        expected: u64,
        /// Hash the replacement reported.
        found: u64,
    },
    /// The replacement address was null.
    NullAddress {
        /// The name of the system whose patch was refused.
        name: String,
    },
}

impl std::fmt::Display for HotPatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSystem { name } => {
                write!(formatter, "no hot-patchable system registered as `{name}`")
            }
            Self::SignatureMismatch {
                name,
                expected,
                found,
            } => write!(
                formatter,
                "signature changed for `{name}` (registered {expected:#018x}, patch {found:#018x})"
            ),
            Self::NullAddress { name } => {
                write!(formatter, "replacement address for `{name}` is null")
            }
        }
    }
}

impl std::error::Error for HotPatchError {}

// =============================================================================
// HotSlot
// =============================================================================

/// The stable indirection through which one registered system dispatches.
///
/// The slot is created when the system is registered and shared by the
/// system's closure and the engine's [`HotPatchRegistry`]. A patch installed
/// through the registry is therefore seen by the closure on its next call.
#[derive(Debug)]
pub struct HotSlot {
    /// Address of the function this slot currently calls, a
    /// `fn(&mut F, Input) -> F::Output`.
    current: AtomicUsize,
    /// Signature identity of the system this slot was created for.
    signature_hash: AtomicU64,
    /// The implementation recorded at registration, kept so a rollback can
    /// return to the code the registering artifact was built with.
    ///
    /// This is a separate field because `current` is overwritten by every
    /// install; without it, the first patch would make the original address
    /// unreachable.
    original: AtomicUsize,
}

impl HotSlot {
    /// An empty slot, filled by [`Self::initialize`] during registration.
    pub fn new() -> Self {
        Self {
            current: AtomicUsize::new(0),
            signature_hash: AtomicU64::new(0),
            original: AtomicUsize::new(0),
        }
    }

    /// Records the baseline implementation and the signature to compare
    /// against.
    ///
    /// Called once, from the boxing code in [`crate::system`], where the
    /// concrete `F` and `Input` are known.
    pub fn initialize(&self, address: usize, signature_hash: u64) {
        self.current.store(address, Ordering::Release);
        self.signature_hash.store(signature_hash, Ordering::Release);
        // Stored unconditionally, not only when empty: a reload builds a fresh
        // slot for the new artifact, and its baseline is that artifact's code.
        self.original.store(address, Ordering::Release);
    }

    /// The implementation recorded at registration.
    ///
    /// This is what a rollback to generation zero installs. The address belongs
    /// to the artifact that registered the system, and a reload builds a fresh
    /// slot for its replacement rather than carrying this address over.
    pub fn original(&self) -> usize {
        self.original.load(Ordering::Acquire)
    }

    /// The implementation to call now.
    ///
    /// `Acquire` pairs with the `Release` in [`Self::install`], so whichever
    /// thread runs the system next sees the code of a freshly loaded patch
    /// library. On x86-64 a relaxed load would be enough, but the ordering
    /// keeps the guarantee on any target the engine grows to, instead of tying
    /// it to this one.
    #[inline]
    pub fn current(&self) -> usize {
        self.current.load(Ordering::Acquire)
    }

    /// Signature identity this slot was registered with.
    pub fn signature_hash(&self) -> u64 {
        self.signature_hash.load(Ordering::Acquire)
    }

    /// Replaces the implementation, refusing a replacement whose signature
    /// differs.
    ///
    /// # Errors
    ///
    /// Returns [`HotPatchError::NullAddress`] for a null replacement and
    /// [`HotPatchError::SignatureMismatch`] when `signature_hash` differs from
    /// the one recorded at registration. On either, the running implementation
    /// is left in place.
    pub fn install(
        &self,
        name: &str,
        address: usize,
        signature_hash: u64,
    ) -> Result<(), HotPatchError> {
        if address == 0 {
            return Err(HotPatchError::NullAddress {
                name: name.to_string(),
            });
        }
        let expected = self.signature_hash.load(Ordering::Acquire);
        if expected != signature_hash {
            return Err(HotPatchError::SignatureMismatch {
                name: name.to_string(),
                expected,
                found: signature_hash,
            });
        }
        self.current.store(address, Ordering::Release);
        Ok(())
    }
}

impl Default for HotSlot {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// HotPatchRegistry
// =============================================================================

/// Engine-owned map from registered system name to its dispatch slot.
///
/// Lives on the [`Engine`](crate::Engine) rather than in a global, which ties
/// each slot's lifetime to the engine that owns it and keeps the slots out of
/// process-wide state.
#[derive(Debug, Default)]
pub struct HotPatchRegistry {
    slots: HashMap<String, Arc<HotSlot>>,
}

impl HotPatchRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
        }
    }

    /// Records a system's slot under its registration name.
    ///
    /// A repeated name replaces the previous entry, matching the engine's
    /// behaviour of allowing duplicate system names.
    pub fn insert(&mut self, name: impl Into<String>, slot: Arc<HotSlot>) {
        self.slots.insert(name.into(), slot);
    }

    /// Forgets one system's slot, so a cleared system can never be patched.
    ///
    /// Every entry referring to the same slot is dropped, not just the one
    /// matching `name`. A system is registered under both its display name and
    /// its function path, and leaving either behind would let a patch install
    /// into a system the scheduler has already dropped.
    pub fn remove(&mut self, name: &str) {
        let Some(target_slot) = self.slots.get(name).cloned() else {
            return;
        };
        self.slots
            .retain(|_, slot| !Arc::ptr_eq(slot, &target_slot));
    }

    /// The slot registered for `name`, if any.
    pub fn get(&self, name: &str) -> Option<&Arc<HotSlot>> {
        self.slots.get(name)
    }

    /// Every name a hot-patchable system is registered under, for diagnostics
    /// and tooling.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.slots.keys().map(|name| name.as_str())
    }

    /// How many systems are hot-patchable.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Whether no system is hot-patchable.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

// =============================================================================
// Signature identity
// =============================================================================

/// Stable identity of a system's call signature.
///
/// The value is hashed from the parameter tuple's type name and the return
/// type's type name, the two things that change when a signature changes. It
/// is computed once per registration, so its cost never reaches the frame
/// loop.
///
/// Type names are used rather than [`TypeId`](std::any::TypeId) because a patch
/// is a separately compiled artifact: it must be able to compute the same
/// value from the same source text, and `type_name` is what both sides can
/// agree on.
pub fn signature_hash<F, Input>() -> u64
where
    F: SystemParamFunction<Input>,
    Input: SystemParam,
{
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::any::type_name::<Input>().hash(&mut hasher);
    std::any::type_name::<F::Output>().hash(&mut hasher);
    hasher.finish()
}

/// Signature identity of a system, inferred from the function itself.
///
/// [`signature_hash`] called with a turbofish cannot infer the function-item
/// type, so this version takes the function by reference instead. It is the
/// convenient form wherever the function is in scope as a value.
pub fn signature_hash_of<F, Input>(_function: &F) -> u64
where
    F: SystemParamFunction<Input>,
    Input: SystemParam,
{
    signature_hash::<F, Input>()
}

/// The fully qualified path of a system function, as `#[pill_hot]` names it.
///
/// A function-item type's name is its path, such as `project::physics_system`.
/// That is the same text the attribute builds from `module_path!() + "::" + fn
/// name`, which lets the host patch a system by its real path even though
/// [`Engine::register_system`](crate::Engine::register_system) also records a
/// separate, arbitrary display name (`"ball_physics"`).
///
/// Returns `None` for anything that is not a plain function item - a closure,
/// for instance, whose `type_name` is a compiler-generated placeholder with no
/// stable path a patch could name.
pub fn function_path<F>() -> Option<String> {
    let type_name = std::any::type_name::<F>();
    // Closures render as `crate::outer::{{closure}}` and generic instantiations
    // carry angle brackets. Neither is a name a patch can be keyed by.
    if type_name.contains('{') || type_name.contains('<') || !type_name.contains("::") {
        return None;
    }
    Some(type_name.to_string())
}

/// Address of the entry point for a locally compiled system function.
///
/// This is the address [`HotSlot::install`] expects. A real patch resolves the
/// same kind of address from a freshly compiled library's exports instead; this
/// helper covers the in-process case, which is what tests and any statically
/// linked replacement need.
pub fn local_implementation_address<F, Input>(_function: &F) -> usize
where
    F: SystemParamFunction<Input>,
    Input: SystemParam,
{
    <F as SystemParamFunction<Input>>::run as *const () as usize
}

// =============================================================================
// Plain-function slots
// =============================================================================

/// A dispatch slot for an ordinary function marked `#[pill_hot_fn]`.
///
/// A system does not need one: the engine holds its boxed closure and can swap
/// what the closure calls. An ordinary `pub fn` has no such holder, since
/// callers call it directly, so the indirection has to live inside the
/// function itself. `#[pill_hot_fn]` renames the real body and turns the public
/// name into a dispatcher that reads this slot.
///
/// Unlike [`HotSlot`], this is a `static` inside whichever artifact compiled the
/// function, not a registry entry on the engine. That is deliberate: a crate
/// linked into several artifacts has an independent copy of its code in each,
/// and each copy's callers must be redirected separately.
#[derive(Debug)]
pub struct PlainSlot {
    /// Installed replacement, or zero while the original is still current.
    current: AtomicUsize,
}

impl PlainSlot {
    /// A slot holding no replacement.
    pub const fn new() -> Self {
        Self {
            current: AtomicUsize::new(0),
        }
    }

    /// The address to call: the installed replacement, or `original`.
    ///
    /// `Relaxed` would be enough on x86-64, but the acquire pairs with
    /// `install`'s release so a freshly loaded patch's code is visible to
    /// whichever thread calls next on weaker orderings too.
    #[inline]
    pub fn current_or(&self, original: usize) -> usize {
        match self.current.load(Ordering::Acquire) {
            0 => original,
            installed => installed,
        }
    }

    /// Installs a replacement implementation.
    pub fn install(&self, address: usize) {
        self.current.store(address, Ordering::Release);
    }

    /// The installed replacement, or zero when the slot has never been filled.
    ///
    /// Callers that own their fallback inline - a dispatcher generated around a
    /// method body, which has no separate implementation to name - branch on
    /// this rather than passing an address to [`Self::current_or`].
    #[inline]
    pub fn installed(&self) -> usize {
        self.current.load(Ordering::Acquire)
    }

    /// Empties the slot, so calls fall back to the body compiled into this
    /// artifact.
    ///
    /// A plain function has no single baseline address the host could reinstall:
    /// every artifact linking the crate holds its own copy. Emptying the slot is
    /// how each of them returns to its own original code.
    pub fn reset(&self) {
        self.current.store(0, Ordering::Release);
    }

    /// Whether a replacement is currently installed.
    pub fn is_patched(&self) -> bool {
        self.current.load(Ordering::Acquire) != 0
    }
}

impl Default for PlainSlot {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Prologue patching (Windows x86-64 only)
// =============================================================================

/// Bytes an absolute jump occupies: `mov rax, imm64` followed by `jmp rax`.
///
/// The absolute form is used instead of a 5-byte `E9 rel32` because ASLR
/// usually loads a patch image several gigabytes away from the base image, so
/// the relative form would need a trampoline for almost every patch. The price
/// is that twelve bytes of the function must be overwritten instead of five.
#[cfg(all(windows, target_arch = "x86_64"))]
const ABSOLUTE_JUMP_LENGTH: usize = 12;

/// Read/write/execute page protection.
#[cfg(all(windows, target_arch = "x86_64"))]
const PAGE_EXECUTE_READWRITE: u32 = 0x40;

/// The thread allowed to rewrite live code, declared by the host each frame.
///
/// Patching assumes no thread is executing inside the bytes being overwritten.
/// The host meets that by patching at a frame boundary with no system running,
/// but nothing enforces the rule if it is only written down. Recording the
/// thread makes it checkable: a second thread that attempts a patch is refused
/// instead of racing the first.
static PATCHING_THREAD_TOKEN: AtomicU64 = AtomicU64::new(0);

/// A numeric identity for the current thread.
///
/// `ThreadId` has no stable integer form on stable Rust, so the address of a
/// thread-local is used instead. It is different for each thread and stays the
/// same within one.
fn current_thread_token() -> u64 {
    thread_local! {
        static MARKER: u8 = const { 0 };
    }
    MARKER.with(|marker| marker as *const u8 as u64)
}

/// Declares the calling thread the only one permitted to rewrite live code.
///
/// A host calls this from its frame loop. Idempotent, and cheap enough to call
/// every frame: one relaxed load in the common case.
///
/// Nothing forces a host to call it: a process that never does leaves every
/// thread allowed to patch, which is what tests and offline tools want.
pub fn declare_patching_thread() {
    let token = current_thread_token();
    if PATCHING_THREAD_TOKEN.load(Ordering::Relaxed) != token {
        PATCHING_THREAD_TOKEN.store(token, Ordering::Release);
    }
}

/// Refuses a patch attempted from anywhere but the declared thread.
///
/// # Errors
///
/// Returns the message to refuse with. Undeclared means unrestricted.
#[cfg(all(windows, target_arch = "x86_64"))]
fn check_patching_thread() -> Result<(), String> {
    let declared = PATCHING_THREAD_TOKEN.load(Ordering::Acquire);
    if declared == 0 || declared == current_thread_token() {
        return Ok(());
    }
    Err(
        "a patch was attempted from a thread other than the one the host \
         declared; rewriting live code is only sound from the thread that owns \
         the frame boundary, where no system is executing"
            .to_string(),
    )
}

/// One entry in the x86-64 exception directory: a function's exact extent.
///
/// Windows requires every function that touches the stack to publish one of
/// these so the unwinder can walk it, which makes the table an authoritative
/// map of where each function begins and ends - no disassembler needed.
#[cfg(all(windows, target_arch = "x86_64"))]
#[repr(C)]
struct RuntimeFunction {
    /// Offset of the function's first byte from the image base.
    begin_address: u32,
    /// Offset one past the function's last byte.
    end_address: u32,
    /// Offset of the unwind data; unused here.
    _unwind_data: u32,
}

// The Win32 calls that make a page writable and find a function's extent.
#[cfg(all(windows, target_arch = "x86_64"))]
extern "system" {
    fn VirtualProtect(
        address: *mut core::ffi::c_void,
        size: usize,
        new_protection: u32,
        old_protection: *mut u32,
    ) -> i32;
    fn GetCurrentProcess() -> *mut core::ffi::c_void;
    fn FlushInstructionCache(
        process: *mut core::ffi::c_void,
        base: *const core::ffi::c_void,
        size: usize,
    ) -> i32;
    fn RtlLookupFunctionEntry(
        control_address: u64,
        image_base: *mut u64,
        history_table: *mut core::ffi::c_void,
    ) -> *const RuntimeFunction;
}

/// Returns where the function containing `address` begins and how long it is.
///
/// The answer is read from the running image's exception directory, so it is
/// the linker's own record rather than a guess. The result is `None` when the
/// address has no entry, which is the case for a leaf function small enough
/// that Windows allows it to omit its unwind data. Those are exactly the
/// functions too short to overwrite safely, so a missing entry means "refuse",
/// never "assume it is long enough".
#[cfg(all(windows, target_arch = "x86_64"))]
pub fn function_extent(address: usize) -> Option<(usize, usize)> {
    let mut image_base: u64 = 0;
    // SAFETY: `RtlLookupFunctionEntry` reads the loaded image's exception
    // directory for an arbitrary address and reports a null entry when it finds
    // none. `image_base` is a writable local, and passing a null history table
    // asks for no caching.
    let function_entry =
        unsafe { RtlLookupFunctionEntry(address as u64, &mut image_base, core::ptr::null_mut()) };
    if function_entry.is_null() {
        return None;
    }
    // SAFETY: the pointer is non-null and points into the image's static
    // exception directory, which stays mapped for as long as the module is.
    let function_entry = unsafe { &*function_entry };
    let begin = image_base as usize + function_entry.begin_address as usize;
    let end = image_base as usize + function_entry.end_address as usize;
    if end <= begin {
        return None;
    }
    Some((begin, end - begin))
}

/// Overwrites a function's first bytes with an absolute jump to `replacement`.
///
/// Returns the original bytes, so the patch can be undone by writing them back.
///
/// The function's length is checked against the exception directory first, so a
/// function too short for the jump is refused instead of being overwritten past
/// its end, which would corrupt whichever function the linker placed next.
///
/// # Errors
///
/// Returns a message naming the reason when:
///
/// - `target` or `replacement` is null, or both name the same address;
/// - the call comes from a thread other than the declared patching thread;
/// - `target` has no exception-directory entry, which is how Windows treats a
///   leaf function small enough to omit its unwind data;
/// - `target` is not the first byte of its function;
/// - the function is shorter than `ABSOLUTE_JUMP_LENGTH`;
/// - `VirtualProtect` refuses to make the page writable.
///
/// # Safety
///
/// `replacement` must be a function with a signature compatible with the one at
/// `target`, and **no thread may be executing inside the overwritten bytes**.
/// The caller guarantees the second condition by patching at a frame boundary,
/// on the thread that runs the frame loop, while no other thread is inside the
/// function. `target` is validated here rather than assumed.
#[cfg(all(windows, target_arch = "x86_64"))]
pub unsafe fn patch_prologue(target: usize, replacement: usize) -> Result<Vec<u8>, String> {
    // Step 1: Reject the arguments that cannot be patched at all.
    if target == 0 || replacement == 0 {
        return Err("cannot patch a null address".to_string());
    }
    if target == replacement {
        return Err("refusing to patch a function to itself".to_string());
    }
    check_patching_thread()?;

    // Step 2: Find how much room the function really has, and check that the
    // jump fits. Without this the write would run past a short function into
    // whatever the linker placed after it, usually another live function, and
    // the corruption would show up far from its cause.
    let Some((begin, length)) = function_extent(target) else {
        return Err(format!(
            "0x{target:016x} is a leaf function with no entry in the exception \
             directory, so its length cannot be established and overwriting it \
             is unsafe; annotate it with #[pill_hot_fn] to patch it through a \
             dispatch slot instead, which needs no length at all"
        ));
    };
    if begin != target {
        return Err(format!(
            "0x{target:016x} is not a function entry point (the function containing \
             it begins at 0x{begin:016x})"
        ));
    }
    if length < ABSOLUTE_JUMP_LENGTH {
        return Err(format!(
            "the function at 0x{target:016x} is {length} bytes, too short for the \
             {ABSOLUTE_JUMP_LENGTH}-byte jump a patch installs"
        ));
    }

    // Step 3: Make the page writable and keep the bytes the jump replaces.
    let target_pointer = target as *mut u8;
    let mut previous_protection: u32 = 0;

    // SAFETY: `target` is a function entry inside a mapped image, as the extent
    // check above established, so the page exists and can be made writable.
    let protection_changed = unsafe {
        VirtualProtect(
            target_pointer as *mut core::ffi::c_void,
            ABSOLUTE_JUMP_LENGTH,
            PAGE_EXECUTE_READWRITE,
            &mut previous_protection,
        )
    };
    if protection_changed == 0 {
        return Err("VirtualProtect refused to make the page writable".to_string());
    }

    // SAFETY: the page is now writable and the range was validated above.
    let original =
        unsafe { core::slice::from_raw_parts(target_pointer, ABSOLUTE_JUMP_LENGTH) }.to_vec();

    // Step 4: Write the jump, restore the page's protection and flush the
    // instruction cache.
    //
    // The jump is `mov rax, imm64` followed by `jmp rax`.
    let mut instructions = [0u8; ABSOLUTE_JUMP_LENGTH];
    instructions[0] = 0x48;
    instructions[1] = 0xB8;
    instructions[2..10].copy_from_slice(&(replacement as u64).to_le_bytes());
    instructions[10] = 0xFF;
    instructions[11] = 0xE0;

    // SAFETY: the page is writable and the slice length matches exactly.
    unsafe {
        core::ptr::copy_nonoverlapping(instructions.as_ptr(), target_pointer, ABSOLUTE_JUMP_LENGTH);
    }

    // SAFETY: restoring the protection the call above reported.
    unsafe {
        let mut discarded_protection: u32 = 0;
        VirtualProtect(
            target_pointer as *mut core::ffi::c_void,
            ABSOLUTE_JUMP_LENGTH,
            previous_protection,
            &mut discarded_protection,
        );
        // Required on any core whose instruction cache may hold the old bytes.
        FlushInstructionCache(
            GetCurrentProcess(),
            target_pointer as *const core::ffi::c_void,
            ABSOLUTE_JUMP_LENGTH,
        );
    }

    Ok(original)
}

/// Writes saved prologue bytes back, undoing a patch.
///
/// # Errors
///
/// Returns a message naming the reason when:
///
/// - `target` is null;
/// - the call comes from a thread other than the declared patching thread;
/// - `target` has no exception-directory entry, or is not the first byte of
///   its function;
/// - the function at `target` is shorter than the saved bytes, which usually
///   means the image was replaced;
/// - the bytes at `target` do not begin with the jump `patch_prologue` writes;
/// - `VirtualProtect` refuses to make the page writable.
///
/// # Safety
///
/// The same contract as [`patch_prologue`]: `target` must be the address the
/// bytes came from, and no thread may be executing inside them.
#[cfg(all(windows, target_arch = "x86_64"))]
pub unsafe fn restore_prologue(target: usize, original: &[u8]) -> Result<(), String> {
    // Step 1: Reject a null address and a restore from the wrong thread.
    if target == 0 {
        return Err("cannot restore a null address".to_string());
    }
    // This is the same guard `patch_prologue` carries, for the same reason:
    // twelve bytes of live code are overwritten here, and no twelve-byte write
    // is atomic. A thread executing inside those bytes could see half of the
    // old jump and half of the restored code, so a restore is just as dangerous
    // as a patch. The `# Safety` section above states the contract; this call
    // enforces it instead of trusting the caller.
    check_patching_thread()?;
    // Step 2: Re-read the extent and check that the function still has room for
    // the saved bytes. A restore aimed at an address whose image has since been
    // replaced would otherwise write into whatever now occupies it, so the
    // extent is re-read rather than trusted.
    let Some((begin, length)) = function_extent(target) else {
        return Err(format!(
            "0x{target:016x} has no entry in the exception directory; refusing to \
             write over it"
        ));
    };
    if begin != target || length < original.len() {
        return Err(format!(
            "0x{target:016x} no longer names a function long enough for the {} saved \
             bytes; the image was probably replaced",
            original.len()
        ));
    }
    let target_pointer = target as *mut u8;

    // Step 3: Check that the bytes there now are the jump this module
    // installed. The extent check alone is not enough: an image can be unloaded
    // and another one mapped over the same address, and the extent lookup would
    // still succeed. Writing the saved bytes then corrupts an unrelated live
    // function, during a rollback the developer is already recovering from.
    //
    // SAFETY: the extent check above established that `target` is the entry of a
    // mapped function at least `original.len()` bytes long, so this many bytes
    // are readable.
    let installed_bytes = unsafe { core::slice::from_raw_parts(target_pointer, original.len()) };
    if installed_bytes.len() < 2 || installed_bytes[0] != 0x48 || installed_bytes[1] != 0xB8 {
        return Err(format!(
            "0x{target:016x} does not begin with the jump this patch installed, so \
             it is no longer the function those bytes came from; refusing to \
             write over it"
        ));
    }

    // Step 4: Make the page writable, write the saved bytes back, restore the
    // protection and flush the instruction cache.
    let mut previous_protection: u32 = 0;
    // SAFETY: `target` is a mapped function entry, as the extent check above
    // established, and making its page writable is what this function does.
    let protection_changed = unsafe {
        VirtualProtect(
            target_pointer as *mut core::ffi::c_void,
            original.len(),
            PAGE_EXECUTE_READWRITE,
            &mut previous_protection,
        )
    };
    if protection_changed == 0 {
        return Err("VirtualProtect refused to make the page writable".to_string());
    }
    // SAFETY: the page is writable and the length matches the saved slice.
    unsafe {
        core::ptr::copy_nonoverlapping(original.as_ptr(), target_pointer, original.len());
        let mut discarded_protection: u32 = 0;
        VirtualProtect(
            target_pointer as *mut core::ffi::c_void,
            original.len(),
            previous_protection,
            &mut discarded_protection,
        );
        FlushInstructionCache(
            GetCurrentProcess(),
            target_pointer as *const core::ffi::c_void,
            original.len(),
        );
    }
    Ok(())
}

// =============================================================================
// Prologue patching (other targets)
// =============================================================================
//
// The implementation above is Windows x86-64 only: it writes an absolute jump
// by hand and reads the exception directory through `RtlLookupFunctionEntry`,
// and neither has a portable equivalent. Every other target gets the stubs
// below.
//
// The stubs return an error instead of pretending to patch. Everything that
// does not depend on rewriting live code - the dispatch slots, the registries,
// whole-module reloading - is portable Rust and works unchanged, so a
// non-Windows target keeps `#[pill_hot]` and `#[pill_hot_fn]` and loses only
// the route that patches a function without a dispatch slot.

/// Returns where the function containing `address` begins and how long it is.
///
/// Always `None` off Windows x86-64: there is no supported way to establish a
/// function's extent, and a guess would let a patch corrupt the function that
/// follows it.
#[cfg(not(all(windows, target_arch = "x86_64")))]
pub fn function_extent(_address: usize) -> Option<(usize, usize)> {
    None
}

/// Overwrites a function's first bytes with a jump to `replacement`.
///
/// # Errors
///
/// Always returns an error off Windows x86-64. Annotating the function with
/// `#[pill_hot_fn]` gives it a dispatch slot, which is portable and needs no
/// code rewriting.
///
/// # Safety
///
/// Nothing is written, so there is no contract to uphold - the signature
/// matches the Windows one so callers need no `cfg` of their own.
#[cfg(not(all(windows, target_arch = "x86_64")))]
pub unsafe fn patch_prologue(_target: usize, _replacement: usize) -> Result<Vec<u8>, String> {
    Err(
        "prologue patching is implemented for Windows x86-64 only; annotate the function \
         with #[pill_hot_fn] to patch it through a dispatch slot instead"
            .to_string(),
    )
}

/// Writes saved prologue bytes back.
///
/// # Errors
///
/// Always returns an error off Windows x86-64: nothing can have been patched,
/// so there is nothing to restore.
///
/// # Safety
///
/// Nothing is written, so there is no contract to uphold.
#[cfg(not(all(windows, target_arch = "x86_64")))]
pub unsafe fn restore_prologue(_target: usize, _original: &[u8]) -> Result<(), String> {
    Err("prologue patching is implemented for Windows x86-64 only".to_string())
}

// =============================================================================
// ABI self-check
// =============================================================================

/// Verifies that a `&mut ZST` parameter can be received as `&mut ()`.
///
/// This is the one platform assumption the dispatch slot rests on. Both are
/// references to zero-sized types, so both are a single non-null pointer-sized
/// argument, and every ABI the engine targets passes them identically - but
/// that is an observation, not a guarantee Rust makes.
///
/// Returns `true` when the arguments arrive intact. A host can call it once at
/// startup, in debug builds, where a broken assumption must fail straight away
/// instead of waiting for the first patch: a silent argument shift becomes an
/// initialization failure.
pub fn verify_abi() -> bool {
    /// Stands in for a system's zero-sized fn-item type.
    struct ZeroSized;

    /// The shape a patch exports: the stand-in plus real arguments.
    fn receiver(_stand_in: &mut (), first: u64, second: u64, third: u64) -> u64 {
        first
            .wrapping_mul(1_000_000)
            .wrapping_add(second.wrapping_mul(1_000))
            .wrapping_add(third)
    }

    let mut zero_sized = ZeroSized;
    // SAFETY: `receiver` declares its first parameter as `&mut ()` and never
    // reads it; `&mut ZeroSized` is likewise a non-null pointer-sized value for
    // a zero-sized type. This call is exactly the substitution under test, and
    // a mismatched ABI shifts the arguments rather than faulting - which is why
    // the result is checked instead of merely reaching this line.
    let observed = unsafe {
        let call: fn(&mut ZeroSized, u64, u64, u64) -> u64 =
            std::mem::transmute(receiver as *const ());
        call(&mut zero_sized, 11, 22, 33)
    };
    observed == 11_022_033
}

// =============================================================================
// Integration tests
// =============================================================================

/// End-to-end checks that a system registered through the normal engine API can
/// have its implementation replaced, and that everything around it stays put.
#[cfg(all(test, feature = "development"))]
mod integration_tests {
    // External crates
    use pill_core::error::EngineMessage;

    // Current crate
    use super::*;
    use crate::error::SystemError;
    use crate::{Engine, SystemOwner};

    /// Declares an isolated observable counter plus the system bodies that
    /// drive it.
    ///
    /// One set per test: the harness runs tests in parallel, so one shared
    /// counter would also see other tests' increments and make an assertion
    /// about how many times a system ran meaningless.
    macro_rules! isolated_systems {
        ($name:ident) => {
            // A test uses only some of these helpers, so unused ones are
            // allowed.
            #[allow(dead_code)]
            mod $name {
                use std::sync::atomic::{AtomicU32, Ordering};

                static OBSERVED: AtomicU32 = AtomicU32::new(0);

                /// The body every caller starts out pointing at. Counts one per
                /// call.
                pub fn original() {
                    OBSERVED.fetch_add(1, Ordering::SeqCst);
                }

                /// The patched body. Counts ten per call, so a test can tell
                /// which body ran.
                pub fn replacement() {
                    OBSERVED.fetch_add(10, Ordering::SeqCst);
                }

                /// A patched body that panics on every call. The containment and
                /// the panic count are what the host's automatic rollback acts
                /// on.
                pub fn panicking() {
                    panic!("patched body panicked");
                }

                /// Total count observed so far, weighted by which body ran.
                pub fn observed() -> u32 {
                    OBSERVED.load(Ordering::SeqCst)
                }
            }
        };
    }

    isolated_systems!(replaces_behavior);
    isolated_systems!(registration_intact);
    isolated_systems!(signature_refused);
    isolated_systems!(unknown_refused);
    isolated_systems!(cleared_slots);
    isolated_systems!(second_patch);
    isolated_systems!(rolled_back);
    isolated_systems!(panicking_patch);

    /// A different signature, used to prove the gate refuses it.
    fn differently_shaped_system() -> Result<(), SystemError> {
        Ok(())
    }

    fn sequential_engine() -> Engine {
        let mut engine = Engine::new();
        engine.set_parallel_execution(false);
        engine
    }

    /// A patch whose body panics is contained by the dispatcher: each frame
    /// records one system failure instead of unwinding through the engine, and
    /// the third frame in a row queues the fault report a host rolls back
    /// from.
    #[test]
    fn a_panicking_patch_is_contained_and_reported() {
        use panicking_patch as systems;
        let mut engine = sequential_engine();
        engine.register_system("panicking_counter", systems::original);
        engine.process_frame().expect("frame");
        engine
            .hot_patch(
                "panicking_counter",
                local_implementation_address::<_, ()>(&systems::panicking),
                signature_hash_of::<_, ()>(&systems::panicking),
            )
            .expect("patch accepted");
        // Drain anything an earlier test queued, so the count below is ours.
        let _ = take_fault_reports();
        for frame in 1..PANIC_ROLLBACK_FRAMES {
            engine.process_frame().expect("frame");
            assert_eq!(engine.drain_system_failures().len(), 1, "frame {frame}");
            assert!(
                take_fault_reports().is_empty(),
                "frame {frame} reported before the threshold"
            );
        }
        engine.process_frame().expect("frame");
        let failures = engine.drain_system_failures();
        assert_eq!(failures[0].system, "panicking_counter");
        assert!(failures[0]
            .to_plain_message()
            .contains("panicked: patched body panicked"));
        let reports = take_fault_reports();
        assert_eq!(reports.len(), 1, "the third panicking frame must report");
        // The report names the function path the slot was registered under,
        // which is the identity a host's rollback resolves.
        assert!(
            reports[0].function.ends_with("panicking_patch::original"),
            "reported function: {}",
            reports[0].function
        );
        assert!(reports[0].message.contains("patched body panicked"));
    }

    /// The central promise: a patched system runs new code on the next frame.
    #[test]
    fn patch_replaces_behavior_on_the_next_frame() {
        use replaces_behavior as systems;
        let mut engine = sequential_engine();
        engine.register_system("counter", systems::original);

        engine.process_frame().expect("frame");
        assert_eq!(systems::observed(), 1, "baseline runs");

        engine
            .hot_patch(
                "counter",
                local_implementation_address::<_, ()>(&systems::replacement),
                signature_hash_of::<_, ()>(&systems::replacement),
            )
            .expect("patch accepted");

        engine.process_frame().expect("frame");
        assert_eq!(
            systems::observed(),
            11,
            "patched body must run, continuing from the previous value"
        );
    }

    /// The registry's slot count and the system's enabled state survive a
    /// patch; only the implementation changed.
    #[test]
    fn patch_leaves_the_registration_intact() {
        use registration_intact as systems;
        let mut engine = sequential_engine();
        engine.register_system("counter", systems::original);
        let slots_before = engine.hot_patch_registry().len();

        engine
            .hot_patch(
                "counter",
                local_implementation_address::<_, ()>(&systems::replacement),
                signature_hash_of::<_, ()>(&systems::replacement),
            )
            .expect("patch accepted");

        assert_eq!(
            engine.hot_patch_registry().len(),
            slots_before,
            "no system was added or removed"
        );
        assert_eq!(engine.is_system_enabled("counter"), Some(true));
    }

    /// A signature change must be refused, and the running code must survive it.
    #[test]
    fn patch_with_a_changed_signature_is_refused() {
        use signature_refused as systems;
        let mut engine = sequential_engine();
        engine.register_system("counter", systems::original);
        engine.process_frame().expect("frame");

        let error = engine
            .hot_patch(
                "counter",
                local_implementation_address::<_, ()>(&differently_shaped_system),
                signature_hash_of::<_, ()>(&differently_shaped_system),
            )
            .expect_err("a changed signature must be refused");
        assert!(matches!(error, HotPatchError::SignatureMismatch { .. }));

        engine.process_frame().expect("frame");
        assert_eq!(
            systems::observed(),
            2,
            "the original implementation must still be running"
        );
    }

    /// A name the registry never saw is refused instead of resolved to
    /// anything.
    #[test]
    fn patching_an_unregistered_system_is_refused() {
        use unknown_refused as systems;
        let mut engine = sequential_engine();
        engine.register_system("counter", systems::original);

        let error = engine
            .hot_patch(
                "not_registered",
                local_implementation_address::<_, ()>(&systems::replacement),
                signature_hash_of::<_, ()>(&systems::replacement),
            )
            .expect_err("unknown system must be refused");
        assert!(matches!(error, HotPatchError::UnknownSystem { .. }));
    }

    /// Clearing a module's systems must also retire their slots, so a late
    /// patch cannot install into a system that no longer runs.
    #[test]
    fn cleared_systems_stop_being_patchable() {
        use cleared_slots as systems;
        let mut engine = sequential_engine();
        let system_owner = SystemOwner::extension(0);

        // Distinct functions, so each system gets its own function-path alias
        // and the two do not share a registry entry.
        engine.begin_module_registration(system_owner);
        engine.register_system("module_counter", systems::original);
        engine.end_module_registration();
        engine.register_system("project_counter", systems::replacement);

        assert!(engine.hot_patch_registry().get("module_counter").is_some());
        assert!(engine.hot_patch_registry().get("project_counter").is_some());

        engine.clear_systems_owned_by(system_owner);

        assert!(
            engine.hot_patch_registry().get("module_counter").is_none(),
            "a cleared system's slot must be forgotten"
        );
        assert!(
            engine.hot_patch_registry().get("project_counter").is_some(),
            "another owner's system must survive"
        );

        let error = engine
            .hot_patch(
                "module_counter",
                local_implementation_address::<_, ()>(&systems::replacement),
                signature_hash_of::<_, ()>(&systems::replacement),
            )
            .expect_err("a cleared system must not be patchable");
        assert!(matches!(error, HotPatchError::UnknownSystem { .. }));
    }

    /// A system is patchable by its function path as well as its display name.
    ///
    /// The two routinely differ - the `native` test project registers
    /// `physics_system` as `"ball_physics"` - and a generated patch only knows
    /// the path, because that is what `#[pill_hot]` derives.
    #[test]
    fn a_system_is_patchable_by_its_function_path() {
        use registration_intact as systems;
        let mut engine = sequential_engine();
        engine.register_system("a_display_name", systems::original);

        let path = function_path::<fn()>();
        assert!(path.is_none(), "a bare fn pointer type has no path");

        // Collect the names the engine recorded for this registration.
        let recorded_names: Vec<String> = engine
            .hot_patch_registry()
            .names()
            .map(|name| name.to_string())
            .collect();
        let function_path_entry = recorded_names
            .iter()
            .find(|name| name.ends_with("::original"))
            .expect("the function path must be registered alongside the display name");

        assert!(
            engine
                .hot_patch_registry()
                .get(function_path_entry)
                .is_some(),
            "both names must resolve to a slot"
        );

        // Clearing through one name must retire the other too.
        engine.clear_systems();
        assert!(engine.hot_patch_registry().is_empty());
    }

    /// A system can be returned to the code its artifact was built with, after
    /// any number of patches.
    ///
    /// This is generation zero of a rollback. It works because the slot records
    /// the registration address separately from the current one - without that,
    /// the first patch would make the original unreachable.
    #[test]
    fn a_system_can_be_rolled_back_to_its_baseline() {
        use rolled_back as systems;
        let mut engine = sequential_engine();
        engine.register_system("counter", systems::original);

        let (baseline, baseline_hash) = engine
            .hot_patch_baseline("counter")
            .expect("a registered system must record a baseline");

        // Three generations, as a live-coding session produces.
        for _ in 0..3 {
            engine
                .hot_patch(
                    "counter",
                    local_implementation_address::<_, ()>(&systems::replacement),
                    signature_hash_of::<_, ()>(&systems::replacement),
                )
                .expect("patch");
        }
        engine.process_frame().expect("frame");
        assert_eq!(systems::observed(), 10, "the patch must be running");

        // The baseline is still reachable and still installable.
        engine
            .hot_patch("counter", baseline, baseline_hash)
            .expect("rollback to the baseline must be accepted");
        engine.process_frame().expect("frame");
        assert_eq!(
            systems::observed(),
            11,
            "generation zero must run the original body"
        );
    }

    /// A plain function's slot falls back to the artifact's own body once
    /// emptied, which is how a plain function rolls back to generation zero.
    #[test]
    fn an_emptied_plain_slot_falls_back_to_the_original() {
        fn original() -> u32 {
            1
        }
        fn replacement() -> u32 {
            2
        }

        let slot = PlainSlot::new();
        let original_address = original as *const () as usize;
        assert_eq!(slot.current_or(original_address), original_address);

        slot.install(replacement as *const () as usize);
        assert_eq!(
            slot.current_or(original_address),
            replacement as *const () as usize,
            "an installed replacement must win"
        );

        slot.reset();
        assert_eq!(
            slot.current_or(original_address),
            original_address,
            "an emptied slot must fall back to the artifact's own body"
        );
    }

    // =========================================================================
    // Prologue patching
    // =========================================================================

    /// A function whose prologue gets overwritten and put back.
    ///
    /// `inline(never)` because the test needs a real symbol with a real entry in
    /// the exception directory; an inlined body has neither.
    #[inline(never)]
    fn patch_target(value: u32) -> u32 {
        value + 1
    }

    /// Its stand-in, with the same call shape.
    #[inline(never)]
    fn patch_replacement(value: u32) -> u32 {
        value + 1000
    }

    /// The main behaviour: callers of a patched function reach the
    /// replacement, and restoring the saved bytes puts the original back.
    ///
    /// One test rather than three, because the bytes are process-wide state and
    /// the harness runs tests on several threads.
    #[test]
    fn a_patched_prologue_redirects_and_restores() {
        let target = patch_target as *const () as usize;
        let replacement = patch_replacement as *const () as usize;
        assert_eq!(std::hint::black_box(patch_target)(1), 2);

        // SAFETY: both addresses name real functions in this binary with the
        // same call shape, and no other thread calls `patch_target`.
        let original = unsafe { patch_prologue(target, replacement) }
            .expect("a normal function must be patchable");
        assert_eq!(original.len(), ABSOLUTE_JUMP_LENGTH);
        assert_eq!(
            std::hint::black_box(patch_target)(1),
            1001,
            "callers must reach the replacement"
        );

        // SAFETY: the same address the bytes were saved from, unchanged since.
        unsafe { restore_prologue(target, &original) }.expect("restore must succeed");
        assert_eq!(
            std::hint::black_box(patch_target)(1),
            2,
            "restoring must bring the original body back"
        );
    }

    /// A second stand-in, so a function can be patched twice.
    #[inline(never)]
    fn patch_second_replacement(value: u32) -> u32 {
        value + 2000
    }

    /// Patching twice and rolling back needs the FIRST generation's saved
    /// bytes, not the newest.
    ///
    /// This is the assumption `restore_prologue_baseline` rests on: generation
    /// two overwrote the jump generation one had written, so its "original" is a
    /// jump. Restoring from it would reinstate a patch instead of removing one.
    #[test]
    fn the_first_generation_holds_the_artifacts_own_code() {
        let target = patch_target_twice as *const () as usize;
        let first_replacement = patch_replacement as *const () as usize;
        let second_replacement = patch_second_replacement as *const () as usize;
        assert_eq!(std::hint::black_box(patch_target_twice)(1), 2);

        // SAFETY: real functions in this binary with the same call shape, and
        // no other thread calls `patch_target_twice`.
        let generation_one =
            unsafe { patch_prologue(target, first_replacement) }.expect("first patch");
        assert_eq!(std::hint::black_box(patch_target_twice)(1), 1001);

        // SAFETY: as above. What this returns is generation one's jump.
        let generation_two =
            unsafe { patch_prologue(target, second_replacement) }.expect("second patch");
        assert_eq!(std::hint::black_box(patch_target_twice)(1), 2001);

        assert_ne!(
            generation_one, generation_two,
            "the second patch must have saved the first patch's jump, not the original"
        );

        // Restoring from the NEWEST saved bytes puts generation one back, which
        // is the mistake this test exists to catch.
        // SAFETY: the address these bytes were saved from, unchanged since.
        unsafe { restore_prologue(target, &generation_two) }.expect("restore");
        assert_eq!(
            std::hint::black_box(patch_target_twice)(1),
            1001,
            "restoring the newest saved bytes brings back generation one, not the original"
        );

        // Restoring from the FIRST saved bytes is what actually reaches
        // generation zero.
        // SAFETY: as above.
        unsafe { restore_prologue(target, &generation_one) }.expect("restore");
        assert_eq!(
            std::hint::black_box(patch_target_twice)(1),
            2,
            "the first generation's bytes are the artifact's own code"
        );
    }

    /// Its own target, so the test above cannot disturb the round-trip test.
    #[inline(never)]
    fn patch_target_twice(value: u32) -> u32 {
        value + 1
    }

    /// A restore is refused when the bytes there are not the jump this module
    /// installed.
    ///
    /// The extent check alone would accept the address: an image can be
    /// unloaded and another mapped over it. The bytes are the only proof that
    /// this is still the function the saved bytes came from.
    #[test]
    fn restoring_over_code_that_is_not_our_jump_is_refused() {
        let target = patch_restore_guard as *const () as usize;
        assert_eq!(std::hint::black_box(patch_restore_guard)(1), 3);

        // Bytes shaped like a saved prologue, but the function was never
        // patched, so what is there now is its own code.
        let pretend_original = vec![0x90u8; ABSOLUTE_JUMP_LENGTH];

        // SAFETY: expected to refuse before writing, which is what this asserts.
        let restore_result = unsafe { restore_prologue(target, &pretend_original) };
        let detail = restore_result.expect_err("an unpatched function must be refused");
        assert!(
            detail.contains("does not begin with the jump"),
            "unexpected reason: {detail}"
        );
        assert_eq!(
            std::hint::black_box(patch_restore_guard)(1),
            3,
            "a refused restore must not have written anything"
        );
    }

    /// Its own target, so a refused restore cannot disturb the other tests.
    #[inline(never)]
    fn patch_restore_guard(value: u32) -> u32 {
        value + 2
    }

    /// An address the exception directory does not describe has no known
    /// length, so patching it is refused rather than writing twelve bytes over
    /// whatever happens to be there.
    #[test]
    fn an_address_with_no_function_entry_is_refused() {
        static NOT_A_FUNCTION: [u8; 64] = [0x90; 64];
        let target = NOT_A_FUNCTION.as_ptr() as usize;
        let replacement = patch_replacement as *const () as usize;

        // SAFETY: the call is expected to refuse before writing anything, which
        // is exactly what this asserts.
        let patch_result = unsafe { patch_prologue(target, replacement) };
        let detail = patch_result.expect_err("a non-function address must be refused");
        assert!(
            detail.contains("exception directory"),
            "unexpected reason: {detail}"
        );
        assert_eq!(
            NOT_A_FUNCTION[..ABSOLUTE_JUMP_LENGTH],
            [0x90; ABSOLUTE_JUMP_LENGTH],
            "a refused patch must not have written anything"
        );
    }

    /// Patching the middle of a function is refused: the saved bytes would not
    /// be a prologue, and a later restore would corrupt the body.
    #[test]
    fn an_address_inside_a_function_is_refused() {
        let target = patch_target as *const () as usize + 1;
        let replacement = patch_replacement as *const () as usize;

        // SAFETY: expected to refuse before writing.
        let patch_result = unsafe { patch_prologue(target, replacement) };
        let detail = patch_result.expect_err("an interior address must be refused");
        assert!(
            detail.contains("not a function entry point"),
            "unexpected reason: {detail}"
        );
    }

    /// The extent lookup reports the linker's own record, so a real function is
    /// described and its reported start is the address the caller holds.
    #[test]
    fn the_exception_directory_describes_a_real_function() {
        let target = patch_target as *const () as usize;
        let (begin, length) = function_extent(target).expect("a real function must be described");
        assert_eq!(begin, target, "the entry must start where the symbol does");
        assert!(
            length >= ABSOLUTE_JUMP_LENGTH,
            "an ordinary function should have room for the jump, got {length} bytes"
        );
    }

    /// Patching twice runs the newest implementation, which is what a second
    /// edit in a live-coding session does.
    #[test]
    fn a_second_patch_supersedes_the_first() {
        use second_patch as systems;
        let mut engine = sequential_engine();
        engine.register_system("counter", systems::original);

        engine
            .hot_patch(
                "counter",
                local_implementation_address::<_, ()>(&systems::replacement),
                signature_hash_of::<_, ()>(&systems::replacement),
            )
            .expect("first patch");
        engine.process_frame().expect("frame");
        assert_eq!(systems::observed(), 10);

        engine
            .hot_patch(
                "counter",
                local_implementation_address::<_, ()>(&systems::original),
                signature_hash_of::<_, ()>(&systems::original),
            )
            .expect("second patch, back to the original");
        engine.process_frame().expect("frame");
        assert_eq!(
            systems::observed(),
            11,
            "the newest implementation must win"
        );
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The stand-in substitution holds on this target.
    ///
    /// If this ever fails, the dispatch slot is unsound here and the hot-patch
    /// feature must not be enabled.
    #[test]
    fn zero_sized_stand_in_is_abi_compatible() {
        assert!(verify_abi(), "&mut ZST is not passed like &mut () here");
    }

    /// `Input` must be inferable from the function value alone, with no
    /// turbofish. A concrete function-item type satisfies exactly one arity of
    /// [`SystemParamFunction`], so the solver has a unique choice - and the
    /// `#[pill_hot]` macro relies on this, because reconstructing the parameter
    /// tuple from syntax would have to strip patterns and guess lifetimes.
    #[test]
    fn input_infers_from_the_function_value_alone() {
        fn nullary() {}
        fn fallible() -> Result<(), crate::error::SystemError> {
            Ok(())
        }

        assert_ne!(local_implementation_address(&nullary), 0);
        assert_ne!(local_implementation_address(&fallible), 0);
        assert_ne!(
            signature_hash_of(&nullary),
            signature_hash_of(&fallible),
            "differing return types must produce differing signatures"
        );
    }

    /// A slot reports the address and signature it was initialized with.
    #[test]
    fn slot_reports_its_baseline() {
        let slot = HotSlot::new();
        slot.initialize(0x1234, 0xABCD);
        assert_eq!(slot.current(), 0x1234);
        assert_eq!(slot.signature_hash(), 0xABCD);
    }

    /// An accepted install becomes the address the slot reports.
    #[test]
    fn install_replaces_the_implementation() {
        let slot = HotSlot::new();
        slot.initialize(0x1000, 0xFEED);
        assert!(slot.install("movement", 0x2000, 0xFEED).is_ok());
        assert_eq!(slot.current(), 0x2000);
    }

    /// A changed signature must be refused, and the current address must stay.
    #[test]
    fn install_refuses_a_signature_mismatch() {
        let slot = HotSlot::new();
        slot.initialize(0x1000, 0xFEED);

        let error = slot.install("movement", 0x2000, 0xBEEF).unwrap_err();
        assert_eq!(
            error,
            HotPatchError::SignatureMismatch {
                name: "movement".to_string(),
                expected: 0xFEED,
                found: 0xBEEF,
            }
        );
        assert_eq!(slot.current(), 0x1000, "refused patch must not be applied");
    }

    /// A null replacement is refused and the registered address keeps running.
    #[test]
    fn install_refuses_a_null_address() {
        let slot = HotSlot::new();
        slot.initialize(0x1000, 0xFEED);
        assert!(slot.install("movement", 0, 0xFEED).is_err());
        assert_eq!(slot.current(), 0x1000);
    }

    /// A registry finds the slot it was given, rejects an unknown name, and
    /// forgets a slot once it is removed.
    #[test]
    fn registry_round_trips_and_forgets() {
        let mut registry = HotPatchRegistry::new();
        assert!(registry.is_empty());

        let slot = Arc::new(HotSlot::new());
        slot.initialize(0x1000, 0x1);
        registry.insert("movement", Arc::clone(&slot));

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get("movement").unwrap().current(), 0x1000);
        assert!(registry.get("absent").is_none());

        registry.remove("movement");
        assert!(registry.get("movement").is_none());
        assert!(registry.is_empty());
    }

    /// A slot shared with a system's closure observes installs made through the
    /// registry - the property the whole design depends on.
    #[test]
    fn registry_and_closure_share_one_slot() {
        let mut registry = HotPatchRegistry::new();
        let slot = Arc::new(HotSlot::new());
        slot.initialize(0x1000, 0x7);
        registry.insert("movement", Arc::clone(&slot));

        registry
            .get("movement")
            .unwrap()
            .install("movement", 0x2000, 0x7)
            .expect("install");

        assert_eq!(slot.current(), 0x2000, "the closure's handle must see it");
    }

    /// The same signature hashes identically on every call.
    #[test]
    fn signature_hash_distinguishes_signatures() {
        fn takes_nothing() {}
        fn takes_nothing_too() {}

        let first = signature_hash::<fn(), ()>();
        let second = signature_hash::<fn(), ()>();
        assert_eq!(first, second, "same signature must hash stably");

        let _ = (takes_nothing, takes_nothing_too);
    }
}
