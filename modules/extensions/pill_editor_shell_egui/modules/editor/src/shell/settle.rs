//! The stage that runs after a shell pass.
//!
//! # Responsibilities
//!
//! - [`FrameRequests`]: the one value a pass's tabs and menus write where they
//!   raise a deferred request - a dock edit they cannot make while the dock
//!   is borrowed, a drag that left the windows, a quit.
//! - [`settle_pass`]: applies those requests in the order the pass requires,
//!   and returns the frame the pass produced.
//!
//! # Design
//!
//! The order of the steps is the contract, which is why this stage is one
//! function instead of blocks scattered through `run_frame`:
//!
//! 1. a tab that left the popout set loses its context first. A context that
//!    nothing draws into would otherwise be cleared on the first frame of
//!    every generation and kept for the session's life.
//! 2. each remaining popped-out tab runs its own pass with the same viewer,
//!    because its window has its own widget state and input record.
//! 3. a tab menu's pop-out runs after those passes, so a tab that asked to
//!    leave is drawn in the dock for the rest of this pass.
//! 4. a re-dock runs after the pop-out, because both edit the dock tree.
//! 5. a new scene view is docked after both, and the executable activates it
//!    through its own path, so the new tab is at the front of its group the
//!    moment it appears.
//! 6. a panel window's answer is cleared only after a pass that read it.
//! 7. the scripted window request is cleared only when the panel it named
//!    drew.
//! 8. a panel that went over its frame budget gets its toast after every pass
//!    that timed a panel, because the budget is only complete then.
//! 9. the asks the pass raised for the executable - a Problems row, undo/redo
//!    and a dock tab - move onto the frame in one take, beside the drag path
//!    and the menu's quit.
//!
//! Only step 2 draws, and it draws inside a popped-out tab's own window.
//! Every other step edits state that the next frame reads, which is what
//! makes the stage safe to run at the boundary.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
// Deliberately unused in this compilation: a patch crate replays this file's
// opening, and the first sibling this body names resolves through it.
#[allow(unused_imports)]
use pill_editor_shell_egui_editor::shell::*;

// =============================================================================
// Types
// =============================================================================

