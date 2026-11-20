//! The [`Mut<T>`] handle a query yields for a component or resource it may
//! change.
//!
//! # Responsibilities
//!
//! - Gives mutable access to a component through `DerefMut`, while keeping a
//!   borrow of the component's [`ComponentTicks`].
//! - Records the component as changed on every mutable dereference, so a
//!   [`Changed`](crate::query::Changed) filter run later sees the row.
//!
//! # Design
//!
//! A query that asks for `&mut T` yields the row as `Mut<'a, T>` instead of a
//! plain `&'a mut T`. Reading through `Deref` leaves the row's ticks alone,
//! so a row only counts as changed once something is written. Writing through
//! `DerefMut` sets [`ComponentTicks::changed`] to the tick of the current run.
//! That store is per row and needs no locking: the scheduler never runs two
//! systems that write the same component at the same time.

// Standard library
use std::ops::{Deref, DerefMut};

// Current crate
use crate::component::{ComponentTicks, Tick};

// =============================================================================
// Mut
// =============================================================================

/// Mutable, change-tracking access to one component (or resource) row.
///
/// The handle holds a borrow of the component's value, a borrow of its
/// [`ComponentTicks`], and the tick of the current run (`this_run`). Writing
/// through `DerefMut` sets `ticks.changed` to that tick, so the next
/// [`Changed`](crate::query::Changed) check sees the write. The store needs no
/// synchronization because the scheduler never lets two threads write the same
/// row.
///
/// # Examples
///
/// ```no_run
/// # use pill_engine::*;
/// # #[derive(Debug, Clone)] struct Transform { x: f32, y: f32 }
/// # impl Component for Transform {}
/// # #[derive(Debug, Clone)] struct Velocity { x: f32, y: f32 }
/// # impl Component for Velocity {}
/// fn movement(mut q: Query<(&mut Transform, &Velocity)>) {
///     for (mut transform, velocity) in q.iter_mut() {
///         transform.x += velocity.x; // DerefMut marks the row as changed
///     }
/// }
/// ```
pub struct Mut<'a, T: ?Sized> {
    /// The component value this handle gives access to.
    pub(crate) value: &'a mut T,
    /// The change-detection ticks of this row.
    pub(crate) ticks: &'a mut ComponentTicks,
    /// The tick a write through this handle records as the change.
    pub(crate) this_run: Tick,
}

impl<'a, T: ?Sized> Mut<'a, T> {
    /// Builds a [`Mut`] from the component value, its row's ticks and the
    /// run's tick.
    #[inline]
    pub fn new(value: &'a mut T, ticks: &'a mut ComponentTicks, this_run: Tick) -> Self {
        Self {
            value,
            ticks,
            this_run,
        }
    }

    /// Returns the change-detection ticks of this row.
    #[inline]
    pub fn ticks(&self) -> &ComponentTicks {
        self.ticks
    }

    /// Returns the tick at which this component was added to its entity.
    #[inline]
    pub fn last_added(&self) -> Tick {
        self.ticks.added
    }

    /// Returns the tick at which this component was last marked as changed.
    #[inline]
    pub fn last_changed(&self) -> Tick {
        self.ticks.changed
    }

    /// Returns the tick that a write through this handle records as the
    /// change.
    #[inline]
    pub fn this_run(&self) -> Tick {
        self.this_run
    }

    /// Returns mutable access to the component without recording a change.
    ///
    /// Use it for bookkeeping writes that should not count as a change: the
    /// next [`Changed`](crate::query::Changed) run would otherwise see the row.
    #[inline]
    pub fn bypass_change_detection(&mut self) -> &mut T {
        self.value
    }

    /// Records this row as changed at the handle's run tick, even if nothing
    /// is written through it.
    #[inline]
    pub fn set_changed(&mut self) {
        self.ticks.changed = self.this_run;
    }
}

impl<T: ?Sized> Deref for Mut<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.value
    }
}

impl<T: ?Sized> DerefMut for Mut<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // This store marks the row as changed. It touches only this row, so no
        // atomics are needed: the query and the scheduler guarantee that no
        // two threads hold the same row.
        self.ticks.changed = self.this_run;
        self.value
    }
}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for Mut<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mut")
            .field("value", &self.value)
            .field("ticks", &self.ticks)
            .field("this_run", &self.this_run)
            .finish()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks that reading through `Deref` leaves the `changed` tick alone.
    #[test]
    fn deref_does_not_bump_changed() {
        let mut component_value = 42_i32;
        let mut ticks = ComponentTicks::new(Tick::new(1));
        let component_access = Mut::new(&mut component_value, &mut ticks, Tick::new(5));
        let _ = *component_access;
        assert_eq!(component_access.last_changed(), Tick::new(1));
    }

    /// Checks that `DerefMut` moves the `changed` tick to the handle's run
    /// tick.
    #[test]
    fn deref_mut_bumps_changed() {
        let mut component_value = 42_i32;
        let mut ticks = ComponentTicks::new(Tick::new(1));
        let mut component_access = Mut::new(&mut component_value, &mut ticks, Tick::new(5));
        *component_access += 1;
        assert_eq!(component_access.last_changed(), Tick::new(5));
        assert_eq!(*component_access, 43);
    }

    /// Checks that `bypass_change_detection` writes the value without moving
    /// the `changed` tick.
    #[test]
    fn bypass_skips_tick_bump() {
        let mut component_value = 0_i32;
        let mut ticks = ComponentTicks::new(Tick::new(1));
        let mut component_access = Mut::new(&mut component_value, &mut ticks, Tick::new(7));
        *component_access.bypass_change_detection() = 99;
        assert_eq!(component_access.last_changed(), Tick::new(1));
        assert_eq!(*component_access, 99);
    }

    /// Checks that `set_changed` marks the row at the handle's run tick.
    #[test]
    fn set_changed_marks_row() {
        let mut component_value = 0_i32;
        let mut ticks = ComponentTicks::new(Tick::new(1));
        let mut component_access = Mut::new(&mut component_value, &mut ticks, Tick::new(9));
        component_access.set_changed();
        assert_eq!(component_access.last_changed(), Tick::new(9));
    }
}
