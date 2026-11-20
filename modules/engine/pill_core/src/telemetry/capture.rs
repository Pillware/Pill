//! Bounded in-process log capture for the editor console.
//!
//! # Responsibilities
//!
//! - [`LogBuffer`]: a clonable handle to a bounded ring of owned log lines.
//! - [`CaptureLayer`]: a tracing layer that copies each event into the ring.
//!
//! # Design
//!
//! The ring is bounded by line count: once it is full, the oldest lines are
//! dropped first. Every line carries a sequence number that only goes up, so
//! a reader asks for just the lines it has not seen yet
//! ([`LogBuffer::since`]). An event is formatted before it reaches the ring,
//! so no caller ever holds the lock while it formats text.
//!
//! Every line holds owned strings, never anything borrowed from a loaded
//! module. That is what lets the editor shell - itself a reloadable module -
//! read lines that another module logged before it unloaded.

// Standard library
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

// External crates
use tracing::{Event, Level, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::layer::{Context, Layer};

// Current crate
use super::StyledFieldVisitor;

// =============================================================================
// Constants
// =============================================================================

/// The number of lines a ring keeps when the caller does not choose a size.
pub const DEFAULT_CAPTURE_CAPACITY: usize = 4096;

// =============================================================================
// Types
// =============================================================================

/// One captured log line, with its own copy of every string.
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// Sequence number that only goes up: it starts at 1 and no two lines
    /// share one.
    pub seq: u64,
    /// How serious the line is, low to high: 1 trace, 2 debug, 3 info,
    /// 4 warn, 5 error.
    pub level: u8,
    /// The tracing target the line came from (the crate or module path).
    pub target: String,
    /// The message text, with the event's structured fields appended after
    /// the engine's ` | ` separator as `name=value`.
    pub message: String,
}

/// The ring's contents, guarded by the mutex inside the shared handle.
struct Ring {
    capacity: usize,
    next_sequence_number: u64,
    lines: VecDeque<LogLine>,
}

/// A clonable handle to a bounded ring of captured log lines.
#[derive(Clone)]
pub struct LogBuffer {
    shared_ring: Arc<Mutex<Ring>>,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPTURE_CAPACITY)
    }
}

impl LogBuffer {
    /// Creates a buffer that keeps at most `capacity` lines; a capacity of 0
    /// is treated as 1.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            shared_ring: Arc::new(Mutex::new(Ring {
                capacity: capacity.max(1),
                next_sequence_number: 1,
                lines: VecDeque::new(),
            })),
        }
    }

    /// Locks the ring and returns the guard, even if the lock was poisoned.
    ///
    /// A poisoned lock still holds every line written before the panic. The
    /// console exists to show what went wrong, so a panic in another thread
    /// must not leave the panel empty for the rest of the session. Every
    /// accessor here recovers the same way, as the locks in `metrics` and
    /// `native_buffer` do.
    fn lock_ring(&self) -> std::sync::MutexGuard<'_, Ring> {
        self.shared_ring
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Appends one line and drops the oldest line once the ring is full.
    pub fn push(&self, level: u8, target: &str, message: String) {
        let mut ring = self.lock_ring();
        let seq = ring.next_sequence_number;
        ring.next_sequence_number += 1;
        ring.lines.push_back(LogLine {
            seq,
            level,
            target: target.to_string(),
            message,
        });
        while ring.lines.len() > ring.capacity {
            ring.lines.pop_front();
        }
    }

    /// Returns every line with a sequence number greater than
    /// `after_sequence_number`, oldest first.
    pub fn since(&self, after_sequence_number: u64) -> Vec<LogLine> {
        let ring = self.lock_ring();
        ring.lines
            .iter()
            .filter(|line| line.seq > after_sequence_number)
            .cloned()
            .collect()
    }

    /// The sequence number of the newest line (0 while empty).
    pub fn latest_seq(&self) -> u64 {
        self.lock_ring().next_sequence_number - 1
    }

    /// The number of lines the ring holds now.
    pub fn len(&self) -> usize {
        self.lock_ring().lines.len()
    }

    /// Whether the ring holds no lines.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The number of lines the ring holds before it starts dropping the
    /// oldest ones.
    pub fn capacity(&self) -> usize {
        self.lock_ring().capacity
    }
}

impl fmt::Debug for LogBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogBuffer")
            .field("len", &self.len())
            .field("capacity", &self.capacity())
            .finish()
    }
}

/// A tracing layer that copies events at or above a level into a
/// [`LogBuffer`].
pub struct CaptureLayer {
    buffer: LogBuffer,
    minimum_level: u8,
}

