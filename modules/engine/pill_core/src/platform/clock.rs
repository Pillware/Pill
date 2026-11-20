//! Monotonic and wall-clock time on every target.
//!
//! # Responsibilities
//!
//! - Define [`Instant`], the engine's monotonic timestamp, with the subset of
//!   the `std::time::Instant` API the engine uses.
//! - Provide [`unix_time`], the wall-clock time since the Unix epoch.
//!
//! On native both come from `std::time`. The `std` versions panic on
//! `wasm32-unknown-unknown`, so there both come from `web-time`, which reads
//! `performance.now()` and `Date.now()`.
//!
//! [`Instant`] is a newtype, not a re-export. Clippy's `disallowed-types` lint
//! matches the type behind a name, so a re-export would still be
//! `std::time::Instant` and every use through it would be reported.

// Standard library
use std::ops::{Add, AddAssign, Sub, SubAssign};
use std::time::Duration;

// The engine's monotonic clock type: `std` on native, `web-time` on the web.
// Code elsewhere uses `Instant` rather than naming `std::time::Instant`, which
// the workspace lint bans.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::disallowed_types)]
type PlatformInstant = std::time::Instant;
#[cfg(target_arch = "wasm32")]
type PlatformInstant = web_time::Instant;

/// A monotonic timestamp that works on native and on the web.
///
/// Use it wherever `std::time::Instant` would be used. It offers the part of
/// that API the engine uses: `now`, `elapsed`, the `duration_since` methods,
/// and arithmetic with [`Duration`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Instant(PlatformInstant);

impl Instant {
    /// The current instant.
    pub fn now() -> Self {
        Self(PlatformInstant::now())
    }

    /// The time elapsed since this instant.
    pub fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    /// The time from `earlier` to this instant, zero if `earlier` is later.
    ///
    /// It saturates: a reversed pair gives zero instead of a panic, the same
    /// as [`std::time::Instant::duration_since`]. This and
    /// [`saturating_duration_since`] do the same thing; prefer this one in new
    /// code.
    ///
    /// [`saturating_duration_since`]: Instant::saturating_duration_since
    pub fn duration_since(&self, earlier: Instant) -> Duration {
        self.saturating_duration_since(earlier)
    }

    /// The time from `earlier` to this instant, zero if `earlier` is later.
    ///
    /// The same operation as [`duration_since`](Instant::duration_since), under
    /// the name the standard library uses.
    pub fn saturating_duration_since(&self, earlier: Instant) -> Duration {
        self.0.saturating_duration_since(earlier.0)
    }

    /// The time from `earlier` to this instant, or `None` if `earlier` is later.
    pub fn checked_duration_since(&self, earlier: Instant) -> Option<Duration> {
        self.0.checked_duration_since(earlier.0)
    }

    /// This instant moved forward by `duration`, or `None` on overflow.
    pub fn checked_add(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_add(duration).map(Self)
    }

    /// This instant moved back by `duration`, or `None` on underflow.
    pub fn checked_sub(&self, duration: Duration) -> Option<Instant> {
        self.0.checked_sub(duration).map(Self)
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;

    fn add(self, duration: Duration) -> Instant {
        Self(self.0 + duration)
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, duration: Duration) {
        self.0 += duration;
    }
}

impl Sub<Duration> for Instant {
    type Output = Instant;

    fn sub(self, duration: Duration) -> Instant {
        Self(self.0 - duration)
    }
}

impl SubAssign<Duration> for Instant {
    fn sub_assign(&mut self, duration: Duration) {
        self.0 -= duration;
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;

    fn sub(self, earlier: Instant) -> Duration {
        self.duration_since(earlier)
    }
}

/// The wall-clock time since the Unix epoch, zero if the clock is set before it.
///
/// Use it for a time a person reads, such as a calendar date; use [`Instant`]
/// to measure elapsed time.
pub fn unix_time() -> Duration {
    #[cfg(not(target_arch = "wasm32"))]
    let elapsed_since_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    #[cfg(target_arch = "wasm32")]
    let elapsed_since_epoch = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH);
    elapsed_since_epoch.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instants_are_monotonic_and_measure_elapsed_time() {
        let earlier = Instant::now();
        let later = earlier + Duration::from_millis(5);
        assert!(later > earlier);
        assert_eq!(later - earlier, Duration::from_millis(5));
        assert_eq!(later.duration_since(earlier), Duration::from_millis(5));
        assert_eq!(earlier.duration_since(later), Duration::ZERO);
        assert_eq!(earlier.checked_duration_since(later), None);
        assert_eq!(later.checked_sub(Duration::from_millis(5)), Some(earlier));
    }

    #[test]
    fn unix_time_is_after_the_epoch() {
        assert!(unix_time() > Duration::ZERO);
    }
}
