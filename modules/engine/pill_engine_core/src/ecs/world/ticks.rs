//! Change detection: the world's tick counter and each system's baseline.
//!
//! # Responsibilities
//!
//! - Owns the world tick counter that every component write is stamped with.
//! - Answers what "since my last run" means for the system that is running
//!   now: the baseline that `Changed<T>` and `Added<T>` compare against.
//!
//! # Design
//!
//! A component instance carries two ticks of its own, `added` and `changed`.
//! They are kept in a separate list beside the component's data, at the same
//! index as the entity's row. A write through a mutable query sets `changed`
//! to the tick of the current run. Nothing in this file keeps per-row
//! bookkeeping.
//!
//! A filter therefore needs only two numbers: the tick of the current run,
//! and the tick at which the calling system last ran. The second one is the
//! baseline that `Changed<T>` and `Added<T>` compare against.
//!
//! The baseline comes from one of two places. In sequential execution it is
//! the `system_last_run` field on `World`, which is set before each system
//! runs. Two systems running at the same time in a parallel batch need
//! different baselines, and one shared field would race, so the engine
//! installs the value in a thread-local before each system runs. Every
//! thread then reads the value installed for its own system, and no two
//! threads share a cell.
//!
//! The tick is a `u32` that wraps: after `u32::MAX` the next one is 0.
//! Comparisons such as `Tick::is_newer_than` therefore measure the distance
//! between two ticks instead of their order, so a wrap changes nothing.

use super::*;

// =============================================================================
// World - Change Detection
// =============================================================================

impl World {
    /// Returns the current world tick and leaves the counter unchanged.
    #[inline]
    pub fn change_tick(&self) -> Tick {
        Tick::new(self.change_tick)
    }

    /// Increases the world tick by one and returns the new value.
    ///
    /// The [`Engine`](crate::engine::Engine) calls this once per frame, and a
    /// mutable query calls it when its iteration begins, so mutations made at
    /// different points of a frame can still be told apart by their tick. A
    /// write applied between frames (a field edit, for example) calls it too,
    /// so the value it stamps is newer than every system's baseline.
    ///
    /// Inside a parallel batch every system runs against the same world, so
    /// increasing the shared counter from a worker would be an unsynchronized
    /// read-modify-write. The engine therefore allocates one tick per batch
    /// member on the dispatch thread and installs it for the thread that runs
    /// that system. Inside a system the reserved tick is returned every time,
    /// however often it is asked for, and the shared counter is left alone.
    #[inline]
    pub fn increment_change_tick(&mut self) -> Tick {
        if let Some(reserved_this_run_tick) = per_thread_this_run_tick() {
            return reserved_this_run_tick;
        }
        self.change_tick = self.change_tick.wrapping_add(1);
        Tick::new(self.change_tick)
    }

    /// Returns the tick at which the calling system last ran.
    ///
    /// When a parallel batch installed a value for this thread, that value
    /// wins. Otherwise the answer comes from the shared field on the world,
    /// which sequential systems and ad-hoc queries use.
    #[inline]
    pub fn system_last_run(&self) -> Tick {
        if let Some(installed_last_run_tick) = per_thread_last_run_tick() {
            return installed_last_run_tick;
        }
        Tick::new(self.system_last_run)
    }

    /// Sets the world-level baseline tick directly.
    ///
    /// The engine sets the baseline itself before each system runs, so this
    /// setter is meant for tests and for one-off queries that compare against
    /// a tick they chose.
    #[inline]
    pub fn set_system_last_run(&mut self, tick: Tick) {
        self.system_last_run = tick.get();
    }
}
