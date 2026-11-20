//! The windows that module panels ask the shell to show.
//!
//! # Responsibilities
//!
//! - [`panel_windows`]: draw every open panel window. Each window calls the
//!   draw function of the panel that opened it and passes the window's id, so
//!   a panel's dialog body and its tab body are the same code.
//! - Answer a modal with Enter or Escape, or with its Apply and Cancel
//!   buttons, and leave the answer for the panel.
//! - Close the windows of a module whose panels are gone, on every pass, so a
//!   window is never drawn through a draw function from an image that has
//!   unloaded.
//! - Publish each non-modal window's rectangle into the frame's overlay
//!   regions, so a click on a window belongs to the UI and is never a scene
//!   pick. A modal needs no rectangle: it takes those clicks itself.
//!
//! # Design
//!
//! The shell owns the window (its position, its lifetime, its modality) and
//! the panel owns the content. The windows live in [`PanelWindowSession`], a
//! session resource, so a shell reload keeps them open. A window closes when
//! the user closes it, when a modal is answered, when the owning module
//! unloads and its panels are gone, or when a new window opens past
//! [`PANEL_WINDOWS_MAX`]. A panel that wants a closed window back asks for it
//! again on its next draw.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
// Unused in this build on purpose: a patch crate replays this file's opening
// `use` lines, and the file's own public items resolve through this glob
// there.
#[allow(unused_imports)]
use pill_editor_shell_egui_editor::shell::*;

/// The data a module panel reads while it draws, collected once per pass.
pub struct PanelFeed {
    /// The world's entities, as the panels see them.
    pub entities: Vec<PanelEntityRow>,
    /// The selected entity's components, when one is selected.
    pub detail: Option<PanelEntityDetail>,
    /// The editor's smoothed frame rate.
    pub frames_per_second: f32,
}

/// Draw every open panel window.
///
/// A non-modal window's rectangle goes into `overlay_regions`, so a click on
/// the window belongs to the UI. A modal publishes no rectangle: it takes the
/// editor's input itself.
///
/// `panel_commands` collects what the windows ask for; the caller forwards the
/// queue to the executable at the frame boundary. A request queued from inside
/// a window has no slot to attribute it to, so an open or close request from a
/// window is ignored and only a tab body's own request takes effect.
pub fn panel_windows(
    context: &egui::Context,
    window_session: &mut PanelWindowSession,
    registry: &PanelRegistry,
    panel_budget: &mut PanelBudget,
    panel_feed: &PanelFeed,
    panel_commands: &mut Vec<PanelCommand>,
    overlay_regions: &mut Vec<ShellRect>,
) {
    // Step 1: Drop the windows whose panel's module has unloaded. Their draw
    // function is code from an image that has unloaded, so there is nothing
    // safe to call.
    window_session
        .windows
        .retain(|window| registry.alive(window.slot));
    if window_session.windows.is_empty() {
        return;
    }
    // Step 2: Draw each remaining window. The list is copied first, because
    // drawing can close a window (an answer or its close button) while the
    // copy is being walked.
    let open_windows = window_session.windows.clone();
    for window in &open_windows {
        let Some(draw) = registry.draw_of(window.slot) else {
            continue;
        };
        let answer =
            window_session
                .answer
                .as_ref()
                .map(|answer| pill_editor_core::panels::PanelAnswer {
                    window: answer.window.clone(),
                    accepted: answer.accepted,
                });
        let mut draw_panel_body = |ui: &mut egui::Ui, panel_commands: &mut Vec<PanelCommand>| {
            let mut panel_context = PanelContext {
                frames_per_second: panel_feed.frames_per_second,
                selected_entity: panel_feed.detail.as_ref().map(|detail| detail.entity),
                entities: panel_feed.entities.as_slice(),
                detail: panel_feed.detail.as_ref(),
                window: Some(window.id.as_str()),
                requested_window: None,
                answer: answer.as_ref(),
                commands: panel_commands,
            };
            let started_at = pill_core::platform::Instant::now();
            draw(ui, &mut panel_context);
            let elapsed_microseconds = started_at.elapsed().as_micros() as f32;
            panel_budget.record(window.slot, elapsed_microseconds);
        };
        if window.modal {
            // A modal takes the editor's input while it is open: egui's modal
            // layer darkens what is behind it and handles the clicks first, so
            // neither the dock nor the chips it published can be clicked. That
            // is also why this path publishes no rectangle for the window.
            let modal_answer =
                egui::Modal::new(egui::Id::new(("pill_editor.panel_modal", &window.id)))
                    .show(context, |ui| {
                        ui.set_width(window.width);
                        ui.heading(window.title.as_str());
                        ui.separator();
                        draw_panel_body(ui, panel_commands);
                        ui.separator();
                        // Plain egui buttons, with Apply before Cancel.
                        let button_clicks = ui
                            .horizontal(|ui| {
                                let apply_clicked = ui.button("Apply").clicked();
                                let cancel_clicked = ui.button("Cancel").clicked();
                                (apply_clicked, cancel_clicked)
                            })
                            .inner;
                        let key_presses = ui.input(|input| {
                            (
                                input.key_pressed(egui::Key::Enter),
                                input.key_pressed(egui::Key::Escape),
                            )
                        });
                        if button_clicks.0 || key_presses.0 {
                            return Some(true);
                        }
                        if button_clicks.1 || key_presses.1 {
                            return Some(false);
                        }
                        None
                    })
                    .inner;
            if let Some(accepted) = modal_answer {
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    window = window.id.as_str(),
                    accepted,
                    "[editor] panel modal answered"
                );
                window_session.answer = Some(PanelWindowAnswer {
                    window: window.id.clone(),
                    source: window.source.clone(),
                    accepted,
                });
                window_session.close(&window.source, &window.id);
            }
            continue;
        }
        let mut open = true;
        let response = egui::Window::new(window.title.as_str())
            .id(egui::Id::new(("pill_editor.panel_window", &window.id)))
            .open(&mut open)
            .default_width(window.width)
            .default_height(window.height)
            .resizable(true)
            .show(context, |ui| {
                ui.set_width(window.width);
                draw_panel_body(ui, panel_commands);
            });
        if let Some(response) = response {
            // Publish the window's rectangle: a click on it belongs to the UI,
            // never to a scene view behind it.
            let rect = response.response.rect;
            overlay_regions.push(ShellRect {
                x: rect.min.x,
                y: rect.min.y,
                width: rect.width(),
                height: rect.height(),
            });
        }
        // The title bar's close button is what turns `open` false.
        if !open {
            window_session.close(&window.source, &window.id);
        }
    }
}
