//! Parallel dispatch for managed systems: the host's half of
//! `query.ForEachParallel`.
//!
//! # Responsibilities
//!
//! - Run a managed callback for every index in `0..count` across the shared
//!   [`pill_core::rayon`] pool, joining before returning.
//! - Refuse calls made without an active managed invocation, or made from
//!   inside a parallel callback.
//!
//! # Design
//!
//! A scheduled managed system makes this call on the frame thread. Rayon
//! workers run the callbacks, and the calling thread joins in, so it may run
//! a work item itself. Workers carry no managed scope, so an engine entry
//! point called from a worker refuses the call on its own. The checks here
//! cover only the calling thread's scope and the nesting case, and they run
//! before any dispatch so the status is the same whichever thread Rayon picks.

// Standard library
use std::cell::Cell;
use std::ffi::c_void;

// External crates
use pill_core::rayon::prelude::*;

// Current crate
use crate::context::{active_scope_token, guard_managed_callback};

// =============================================================================
// Constants
// =============================================================================

/// Every index ran.
pub(super) const PARALLEL_OK: u8 = 0;
/// No managed system is scheduled on the calling thread.
pub(super) const PARALLEL_NO_SCOPE: u8 = 3;
/// The call came from inside a parallel callback.
pub(super) const PARALLEL_NESTED: u8 = 4;
/// A panic escaped the dispatch; `guard_managed_callback` returns this status.
const PARALLEL_INTERNAL_FAILURE: u8 = 5;

thread_local! {
    /// Whether this thread is inside a parallel callback right now.
    ///
    /// The flag is set before each callback runs and restored after it, on
    /// whichever thread runs the callback. A nested dispatch is therefore
    /// refused the same way on every thread, including the calling thread,
    /// which already has a managed scope.
    static IN_PARALLEL_CALLBACK: Cell<bool> = const { Cell::new(false) };
}

// =============================================================================
// Types + Impls
// =============================================================================

/// The managed trampoline's signature: the `[UnmanagedCallersOnly]` static
/// invoked once per work item.
pub(super) type ParallelCallback = extern "C" fn(*mut c_void, u32);

/// The managed caller's state pointer, wrapped so it can be sent to Rayon
/// workers.
#[derive(Clone, Copy)]
struct ManagedStatePointer(*mut c_void);

impl ManagedStatePointer {
    /// Returns the wrapped pointer, which is passed straight back to the
    /// managed callback.
    ///
    /// The caller reads it through this method on purpose. An edition-2021
    /// closure that reads `state.0` directly captures only the raw pointer,
    /// which is neither `Send` nor `Sync`. A method call captures the whole
    /// wrapper, which is both.
    fn pointer(self) -> *mut c_void {
        self.0
    }
}

// SAFETY: the pointer belongs to the managed caller, which keeps its target
// alive until the dispatch returns. The Rust side only passes the pointer
// value back to the callback and never dereferences it; the managed side
// resolves it through its own handle.
unsafe impl Send for ManagedStatePointer {}
// SAFETY: shared access only reads the pointer value. The Rust side never
// dereferences it, and the managed caller's GCHandle keeps the target alive
// through the join.
unsafe impl Sync for ManagedStatePointer {}

// =============================================================================
// Free Functions
// =============================================================================

/// Runs `run(index)` for every index in `0..count` on the shared pool and
/// returns after every invocation has finished.
///
/// Returns [`PARALLEL_OK`] when every index ran, [`PARALLEL_NO_SCOPE`] when
/// no managed system is active on the calling thread, or [`PARALLEL_NESTED`]
/// when the call came from inside a parallel callback.
pub(super) fn parallel_for(count: u32, run: impl Fn(u32) + Send + Sync) -> u8 {
    // Step 1: Refuse a nested call and a call with no managed scope before
    // any work starts. The flag is set on whichever thread runs a callback,
    // so a nested call is refused on the calling thread too.
    if IN_PARALLEL_CALLBACK.with(|flag| flag.get()) {
        return PARALLEL_NESTED;
    }
    if active_scope_token() == 0 {
        return PARALLEL_NO_SCOPE;
    }
    if count == 0 {
        return PARALLEL_OK;
    }

    // Step 2: Run the callbacks on the pool. `for_each` returns only when
    // every callback has finished, which is what the managed side depends
    // on: its world borrows, its scope teardown and unloading the assembly
    // all assume no callback is still running.
    (0..count).into_par_iter().for_each(|index| {
        with_callback_flag(|| run(index));
    });
    PARALLEL_OK
}

/// Runs `body` with `IN_PARALLEL_CALLBACK` set on this thread, then restores
/// the flag's previous value.
fn with_callback_flag<R>(body: impl FnOnce() -> R) -> R {
    IN_PARALLEL_CALLBACK.with(|flag| {
        let previous = flag.replace(true);
        let body_result = body();
        flag.set(previous);
        body_result
    })
}

/// Runs the managed trampoline for every index in `0..count`. This is the
/// entry the managed runtime calls through the API table.
///
/// `state` is opaque: it is passed back to `callback` unchanged.
pub(super) extern "C" fn ffi_parallel_for(
    callback: ParallelCallback,
    state: *mut c_void,
    count: u32,
) -> u8 {
    guard_managed_callback("ffi_parallel_for", PARALLEL_INTERNAL_FAILURE, || {
        let state = ManagedStatePointer(state);
        parallel_for(count, move |index| callback(state.pointer(), index))
    })
}
