//! Per-thread change ticks for the system running on the current thread.
//!
//! # Responsibilities
//!
//! - Hold, for each thread, the tick its system last ran at and the tick
//!   reserved for the current run. The engine installs both before it starts
//!   a system in a parallel batch.
//! - Let every DLL in the process read the same values.
//!
//! # Design
//!
//! The engine writes these values and the system's own code reads them.
//! `Query` is generic, so a tick read compiles into the project or extension
//! DLL. A thread-local defined in an rlib would then exist once per DLL, and
//! a module's system would never see the value the engine installed. These
//! values live in `pill_core`, which is loaded once per process, so every
//! DLL shares them, as they share the Rayon pool.
//!
//! The values are raw `u32` ticks. The world wraps them in its `Tick` when it
//! reads them. Every accessor is `#[inline(never)]`, so the thread-local it
//! touches is always the one inside `pill_core.dll`, never a copy inlined
//! into a caller.

// Standard library
use std::cell::Cell;

// =============================================================================
// Thread-local Ticks
// =============================================================================

thread_local! {
    /// The tick at which the system on this thread last ran. `None` outside a
    /// parallel batch, where the world uses its own `system_last_run` field.
    static LAST_RUN_TICK: Cell<Option<u32>> = const { Cell::new(None) };

    /// The tick reserved for the system on this thread. `None` means the
    /// world adds one to its shared tick counter instead.
    static THIS_RUN_TICK: Cell<Option<u32>> = const { Cell::new(None) };
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the tick at which the system on this thread last ran.
///
/// `None` means no parallel batch installed a value, so the world uses its
/// own `system_last_run` field.
#[inline(never)]
pub fn last_run_tick() -> Option<u32> {
    LAST_RUN_TICK.with(Cell::get)
}

/// Replaces this thread's last-run tick and returns the previous value.
///
/// The caller restores that value when the system finishes.
#[inline(never)]
pub fn replace_last_run_tick(value: Option<u32>) -> Option<u32> {
    LAST_RUN_TICK.with(|tick_cell| tick_cell.replace(value))
}

/// Returns the tick reserved for the system on this thread.
///
/// `None` means no parallel batch installed a value, so the world increases
/// its shared tick counter instead.
#[inline(never)]
pub fn this_run_tick() -> Option<u32> {
    THIS_RUN_TICK.with(Cell::get)
}

/// Replaces this thread's reserved tick and returns the previous value.
///
/// The caller restores that value when the system finishes.
#[inline(never)]
pub fn replace_this_run_tick(value: Option<u32>) -> Option<u32> {
    THIS_RUN_TICK.with(|tick_cell| tick_cell.replace(value))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Replacing returns the earlier value, and each thread starts empty.
    #[test]
    fn values_are_per_thread_and_restorable() {
        assert_eq!(replace_this_run_tick(Some(7)), None);
        assert_eq!(replace_last_run_tick(Some(3)), None);
        assert_eq!(this_run_tick(), Some(7));
        assert_eq!(last_run_tick(), Some(3));
        let other_thread_ticks = std::thread::spawn(|| (this_run_tick(), last_run_tick()))
            .join()
            .expect("thread runs");
        assert_eq!(other_thread_ticks, (None, None));
        assert_eq!(replace_this_run_tick(None), Some(7));
        assert_eq!(replace_last_run_tick(None), Some(3));
    }
}