/// The requests one shell pass could not act on, because the dock and its
/// tabs were borrowed while it drew.
///
/// Each field holds one request: a tab that asked to pop out or come back, a
/// menu item that asked for one more scene view or a quit, a tile drag that
/// left the windows. The tabs and menus write the value where they raise the
/// request, through `EditorTabViewer::deferred`, and this stage reads it back
/// from there. So a request has one home, and nothing is copied between the
/// pass and the stage that applies it.
#[derive(Default)]
pub struct FrameRequests {
    /// The asset path a tile drag handed over. The shell cannot start an OLE
    /// drag itself (a drag the OS carries into another application), so the
    /// executable does it.
    pub drag_out: Option<String>,
    /// The tab a tab menu asked to move out into its own window.
    pub tab_popout: Option<EditorTab>,
    /// The floating surface whose tabs a tab menu asked to bring back into
    /// the dock.
    pub tab_redock: Option<SurfaceIndex>,
    /// Whether the Window menu asked for one more scene view.
    pub new_scene_view: bool,
    /// Whether a menu asked the editor to quit. The executable saves the dock
    /// layout and then ends the run.
    pub quit_requested: bool,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Finishes one shell pass: applies the requests it deferred and returns the
/// frame the pass produced.
///
/// The order of the steps is the contract; the module docs say why each step
/// sits where it does. `viewer` is the pass's own viewer: the popped-out
/// windows draw through it, so it is passed whole instead of field by field.
/// `frame_index` is the pass number a panel toast counts its lifetime from.
// The arguments are distinct borrows of the shell's state, taken apart for
// this one call. Grouping them would name a type that exists only to satisfy
// the lint, and would hide which parts of the state this stage changes.
#[allow(clippy::too_many_arguments)]
pub fn settle_pass(
    dock: &mut pill_editor_core::layout::EditorDock,
    popout_contexts: &mut std::collections::BTreeMap<EditorTab, egui::Context>,
    popout_inputs: &pill_editor_core::boundary::EguiPopoutInputs,
    windows: &mut PanelWindowSession,
    window_ping: &mut Option<(u16, String)>,
    toasts: &mut ToastSession,
    panels: &PanelRegistry,
    frame_index: u64,
    viewer: &mut EditorTabViewer<'_>,
    mut shell_frame: EguiShellFrame,
) -> EguiShellFrame {
    // Step 1: keep the context of a tab only while it is popped out.
    popout_contexts.retain(|tab, _| dock.is_popped_out(*tab));
    // Step 2: one more pass per popped-out tab, each in its own context, so
    // it keeps its own widget state and its own input record.
    shell_frame.popout_frames = pill_editor_shell_egui_editor::shell::run_popout_passes(
        dock,
        popout_contexts,
        popout_inputs,
        viewer,
    );
    // Step 3: the tab a tab menu asked to pop out. The dock was borrowed
    // while that menu drew, so the move waits until now, after the passes
    // that draw the tab in the dock.
    if let Some(tab) = viewer.deferred.tab_popout {
        if dock.pop_out(tab) {
            pill_core::info!(
                target: telemetry_target::EDITOR,
                tab = tab.name().as_str(),
                "[editor] popout moved"
            );
        }
    }
    // Step 4: a re-dock follows the pop-out, because both change the dock
    // tree, and neither may change it while a widget draws from it.
    if let Some(surface) = viewer.deferred.tab_redock {
        pill_editor_shell_egui_editor::shell::apply_tab_redock(dock.state_mut(), surface);
    }
    // Step 5: the Window menu's "New Scene View". It waits for the same
    // reason, and the executable activates the tab through its own path.
    if viewer.deferred.new_scene_view {
        let tab = dock.next_scene_view();
        if dock.contains(tab) {
            // Already docked: bring that tab to the front instead of adding
            // a view.
            viewer.requests.pending_tab = Some(tab);
        } else if dock.can_open_scene_view() {
            dock.state_mut().push_to_focused_leaf(tab);
            pill_core::info!(
                target: telemetry_target::EDITOR,
                tab = tab.name().as_str(),
                view = tab.scene_view().unwrap_or_default(),
                "[editor] scene view added"
            );
            viewer.requests.pending_tab = Some(tab);
        } else {
            pill_core::warn!(
                target: telemetry_target::EDITOR,
                limit = pill_editor_core::boundary::SCENE_VIEWS_MAX,
                "[editor] scene view refused: the view limit is reached"
            );
        }
    }
    // Step 6: the pass was offered the modal's answer, so the next pass must
    // not act on it again. The copy lives on the viewer, which is what read
    // it, so no second flag has to agree with it.
    if viewer.panel_answer.is_some() {
        windows.answer = None;
    }
    // Step 7: the panel a scripted window request named drew this pass, so
    // the request is spent.
    if viewer.pending_panel_window_used {
        *window_ping = None;
    }
    // Step 8: a panel the budget marked as too slow gets one toast, named
    // after its tab so the person can tell which panel it is. The budget is
    // complete only after the passes that timed the panels, and `toasted`
    // keeps the line to one.
    for entry in &mut viewer.panel_budget.entries {
        if !entry.warned || entry.toasted {
            continue;
        }
        entry.toasted = true;
        let title = panels.tab_title(entry.slot);
        let text = format!(
            "Panel {} is over its frame budget ({:.1} ms)",
            title,
            entry.micros / 1000.0
        );
        toasts.push_local(NoticeLevel::Warning, text, frame_index);
    }
    // Step 9: the asks this pass raised for the executable leave on the
    // frame. One take moves the whole value, so no request is left behind by
    // a caller that answered one and forgot another.
    shell_frame.requests = std::mem::take(viewer.requests);
    // The tile drag's path and the menu's quit travel on the same frame.
    shell_frame.drag_out = viewer.deferred.drag_out.take();
    shell_frame.quit_requested = viewer.deferred.quit_requested;
    shell_frame
}
