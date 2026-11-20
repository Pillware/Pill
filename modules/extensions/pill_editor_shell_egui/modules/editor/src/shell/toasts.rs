//! The toast stack: what the host or the editor did, drawn over the editor
//! for a few seconds.
//!
//! # Responsibilities
//!
//! - [`push_notices`]: queue each notice the stack has not shown yet as a
//!   toast, keeping the newest [`TOASTS_MAX`] of them.
//! - [`draw_toasts`]: drop expired toasts, then draw the rest in the window's
//!   top-left corner and log each one once. It also publishes every row's
//!   rectangle, so a click on a toast is never a scene pick.
//!
//! # Design
//!
//! Each frame, the executable folds the host's
//! `pill_development_host::HostNotice`s into the [`EditorNotices`] resource.
//! This module reads that resource and draws the events as toasts. A toast
//! is identified by its notice sequence number, so a frame that draws the
//! same events again queues no line twice and logs no line twice.
//! [`ToastSession`] lives in the session, which survives a shell reload, so a
//! reload keeps the stack on screen.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
// Deliberately unused in this compilation: a patch crate replays this
// file's opening, and the first sibling this body names resolves through it.
#[allow(unused_imports)]
use pill_editor_shell_egui_editor::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// Where the stack's first row sits, just below the menu bar.
const TOAST_TOP: f32 = menubar::MENU_BAR_HEIGHT + 6.0;
/// The stack's inset from the window's left edge.
const TOAST_LEFT: f32 = 8.0;
/// One row's width, in logical points.
const TOAST_WIDTH: f32 = 420.0;
/// One row's height, in logical points.
const TOAST_HEIGHT: f32 = 24.0;
/// The gap between two rows, in logical points.
const TOAST_GAP: f32 = 4.0;

// =============================================================================
// Free Functions
// =============================================================================

/// Queues every notice the stack has not shown yet as a toast.
///
/// The newest notice is drawn at the top of the stack, so the latest activity
/// is the first row a reader sees.
pub fn push_notices(state: &mut EguiShellState, notices: &EditorNotices) {
    // Step 1: queue each event the stack has not seen yet.
    let mut newest_seq = state.toasts.last_seq;
    for event in &notices.events {
        let fresh = state.toasts.last_seq.is_none_or(|last| event.seq > last);
        if !fresh {
            continue;
        }
        if state.toasts.toasts.len() >= TOASTS_MAX {
            state.toasts.toasts.pop_front();
        }
        state.toasts.toasts.push_back(Toast {
            seq: event.seq,
            level: event.level,
            text: event.text.clone(),
            expires_at_frame: state.frame_index + TOAST_FRAMES,
        });
        pill_core::info!(
            target: telemetry_target::EDITOR,
            seq = event.seq,
            level = event.level.name(),
            "[editor] toast queued"
        );
        newest_seq = Some(event.seq);
    }
    // Step 2: remember the newest id, so the next frame starts after it.
    state.toasts.last_seq = newest_seq;
}

