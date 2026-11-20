//! The requests one shell pass raises for the editor executable.
//!
//! # Responsibilities
//!
//! - [`ShellRequests`]: the one-shot requests a pass raises for the
//!   executable - a Problems row to open, an undo or redo, a dock tab to
//!   show - taken in one call at the frame boundary.
//! - [`HistoryRequest`]: which history walk, undo or redo, a request asks for.
//!
//! # Design
//!
//! The shell module and the executable are separate DLLs, so the requests
//! cross as plain data: a row index, a [`HistoryRequest`] and a tab, all
//! defined here in the shared `dylib`. The pass writes them while it draws.
//! `settle_pass` then moves the whole value onto
//! [`crate::boundary::EguiShellFrame`], and the executable takes it from
//! there. The frame already carries the shell's quit request and a
//! dragged-out asset, so these requests travel the same way.
//!
//! The caller takes the whole value in one call, and that is the contract:
//! [`ShellRequests::drain`] hands back every request and leaves the resource
//! empty, so a caller cannot answer one and silently leave another behind.
//!
//! A request that must outlive the pass that raised it does not belong here.
//! The scripted channel's `panel window` request stays in the shell session
//! until the panel it names draws. A dock edit the pass cannot make while the
//! dock is borrowed goes into the shell module's `FrameRequests`, which
//! `settle_pass` applies after the pass.

// Current crate
use crate::layout::EditorTab;

/// The one-shot requests one shell pass raises for the editor executable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellRequests {
    /// The Problems row a click asked to open, held as an index into the
    /// editor's problem list. Only the executable can turn the row into a
    /// `file:line` and open it.
    pub problem_open: Option<usize>,
    /// The undo and redo steps the menu bar asked for, oldest first. Each
    /// entry undoes or redoes one recorded step, and the executable applies
    /// them in the order they sit here.
    pub history: Vec<HistoryRequest>,
    /// The dock tab a menu item asked to show. A tab the dock no longer holds
    /// is re-docked by the executable rather than dropped.
    pub pending_tab: Option<EditorTab>,
}

impl ShellRequests {
    /// Takes every request at once and leaves the resource empty.
    ///
    /// One call, one boundary: the caller sees every request the pass raised
    /// and answers them all in one phase, so it cannot answer one request and
    /// silently leave another behind.
    #[must_use]
    pub fn drain(&mut self) -> Self {
        std::mem::take(self)
    }

    /// Returns `true` when the pass raised no request.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.problem_open.is_none() && self.history.is_empty() && self.pending_tab.is_none()
    }
}

/// One undo or redo the menu bar asked for, applied by the executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryRequest {
    /// Undoes the newest recorded step.
    Undo,
    /// Redoes the newest undone step.
    Redo,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drain_returns_every_request_and_clears_them_all() {
        let mut requests = ShellRequests {
            problem_open: Some(3),
            history: vec![HistoryRequest::Undo, HistoryRequest::Redo],
            pending_tab: Some(EditorTab::Assets),
        };
        let drained = requests.drain();
        assert_eq!(drained.problem_open, Some(3));
        assert_eq!(
            drained.history,
            vec![HistoryRequest::Undo, HistoryRequest::Redo]
        );
        assert_eq!(drained.pending_tab, Some(EditorTab::Assets));
        assert!(requests.is_empty());
        assert_eq!(requests, ShellRequests::default());
    }

    #[test]
    fn an_empty_value_is_empty() {
        assert!(ShellRequests::default().is_empty());
    }
}