impl CaptureLayer {
    /// Creates a layer that copies events at or above `minimum_level` into
    /// `buffer`.
    pub fn new(buffer: LogBuffer, minimum_level: Level) -> Self {
        Self {
            buffer,
            minimum_level: level_value(minimum_level),
        }
    }
}

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let metadata = event.metadata();
        let level = level_value(*metadata.level());
        if level < self.minimum_level {
            return;
        }
        let mut visitor = StyledFieldVisitor::plain(event.is_log());
        event.record(&mut visitor);
        self.buffer
            .push(level, metadata.target(), visitor.into_line());
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the number [`LogLine::level`] stores for a logging level: 1 trace
/// through 5 error.
pub fn level_value(level: Level) -> u8 {
    match level {
        Level::TRACE => 1,
        Level::DEBUG => 2,
        Level::INFO => 3,
        Level::WARN => 4,
        Level::ERROR => 5,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::{log_block, telemetry_target};

    /// A captured line has the same shape as the line the terminal formatter
    /// writes: the same fields, the same spelling for each value, and the
    /// bridged `log.*` fields left out.
    ///
    /// Both paths use the same visitor, so this test pins the shape they
    /// share instead of one renderer's separate idea of it.
    #[test]
    fn a_captured_line_matches_the_formatter_line_shape() {
        use tracing_subscriber::layer::SubscriberExt;

        let buffer = LogBuffer::with_capacity(8);
        let subscriber =
            tracing_subscriber::registry().with(CaptureLayer::new(buffer.clone(), Level::INFO));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                target: telemetry_target::RESOURCES,
                module = "pill_spline",
                "texture created"
            );
        });

        let lines = buffer.since(0);
        assert_eq!(lines.len(), 1, "one event, one line");
        assert_eq!(
            lines[0].message, "texture created | module=\"pill_spline\"",
            "a string field keeps its `Debug` form, as the terminal lane shows it"
        );
    }

    /// A block's fields belong on its heading line, as the terminal formatter
    /// puts them, not after the body.
    #[test]
    fn a_captured_block_puts_its_fields_on_the_heading_line() {
        use tracing_subscriber::layer::SubscriberExt;

        let buffer = LogBuffer::with_capacity(8);
        let subscriber =
            tracing_subscriber::registry().with(CaptureLayer::new(buffer.clone(), Level::INFO));
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(
                target: telemetry_target::HOT_RELOAD,
                total = 2,
                "{}",
                log_block("Modules to build:", ["1. pill_spline", "2. project"])
            );
        });

        let lines = buffer.since(0);
        assert_eq!(lines.len(), 1, "one event, one line");
        assert_eq!(
            lines[0].message, "Modules to build: | total=2\n1. pill_spline\n2. project",
            "the console line must put the fields where the log file does"
        );
    }

    /// A poisoned ring still returns the lines written before the panic.
    ///
    /// The console is there to show what went wrong, so a panic in another
    /// thread must not leave it empty for the rest of the session.
    #[test]
    fn a_poisoned_ring_still_returns_its_lines() {
        let buffer = LogBuffer::with_capacity(8);
        buffer.push(3, "test", "before the panic".to_string());

        // Poison the ring: this thread panics while it holds the lock.
        let poisoned = buffer.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned
                .shared_ring
                .lock()
                .expect("the ring is unpoisoned here");
            panic!("a writer panicked while holding the guard");
        })
        .join();
        assert!(buffer.shared_ring.is_poisoned(), "the ring is poisoned");

        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.latest_seq(), 1);
        assert_eq!(buffer.since(0).len(), 1);
        buffer.push(3, "test", "after the panic".to_string());
        assert_eq!(buffer.since(1).len(), 1, "capture continues after a poison");
    }

    #[test]
    fn the_ring_drops_oldest_and_tracks_sequence() {
        let buffer = LogBuffer::with_capacity(3);
        for index in 0..5u32 {
            buffer.push(3, "test", format!("line {index}"));
        }
        assert_eq!(buffer.len(), 3);
        let lines = buffer.since(0);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].message, "line 2");
        assert_eq!(lines[2].message, "line 4");
        assert_eq!(buffer.latest_seq(), 5);

        let newer_lines = buffer.since(3);
        assert_eq!(newer_lines.len(), 2);
        assert_eq!(newer_lines[0].message, "line 3");
    }

    #[test]
    fn since_returns_only_newer_lines() {
        let buffer = LogBuffer::with_capacity(8);
        buffer.push(4, "engine::ecs", "first".to_string());
        let latest_sequence_number = buffer.latest_seq();
        buffer.push(5, "project", "second".to_string());
        let newer_lines = buffer.since(latest_sequence_number);
        assert_eq!(newer_lines.len(), 1);
        assert_eq!(newer_lines[0].target, "project");
        assert_eq!(newer_lines[0].level, 5);
    }

    #[test]
    fn an_empty_buffer_reports_zero() {
        let buffer = LogBuffer::default();
        assert_eq!(buffer.latest_seq(), 0);
        assert!(buffer.since(0).is_empty());
        assert_eq!(buffer.capacity(), DEFAULT_CAPTURE_CAPACITY);
    }
}
