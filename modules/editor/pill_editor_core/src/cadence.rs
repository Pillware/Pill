//! Redraw cadence: how the editor decides when to draw the next frame.
//!
//! # Responsibilities
//!
//! - Name every reason a frame is asked for: input, a scripted command, a
//!   changed snapshot, a resize, something moving, or egui's own repaint
//!   request.
//! - Decide whether the loop draws at once or sleeps for the idle poll.
//! - Count the frames it drew, how many of them were asked for, and how often
//!   it waited.
//! - Read `PILL_EDITOR_CONTINUOUS_REDRAW`, the switch a profile or a suite
//!   sets so that every frame is drawn.
//!
//! # Rules
//!
//! A frame is drawn at once when something asks for it - input arrived, a
//! scripted command arrived, the snapshot changed, the window resized, egui
//! asked for a repaint - or while something is moving, which keeps a running
//! project at the display rate. Only a settled editor sleeps for
//! [`IDLE_POLL`], so an idle one stops drawing.
//!
//! # Design
//!
//! The decision is plain data, so it can be tested without an event loop. The
//! frontend collects this frame's [`FrameDemand`] and asks a [`Cadence`] what
//! to do next. Nothing here touches a window, a clock or the world.

// Standard library
use std::time::Duration;

// =============================================================================
// Constants
// =============================================================================

/// The environment variable that makes the loop draw every frame.
pub const CONTINUOUS_REDRAW_ENV: &str = "PILL_EDITOR_CONTINUOUS_REDRAW";

/// How long an idle editor sleeps between frames.
///
/// A change the host makes on one of its own threads - a notice, a build
/// finishing, a console line - cannot wake the waiting loop the way a window
/// event does. The loop wakes on this timer instead and draws the change, so
/// this value is the Console tab's worst-case delay. Input still wakes the
/// loop sooner.
pub const IDLE_POLL: Duration = Duration::from_millis(250);

// =============================================================================
// Types
// =============================================================================

/// What asks for the next frame.
///
/// Each flag is set by whoever knows about it: the event handler sets `input`
/// and `resized`, the scripted channel sets `scripted`, and the frame itself
/// sets `snapshot`, `animating` and `repaint_requested`. The whole demand is
/// cleared at the end of the frame that honoured it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameDemand {
    /// Input arrived from a window - a pointer move or button, a key or text,
    /// the wheel - or a drag out of the editor finished.
    pub input: bool,
    /// A scripted command arrived from the test channel.
    pub scripted: bool,
    /// The snapshot this frame published differs from the last one.
    pub snapshot: bool,
    /// The window's size or its scale factor changed.
    pub resized: bool,
    /// Something is moving: the engine is not paused, or a camera navigation
    /// or scripted drag is in flight.
    pub animating: bool,
    /// egui asked for a repaint at its own pace.
    pub repaint_requested: bool,
}

impl FrameDemand {
    /// Whether any reason to draw is set.
    pub fn any(&self) -> bool {
        self.input
            || self.scripted
            || self.snapshot
            || self.resized
            || self.animating
            || self.repaint_requested
    }

    /// Whether input, a scripted command, a changed snapshot or a resize asked
    /// for the frame.
    ///
    /// The other two reasons - something moving and egui's repaint request -
    /// are checked separately by [`Cadence::decide`].
    pub fn urgent(&self) -> bool {
        self.input || self.scripted || self.snapshot || self.resized
    }
}

/// When the loop draws next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameWake {
    /// Draw as soon as the loop gets the turn.
    Immediate,
    /// Sleep this long first; input still wakes the loop sooner.
    After(Duration),
}

/// The editor's redraw counters, and the mode that sets when it draws next.
#[derive(Clone, Debug)]
pub struct Cadence {
    continuous: bool,
    /// Frames drawn since startup.
    frames: u64,
    /// Frames drawn because something asked for one; the continuous mode
    /// counts every frame.
    demanded: u64,
    /// Times the loop went to sleep instead of drawing.
    waits: u64,
}

