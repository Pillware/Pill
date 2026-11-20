//! What the host and the editor did, as plain data for the egui shell: toasts
//! and problems.
//!
//! # Responsibilities
//!
//! - Define [`EditorNotices`]: the resource the executable writes before the
//!   shell's pass. It holds the toast events and the Problems rows the
//!   executable fills it with.
//! - Define [`Notice`], [`NoticeLevel`] and [`ProblemRow`]: owned strings only.
//! - Name a level the way the toast log lines do, which the suites match
//!   ([`level_name`]).
//!
//! # Design
//!
//! `pill_development_host::HostNotice` says what the host did: a module
//! reloaded or failed, a patch went live or failed, a function rolled back.
//! The executable turns each notice into a toast line, and a failure also into
//! a Problems row for the module or function that failed. The shell only reads
//! this data. The host owns the notice queue, the executable owns this
//! resource, and the shell draws from it. That is why the resource lives in
//! this shared `dylib` rather than in the reloadable shell module: its identity
//! survives a shell reload, and no field may point into a module image that the
//! reload unmaps.

// External crates
use pill_engine::Resource;

// Current crate
use crate::problems::Problem;

// =============================================================================
// Types
// =============================================================================

/// How serious one toast is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NoticeLevel {
    /// A build or a patch failed; the previous generation keeps running.
    Error,
    /// Something worth a second look: a rollback, a refused drop.
    Warning,
    /// A plain notice: a reload finished, a patch went live.
    Note,
}

impl NoticeLevel {
    /// The level's wire name, as the toast log lines print it.
    pub fn name(self) -> &'static str {
        level_name(self)
    }
}

/// One toast line: something the editor or the host did, shown briefly over
/// the editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// An id that increases for the life of the session. The shell's toast
    /// list keys on it, so a redrawn frame never queues a line it shows.
    pub seq: u64,
    /// How serious it is.
    pub level: NoticeLevel,
    /// The line to show.
    pub text: String,
}

/// One Problems row: the module or function whose build, load or patch
/// reported it, and the diagnostic itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProblemRow {
    /// The module or function that reported it.
    pub module: String,
    /// The diagnostic, with the location when the compiler printed one.
    pub problem: Problem,
}

/// The notices the executable publishes for the shell when the host or the
/// editor has something to say.
#[derive(Clone, Debug, Default)]
pub struct EditorNotices {
    /// How many times the executable has republished this resource; it only
    /// moves when `problems` or `events` changed.
    pub revision: u64,
    /// The current problems, one row per diagnostic, unsorted: `module` names
    /// what reported the row. A module that reloads cleanly drops its rows.
    pub problems: Vec<ProblemRow>,
    /// The toast events; the shell reads them in order and skips the ones it
    /// has already shown, keyed by [`Notice::seq`].
    pub events: Vec<Notice>,
    /// The id the next entry added to `events` takes.
    pub next_event_seq: u64,
}

impl Resource for EditorNotices {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

impl EditorNotices {
    /// Adds one toast event and returns its id.
    ///
    /// The sequence it takes is unique for the life of the session, which is
    /// what lets the shell show each line once without storing the text.
    pub fn push_event(&mut self, level: NoticeLevel, text: String) -> u64 {
        let seq = self.next_event_seq;
        self.next_event_seq = seq.wrapping_add(1);
        self.events.push(Notice { seq, level, text });
        seq
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// The wire name of a level: `error`, `warning` or `info`.
pub fn level_name(level: NoticeLevel) -> &'static str {
    match level {
        NoticeLevel::Error => "error",
        NoticeLevel::Warning => "warning",
        NoticeLevel::Note => "info",
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_name_themselves_for_the_log() {
        assert_eq!(NoticeLevel::Error.name(), "error");
        assert_eq!(NoticeLevel::Warning.name(), "warning");
        assert_eq!(NoticeLevel::Note.name(), "info");
        // The method delegates to the free function, so the two can never
        // name a level differently.
        assert_eq!(level_name(NoticeLevel::Note), NoticeLevel::Note.name());
    }
}
