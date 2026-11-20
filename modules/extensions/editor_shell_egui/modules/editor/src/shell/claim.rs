//! The shell's claim on egui's shared contexts.
//!
//! # Responsibilities
//!
//! - [`claim_shell_context`]: the per-generation wrapper `run_frame` calls
//!   before its pass. It applies the once-per-generation rule kept in this
//!   file, and gets the actual clearing from the session that owns the
//!   contexts.
//! - [`claim_context_once`]: the same rule for a single context, with the
//!   flag passed in instead of read from this file's static.
//!
//! # Design
//!
//! The egui `Context` lives in the session, a world resource whose type the
//! shared model crate defines, and the frontend that keeps the world never
//! unloads. So the context survives a shell reload. egui's `Memory::data` map
//! can still hold values the previous generation created: a scroll position,
//! a text editor's cursor, the dock's own saved state. Such a value also
//! carries drop code and a vtable from the generation that created it. After
//! a reload those point into an image that is no longer loaded, so touching
//! the value is a use-after-unmap.
//!
//! The first frame of every generation therefore clears every context the
//! session owns, main and popped-out alike
//! ([`EguiShellState::claim_generation`](pill_editor_core::session::EguiShellState::claim_generation)).
//! Widget-local state that egui keeps in that map goes with it. That loss is
//! acceptable, because keeping such state is not worth the risk of reading a
//! pointer into an unloaded image. Long-lived UI state lives in the session
//! itself, a world resource the shared model crate defines, which is what
//! makes it safe across generations.
//!
//! The "already claimed" flag is a `static` of this crate, and a static lives
//! in the module's own image. Every generation therefore starts unclaimed,
//! and later frames of the same generation do nothing. That is also why the
//! flag cannot move to the model with the clearing: the model outlives the
//! generation whose claim it would be.

// Standard library
use std::sync::atomic::{AtomicBool, Ordering};

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
// Deliberately unused in this build. A patch crate replays this file's
// opening `use` lines, so a patched body that calls a sibling in the shell
// module finds it through this glob.
#[allow(unused_imports)]
use pill_editor_shell_egui_editor::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// Whether this shell generation has cleared egui's temporary data yet.
///
/// The flag lives in the module's own image, so a reloaded shell always
/// starts from `false`. That is what makes the claim run exactly once per
/// generation.
static CONTEXT_CLAIMED: AtomicBool = AtomicBool::new(false);

// =============================================================================
// Free Functions
// =============================================================================

/// Claims every egui context the session owns for this shell generation.
///
/// `run_frame` calls it before the egui pass, so no value the previous
/// generation left behind is still there when the new generation first
/// draws. Returns what was cleared on that first frame, and `None` on every
/// later frame of the generation.
pub fn claim_shell_context(
    session: &mut pill_editor_core::session::EguiShellState,
) -> Option<pill_editor_core::session::ContextClaim> {
    if CONTEXT_CLAIMED.swap(true, Ordering::AcqRel) {
        return None;
    }
    let context_claim = session.claim_generation();
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entries = context_claim.main_entries,
        windows = session.popout_contexts.len(),
        cleared = context_claim.popout_windows,
        "[editor] shell context claimed"
    );
    Some(context_claim)
}

/// Applies the once-per-generation rule to one context.
///
/// The flag is a parameter so a test can pass its own. Two tests sharing one
/// process-wide static would make each other's expectations depend on the
/// order they run in. Returns `true` when this call did the clearing and
/// `false` when the flag was already set.
pub fn claim_context_once(claimed: &AtomicBool, context: &egui::Context) -> bool {
    if claimed.swap(true, Ordering::AcqRel) {
        return false;
    }
    // Read the count and clear the map in one borrow of egui's memory, so
    // the count is exactly what was cleared and nothing can be added in
    // between.
    let entry_count = context.memory_mut(|memory| {
        let entry_count_before_clear = memory.data.len();
        memory.data.clear();
        entry_count_before_clear
    });
    pill_core::info!(
        target: telemetry_target::EDITOR,
        entries = entry_count,
        "[editor] shell context claimed"
    );
    true
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A value a previous generation could have left in the map. It is
    /// defined here so its drop code belongs to this test's image.
    #[derive(Clone)]
    struct StaleRow {
        /// The text the row carries.
        label: String,
    }

    /// The first claim of a generation drops what the previous one left, and
    /// a later frame of that same generation leaves the map alone.
    #[test]
    fn a_previous_generations_value_is_cleared_on_the_first_claim() {
        let context = egui::Context::default();
        let id = egui::Id::new("stale-row");
        // Store a value the way a previous generation would have.
        context.memory_mut(|memory| {
            // egui-state-ok: the previous generation's value
            memory.data.insert_temp(
                id,
                StaleRow {
                    label: "from the old image".to_string(),
                },
            );
        });
        let claimed = AtomicBool::new(false);

        assert!(
            claim_context_once(&claimed, &context),
            "the first claim of a generation runs"
        );
        assert_eq!(
            context.memory(|memory| memory.data.count::<StaleRow>()),
            0,
            "the previous generation's value is gone"
        );

        // The new generation stores its own value. The next frame's claim
        // must not throw it away.
        context.memory_mut(|memory| {
            // egui-state-ok: the value the run must keep
            memory.data.insert_temp(
                id,
                StaleRow {
                    label: "from the new image".to_string(),
                },
            );
        });
        assert!(
            !claim_context_once(&claimed, &context),
            "a later frame of the same generation does not claim again"
        );
        let kept_row = context
            // egui-state-ok: the value the claim kept
            .memory_mut(|memory| memory.data.get_temp::<StaleRow>(id))
            .expect("the generation's own value survives");
        assert_eq!(kept_row.label, "from the new image");
    }

    /// A fresh context holds nothing to clear, and the claim still marks the
    /// generation.
    #[test]
    fn claiming_an_empty_context_still_marks_the_generation() {
        let context = egui::Context::default();
        let claimed = AtomicBool::new(false);
        // An empty map is still claimed, so the generation gets marked, and
        // the map holds nothing before the claim and nothing after it.
        let entry_count_before_claim = context.memory(|memory| memory.data.len());
        assert!(claim_context_once(&claimed, &context));
        assert_eq!(
            entry_count_before_claim, 0,
            "a fresh context holds no temporary data"
        );
        assert!(context.memory(|memory| memory.data.is_empty()));
    }
}