impl Cadence {
    /// A cadence with an explicit mode: `continuous` draws every frame,
    /// whatever the demand is.
    pub fn new(continuous: bool) -> Self {
        Self {
            continuous,
            frames: 0,
            demanded: 0,
            waits: 0,
        }
    }

    /// A cadence chosen from the environment: continuous when
    /// [`CONTINUOUS_REDRAW_ENV`] is set to anything except an empty string or
    /// `0`.
    pub fn from_environment() -> Self {
        let continuous = std::env::var(CONTINUOUS_REDRAW_ENV)
            .map(|variable_value| !variable_value.is_empty() && variable_value != "0")
            .unwrap_or(false);
        Self::new(continuous)
    }

    /// Whether every frame is drawn regardless of demand.
    pub fn continuous(&self) -> bool {
        self.continuous
    }

    /// Frames drawn since startup.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Frames drawn because something asked for one; the continuous mode
    /// counts every frame.
    pub fn demanded(&self) -> u64 {
        self.demanded
    }

    /// How many times the loop slept between frames.
    pub fn waits(&self) -> u64 {
        self.waits
    }

    /// Count the frame the loop just drew.
    pub fn record_frame(&mut self, demand: FrameDemand) {
        self.frames += 1;
        if self.continuous || demand.any() {
            self.demanded += 1;
        }
    }

    /// Count a sleep the loop took.
    pub fn record_wait(&mut self) {
        self.waits += 1;
    }

    /// Decide when to draw next.
    ///
    /// The loop draws at once when the cadence is continuous, when
    /// [`FrameDemand::urgent`] is set, while something is moving, or when egui
    /// asked for a repaint. Otherwise the frame sleeps for [`IDLE_POLL`].
    pub fn decide(&self, demand: FrameDemand) -> FrameWake {
        if self.continuous || demand.urgent() || demand.animating || demand.repaint_requested {
            FrameWake::Immediate
        } else {
            FrameWake::After(IDLE_POLL)
        }
    }
}

impl Default for Cadence {
    fn default() -> Self {
        Self::new(false)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_continuous_cadence_always_draws_at_once() {
        let cadence = Cadence::new(true);
        assert_eq!(cadence.decide(FrameDemand::default()), FrameWake::Immediate);
    }

    #[test]
    fn each_urgent_reason_draws_at_once() {
        let cadence = Cadence::new(false);
        for demand in [
            FrameDemand {
                input: true,
                ..FrameDemand::default()
            },
            FrameDemand {
                scripted: true,
                ..FrameDemand::default()
            },
            FrameDemand {
                snapshot: true,
                ..FrameDemand::default()
            },
            FrameDemand {
                resized: true,
                ..FrameDemand::default()
            },
        ] {
            assert!(demand.any());
            assert_eq!(cadence.decide(demand), FrameWake::Immediate);
        }
    }

    #[test]
    fn a_repaint_request_draws_at_once() {
        let cadence = Cadence::new(false);
        let demand = FrameDemand {
            repaint_requested: true,
            ..FrameDemand::default()
        };
        assert_eq!(cadence.decide(demand), FrameWake::Immediate);
    }

    #[test]
    fn an_idle_editor_sleeps_for_the_poll_interval() {
        let cadence = Cadence::new(false);
        assert_eq!(
            cadence.decide(FrameDemand::default()),
            FrameWake::After(IDLE_POLL)
        );
    }

    #[test]
    fn something_moving_draws_at_once() {
        let demand = FrameDemand {
            animating: true,
            ..FrameDemand::default()
        };
        assert!(demand.any());
        let cadence = Cadence::new(false);
        assert_eq!(cadence.decide(demand), FrameWake::Immediate);
    }

    #[test]
    fn the_counters_separate_demanded_frames_from_idle_ones() {
        let mut cadence = Cadence::new(false);
        cadence.record_frame(FrameDemand {
            input: true,
            ..FrameDemand::default()
        });
        cadence.record_frame(FrameDemand::default());
        cadence.record_wait();
        assert_eq!(cadence.frames(), 2);
        assert_eq!(cadence.demanded(), 1);
        assert_eq!(cadence.waits(), 1);
    }
}
