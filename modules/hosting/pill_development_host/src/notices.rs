//! Host activity notes for an attached editor frontend.
//!
//! # Responsibilities
//!
//! - Define [`HostNotice`]: what happened, in editor-agnostic terms.
//! - Queue notices from anywhere in `pill_development_host`, dropping the
//!   oldest once the queue is full.
//! - Hand the queue to the frontend's per-frame drain ([`take`]).
//!
//! # Design
//!
//! The host already reports reloads, patches, failures and rollbacks as log
//! lines, but a log line is only text. An editor frontend needs the values
//! behind it - which module, how long the build took, which generation now
//! runs - to raise its own notices and fill its problems list, so the same
//! events are queued here as plain data as well.
//!
//! The queue is process-global, like the analytics collector: the events are
//! raised from `&mut DevHost` paths, from reload reports and from places that
//! hold no host reference, and the editor wants them all in one list. A host
//! with no editor attached never drains the queue, so [`NOTICE_CAPACITY`]
//! keeps it bounded.
//!
//! A notice carries owned strings only. Queues and drains run on the host's
//! main thread - the startup loads queue before the first frame - and the
//! build runner's reader thread never writes here, so the mutex alone is
//! enough.

// Standard library
use std::collections::VecDeque;
use std::sync::Mutex;

// External crates
use pill_core::telemetry::telemetry_target;

// =============================================================================
// Constants
// =============================================================================

/// How many notices can wait for a drain; past this the oldest are dropped.
///
/// Large enough that a burst (a reload plus its patches) arrives whole, small
/// enough that a host nobody drains stays cheap.
const NOTICE_CAPACITY: usize = 256;

/// The queue [`push`] fills and [`take`] empties.
static NOTICE_QUEUE: Mutex<VecDeque<HostNotice>> = Mutex::new(VecDeque::new());

// =============================================================================
// Types
// =============================================================================

/// One event the host reports to an attached editor frontend.
#[derive(Clone, Debug)]
pub enum HostNotice {
    /// A module failed to load or to swap in: its build failed, the new
    /// library could not load, or the module ABI check refused it.
    ModuleFailed {
        /// The module that failed: an extension crate's name, or the project's.
        module: String,
        /// The text that explains the failure: the captured compiler output
        /// when a build ran, the error's own words otherwise.
        output: String,
    },
    /// A module's new code is live in the running game.
    Reloaded {
        /// The module that swapped: an extension crate's name, or the
        /// project's.
        module: String,
        /// The last cargo build's wall time, or the managed assembly's build
        /// time. `0` when the swap was not timed: a shader-only reload, or an
        /// assembly this host did not build.
        build_ms: u64,
    },
    /// A function's body was replaced in the running game without a reload.
    PatchApplied {
        /// The patched functions' qualified names, comma-separated when one
        /// save changed several.
        function: String,
        /// Wall time from noticing the change to the patch being installed, in
        /// milliseconds.
        milliseconds: f64,
    },
    /// A function's patch attempt failed; the previous implementation still
    /// runs.
    PatchFailed {
        /// The function whose patch failed.
        function: String,
        /// The failure's detail text, as the patch pipeline reported it.
        detail: String,
    },
    /// A function was rolled back to an earlier generation, by request or
    /// automatically after repeated faults.
    RolledBack {
        /// The rolled-back function.
        function: String,
        /// The generation now executing.
        to_generation: u32,
        /// Whether the rollback happened without a developer asking for it.
        automatic: bool,
    },
}

// =============================================================================
// Free Functions
// =============================================================================

/// Queues one notice for the frontend's next drain, dropping the oldest when
/// the queue is full.
pub(crate) fn push(notice: HostNotice) {
    // Step 1: Log the notice. When no editor drains the queue, this line is
    // the only record of what the host did.
    pill_core::info!(
        target: telemetry_target::ENGINE,
        kind = notice_kind_log_name(&notice),
        "[editor] host notice"
    );
    // Step 2: Add the notice to the queue, dropping the oldest one when the
    // queue is full.
    let mut queue = NOTICE_QUEUE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if queue.len() >= NOTICE_CAPACITY {
        queue.pop_front();
    }
    queue.push_back(notice);
}

/// The name a notice's kind is logged under.
fn notice_kind_log_name(notice: &HostNotice) -> &'static str {
    match notice {
        HostNotice::ModuleFailed { .. } => "module_failed",
        HostNotice::Reloaded { .. } => "reloaded",
        HostNotice::PatchApplied { .. } => "patch_applied",
        HostNotice::PatchFailed { .. } => "patch_failed",
        HostNotice::RolledBack { .. } => "rolled_back",
    }
}

/// Takes every queued notice, oldest first.
///
/// The editor executable calls this once per frame and folds the result into
/// its notice resource. A host with no editor attached never calls it, so the
/// queue fills up to its cap and stays there.
pub fn take() -> Vec<HostNotice> {
    let mut queue = NOTICE_QUEUE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    queue.drain(..).collect()
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes the two tests that share the process-global queue.
    ///
    /// `cargo test` runs them as parallel threads in one process, so without
    /// this lock the capacity test's pushes can slip between the order test's
    /// two pushes and its drain, and both tests fail. A runner that gives
    /// every test its own process, such as nextest, never hits the race.
    static QUEUE_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// The queue hands notices back in order and empties on take.
    #[test]
    fn notices_drain_in_order() {
        let _serialized = QUEUE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Clear whatever an earlier test left in the shared queue, so this
        // test starts from zero.
        let _ = take();
        push(HostNotice::Reloaded {
            module: "pill_spline".to_string(),
            build_ms: 120,
        });
        push(HostNotice::PatchApplied {
            function: "orbit".to_string(),
            milliseconds: 12.5,
        });
        let taken_notices = take();
        assert_eq!(taken_notices.len(), 2);
        assert!(matches!(
            &taken_notices[0],
            HostNotice::Reloaded { module, .. } if module == "pill_spline"
        ));
        assert!(matches!(
            &taken_notices[1],
            HostNotice::PatchApplied { function, .. } if function == "orbit"
        ));
        assert!(take().is_empty());
    }

    /// The queue drops the oldest notices past its cap, never the newest.
    #[test]
    fn the_cap_drops_oldest_first() {
        let _serialized = QUEUE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Start from an empty queue, as the first test does.
        let _ = take();
        for index in 0..NOTICE_CAPACITY + 4 {
            push(HostNotice::Reloaded {
                module: format!("module_{index}"),
                build_ms: 0,
            });
        }
        let taken_notices = take();
        assert_eq!(taken_notices.len(), NOTICE_CAPACITY);
        let Some(HostNotice::Reloaded { module, .. }) = taken_notices.first() else {
            panic!("first notice is a reload");
        };
        assert_eq!(module, "module_4");
    }
}
