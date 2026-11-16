//! Frame timing, maintained by the engine and readable from any system.
//!
//! # Responsibilities
//!
//! - Defines [`Time`], the engine-owned clock resource.
//! - Tracks the time elapsed since startup, how long the previous frame took
//!   and how far this frame should advance the simulation.
//!
//! # Design
//!
//! [`Time`] is inserted by [`Engine::new`](crate::engine::Engine::new) and
//! advanced at the top of every frame, before any system runs. Systems read it
//! through `Res<Time>`.
//!
//! [`Engine::process_frame`](crate::engine::Engine::process_frame) advances the
//! clock itself instead of running a registered system. A "time system" would
//! only run ahead of the systems that read the clock if every project
//! registered it first. The scheduler fills its parallel batches in
//! registration order (see [`ensure_execution_graph`](crate::scheduler::SystemScheduler::ensure_execution_graph)),
//! so a project that registered a time system late would leave the systems
//! before it reading the previous frame's values. Advancing the clock outside
//! the system set makes every system in a frame see the same values, whatever
//! order the systems were registered in.

// Standard library
use std::time::Duration;

// External crates
use pill_core::platform::Instant;

// Current crate
use crate::resource::Resource;

// =============================================================================
// Constants
// =============================================================================

/// Largest delta the clock will report, in seconds.
///
/// A breakpoint, a hot reload or a slow first frame can make one frame last far
/// longer than normal. Without a limit, the next physics step would move
/// objects far enough to pass through a wall, so the delta stops at this value.
/// Measured time is not limited: [`Time::elapsed`] keeps counting real time.
const MAXIMUM_DELTA_SECONDS: f32 = 0.1;

// =============================================================================
// Time
// =============================================================================

/// Frame timing for the running project: how long the frame took, how much
/// time has passed and how far to advance the simulation.
///
/// The engine creates the clock and inserts it into the world. Read it from any
/// system:
///
/// ```no_run
/// # use pill_engine::*;
/// fn movement(time: Res<Time>) {
///     let Some(time) = time.get() else { return };
///     let _step = 100.0 * time.delta_seconds();
/// }
/// ```
///
/// Every value describes the frame being processed and does not change while
/// that frame runs, so two systems in one frame always agree.
pub struct Time {
    /// The instant the clock started, which is when the engine was created.
    started_at: Instant,
    /// When the current frame began.
    frame_start: Instant,
    /// Wall-clock time from `started_at` to the current frame's start.
    elapsed: Duration,
    /// Unclamped duration of the frame before this one.
    last_frame: Duration,
    /// `last_frame` capped at [`MAXIMUM_DELTA_SECONDS`]; gameplay advances by
    /// this value.
    delta: Duration,
    /// Frames started since the clock was created, counting the current one.
    ///
    /// This is not the world's change tick. That counter orders change
    /// detection and also increases at other times, such as when a mutable
    /// query starts iterating. This counter only counts frames, so a periodic
    /// task can use it.
    frame_count: u64,
}

impl Resource for Time {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Keep
    }
}

impl Time {
    /// Starts a clock with its origin at the current instant.
    ///
    /// The first frame reports a delta of zero. No frame has ended yet, so there
    /// is no real duration to report, and an invented one would show up as a
    /// visible jump on the first frame.
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            started_at: now,
            frame_start: now,
            elapsed: Duration::ZERO,
            last_frame: Duration::ZERO,
            delta: Duration::ZERO,
            // The first `advance` makes this 1, so the first frame a system
            // observes is frame 1 rather than frame 0.
            frame_count: 0,
        }
    }

    /// Moves the clock to the next frame and measures the frame that just
    /// ended.
    ///
    /// [`Engine::process_frame`](crate::engine::Engine::process_frame) calls it
    /// at the top of each frame, before systems run.
    pub(crate) fn advance(&mut self) {
        let now = Instant::now();
        self.last_frame = now.duration_since(self.frame_start);
        self.delta =
            Duration::from_secs_f32(self.last_frame.as_secs_f32().min(MAXIMUM_DELTA_SECONDS));
        self.frame_start = now;
        self.elapsed = now.duration_since(self.started_at);
        // A saturated add rather than a wrapping one: at 1000 frames per second
        // the counter takes about 584 million years to reach this point, and a
        // wrap would make every periodic task that counts frames fire at once,
        // without any error.
        self.frame_count = self.frame_count.saturating_add(1);
    }

    // -------------------------------------------------------------------------
    // Frames
    // -------------------------------------------------------------------------

    /// Returns the number of the frame being processed.
    ///
    /// The value is `1` during the first frame. Use it to run something every
    /// so many frames: `time.frame_count() % 100 == 0`.
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    // -------------------------------------------------------------------------
    // Elapsed since start
    // -------------------------------------------------------------------------

    /// Returns the milliseconds elapsed since the project started.
    ///
    /// The value is a `u128`, the type [`Duration::as_millis`] returns.
    /// Narrowing it here would cut large values off without any warning.
    pub fn elapsed_milliseconds(&self) -> u128 {
        self.elapsed.as_millis()
    }

    /// Returns the seconds elapsed since the project started.
    pub fn elapsed_seconds(&self) -> f32 {
        self.elapsed.as_secs_f32()
    }

    /// Returns the time elapsed since the project started.
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    // -------------------------------------------------------------------------
    // Previous frame
    // -------------------------------------------------------------------------

    /// Returns how long the previous frame took, in milliseconds.
    ///
    /// Unlike [`Self::delta_seconds`], this value is not capped: it is a
    /// measurement, so a frame that really took 400 ms reports 400 ms. Use it
    /// for diagnostics and frame-time displays, never to advance motion.
    pub fn last_frame_milliseconds(&self) -> f32 {
        self.last_frame.as_secs_f32() * 1000.0
    }

    /// Returns how long the previous frame took.
    pub fn last_frame(&self) -> Duration {
        self.last_frame
    }

    // -------------------------------------------------------------------------
    // Delta
    // -------------------------------------------------------------------------

    /// Returns the seconds to advance the simulation by during this frame.
    ///
    /// This is the previous frame's duration, capped at
    /// [`MAXIMUM_DELTA_SECONDS`] so a stalled frame cannot move a physics step
    /// through geometry. Gameplay multiplies motion by this value.
    pub fn delta_seconds(&self) -> f32 {
        self.delta.as_secs_f32()
    }

    /// Returns the milliseconds to advance the simulation by during this frame.
    pub fn delta_milliseconds(&self) -> f32 {
        self.delta.as_secs_f32() * 1000.0
    }

    /// Returns the duration to advance the simulation by during this frame.
    pub fn delta(&self) -> Duration {
        self.delta
    }
}