/// Draws the live toasts and publishes their rectangles.
///
/// The shell calls it from inside its egui pass, after the dock: a toast
/// floats over whatever it covers, and a click on one belongs to the UI.
pub fn draw_toasts(
    toasts: &mut ToastSession,
    frame_index: u64,
    context: &egui::Context,
    overlay_regions: &mut Vec<ShellRect>,
) {
    // Step 1: drop the toasts whose time is up; with nothing left to draw,
    // forget the ids already logged too.
    toasts
        .toasts
        .retain(|toast| toast.expires_at_frame > frame_index);
    if toasts.toasts.is_empty() {
        toasts.logged.clear();
        return;
    }
    // Step 2: copy the rows out, newest first. The loop below updates the
    // log bookkeeping, and the reverse order puts the newest row at the top.
    let rows: Vec<(u64, NoticeLevel, String)> = toasts
        .toasts
        .iter()
        .rev()
        .map(|toast| (toast.seq, toast.level, toast.text.clone()))
        .collect();
    let mut on_screen_seqs = Vec::with_capacity(rows.len());
    // Step 3: draw each row, publish its rectangle and log it once.
    for (index, (seq, level, text)) in rows.into_iter().enumerate() {
        let top = TOAST_TOP + index as f32 * (TOAST_HEIGHT + TOAST_GAP);
        let area = egui::Area::new(egui::Id::new(("editor-toast", seq)))
            // The middle layer, not the foreground one: a menu popup lives in
            // the foreground layer and has to stay above the stack. A toast
            // drawn after the menu bar in the same layer would cover an open
            // menu's rows and take the click meant for them.
            .order(egui::Order::Middle)
            .fixed_pos(egui::pos2(TOAST_LEFT, top))
            .show(context, |ui| {
                egui::Frame::new()
                    .fill(style::panel())
                    .stroke(egui::Stroke::new(1.0, style::line()))
                    .inner_margin(egui::Margin::symmetric(8, 2))
                    .show(ui, |ui| {
                        ui.set_width(TOAST_WIDTH);
                        // A thin bar in the level's colour down the left edge.
                        let bar = egui::Rect::from_min_size(
                            ui.max_rect().left_top(),
                            egui::vec2(3.0, ui.max_rect().height()),
                        );
                        ui.painter().rect_filled(
                            bar,
                            egui::CornerRadius::ZERO,
                            level_colour(level),
                        );
                        ui.horizontal(|ui| {
                            ui.colored_label(level_colour(level), level.name());
                            ui.label(egui::RichText::new(&text).color(style::text()));
                        });
                    });
            });
        let rect = area.response.rect;
        overlay_regions.push(ShellRect {
            x: rect.min.x,
            y: rect.min.y,
            width: rect.width(),
            height: rect.height(),
        });
        on_screen_seqs.push(seq);
        // Log each toast once, not once per frame. The suite reads the
        // level, the text and where the row was placed.
        if !toasts.logged.contains(&seq) {
            toasts.logged.push(seq);
            pill_core::info!(
                target: telemetry_target::EDITOR,
                level = level.name(),
                seq,
                text = text.as_str(),
                x = rect.min.x,
                y = rect.min.y,
                width = rect.width(),
                height = rect.height(),
                "[editor] toast"
            );
        }
    }
    // Step 4: keep only the logged ids of toasts still on screen.
    toasts.logged.retain(|seq| on_screen_seqs.contains(seq));
}

/// Returns the colour that stands for one notice level.
pub fn level_colour(level: NoticeLevel) -> egui::Color32 {
    match level {
        NoticeLevel::Error => style::log_error(),
        NoticeLevel::Warning => style::log_warn(),
        NoticeLevel::Note => style::log_info(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh shell state at frame 100, so the expiry assertions have a
    /// known start.
    fn session() -> EguiShellState {
        let mut state = EguiShellState::new();
        state.frame_index = 100;
        state
    }

    #[test]
    fn each_notice_is_queued_once_and_expires() {
        let mut state = session();
        let notices = EditorNotices {
            events: vec![Notice {
                seq: 0,
                level: NoticeLevel::Note,
                text: "spline reloaded in 120 ms".to_string(),
            }],
            next_event_seq: 1,
            ..Default::default()
        };
        push_notices(&mut state, &notices);
        assert_eq!(state.toasts.toasts.len(), 1);
        let toast = &state.toasts.toasts[0];
        assert_eq!(toast.expires_at_frame, 100 + TOAST_FRAMES);
        // Pushing the same list again queues nothing, because the notice's
        // id was already seen.
        push_notices(&mut state, &notices);
        assert_eq!(state.toasts.toasts.len(), 1);
    }

    #[test]
    fn the_stack_keeps_the_newest_toasts() {
        let mut state = session();
        let notices = EditorNotices {
            events: (0..(TOASTS_MAX as u64 + 2))
                .map(|seq| Notice {
                    seq,
                    level: NoticeLevel::Note,
                    text: format!("notice {seq}"),
                })
                .collect(),
            next_event_seq: TOASTS_MAX as u64 + 2,
            ..Default::default()
        };
        push_notices(&mut state, &notices);
        assert_eq!(state.toasts.toasts.len(), TOASTS_MAX);
        // The two oldest are the ones that dropped off.
        assert_eq!(state.toasts.toasts[0].seq, 2);
    }
}