impl Default for Time {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Time {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Time")
            .field("elapsed_ms", &self.elapsed_milliseconds())
            .field("last_frame_ms", &self.last_frame_milliseconds())
            .field("delta_ms", &self.delta_milliseconds())
            .finish()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A new clock has not run a frame yet, so it reports zero elapsed time, a
    /// zero delta and a zero-length previous frame.
    #[test]
    fn a_new_clock_starts_at_zero() {
        let time = Time::new();

        assert_eq!(time.elapsed_milliseconds(), 0);
        assert_eq!(time.delta_seconds(), 0.0);
        assert_eq!(time.last_frame_milliseconds(), 0.0);
    }

    /// Advancing records how long the frame that just ended took and moves
    /// elapsed time forward.
    #[test]
    fn advancing_measures_the_previous_frame() {
        let mut time = Time::new();
        std::thread::sleep(Duration::from_millis(12));
        time.advance();

        assert!(
            time.last_frame_milliseconds() >= 10.0,
            "a ~12ms frame should report at least 10ms, got {}",
            time.last_frame_milliseconds()
        );
        assert!(time.elapsed_milliseconds() >= 10);
        assert!(time.delta_seconds() > 0.0);
    }

    /// Elapsed time accumulates across frames rather than resetting.
    #[test]
    fn elapsed_accumulates_across_frames() {
        let mut time = Time::new();

        std::thread::sleep(Duration::from_millis(5));
        time.advance();
        let after_first = time.elapsed_milliseconds();

        std::thread::sleep(Duration::from_millis(5));
        time.advance();
        let after_second = time.elapsed_milliseconds();

        assert!(
            after_second > after_first,
            "elapsed must grow: {after_first} then {after_second}"
        );
    }

    /// A long frame is capped in the delta but reported in full as the previous
    /// frame's length, which is the difference between the two accessors.
    #[test]
    fn delta_is_clamped_while_the_measurement_is_not() {
        let mut time = Time::new();
        // Longer than MAXIMUM_DELTA_SECONDS (0.1 s), like a frame stalled at a
        // breakpoint or during a reload.
        std::thread::sleep(Duration::from_millis(150));
        time.advance();

        assert!(
            time.delta_seconds() <= MAXIMUM_DELTA_SECONDS,
            "delta must be clamped, got {}",
            time.delta_seconds()
        );
        assert!(
            time.last_frame_milliseconds() >= 140.0,
            "the raw measurement must not be clamped, got {}",
            time.last_frame_milliseconds()
        );
    }

    /// The second and millisecond accessors report the same value, up to
    /// rounding.
    #[test]
    fn second_and_millisecond_accessors_agree() {
        let mut time = Time::new();
        std::thread::sleep(Duration::from_millis(20));
        time.advance();

        let from_seconds = time.elapsed_seconds() * 1000.0;
        let from_milliseconds = time.elapsed_milliseconds() as f32;
        assert!(
            (from_seconds - from_milliseconds).abs() < 2.0,
            "{from_seconds} and {from_milliseconds} should agree within rounding"
        );

        assert!((time.delta_milliseconds() - time.delta_seconds() * 1000.0).abs() < 0.001);
    }

    /// Values stay fixed between advances, so every system in one frame agrees.
    #[test]
    fn values_are_stable_within_a_frame() {
        let mut time = Time::new();
        std::thread::sleep(Duration::from_millis(5));
        time.advance();

        let elapsed_milliseconds = time.elapsed_milliseconds();
        let delta_seconds = time.delta_seconds();
        std::thread::sleep(Duration::from_millis(5));

        assert_eq!(time.elapsed_milliseconds(), elapsed_milliseconds);
        assert_eq!(time.delta_seconds(), delta_seconds);
    }
}
