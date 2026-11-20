//! The command palette: a searchable popup over every registered command.
//!
//! # Responsibilities
//!
//! - Draw the popup while it is open: the query field, the matching rows and
//!   the row the arrow keys select.
//! - Run the selected row through [`run_menu_action`], the same call a menu
//!   entry makes, so a palette run and a menu click run the same code.
//! - Report the popup's rectangle as an overlay region, so a click on the
//!   popup never becomes a scene pick.
//!
//! # Design
//!
//! The popup's state lives in `PaletteState`, which the shared session keeps.
//! The scripted channel sets a query, a run request or a close on it; this
//! file draws the result and writes back what the user picked. That is why a
//! test can drive the palette without sending key presses.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
use pill_editor_shell_egui_editor::shell::*;

/// The largest number of matching rows the popup draws.
pub const PALETTE_ROWS: usize = 12;

/// The distance from the top of the editor window to the popup's top edge, in
/// logical points. It keeps the popup clear of the menu bar.
pub const PALETTE_TOP: f32 = 72.0;

/// The popup's width, in logical points.
pub const PALETTE_WIDTH: f32 = 360.0;

/// The commands whose titles contain `query`, in the order the list has them.
///
/// The match ignores case and the spaces around `query`, so an empty query
/// matches every command. This is the title filter only: the window drops the
/// rows that have no action, which needs the frame's snapshot and selection.
pub fn matches<'a>(commands: &'a [CommandView], query: &str) -> Vec<&'a CommandView> {
    let needle = query.trim().to_lowercase();
    commands
        .iter()
        .filter(|command| needle.is_empty() || command.title.to_lowercase().contains(&needle))
        .collect()
}

/// Draw the palette when it is open and run the row it was asked to run.
pub fn palette_window(
    context: &egui::Context,
    palette: &mut PaletteState,
    session: &mut MenubarSession,
    viewer: &mut EditorTabViewer<'_>,
) {
    if !palette.open {
        return;
    }
    // Step 1: Keep the commands whose titles match and that have an action the
    // model can run.
    let (runnable, unresolved): (Vec<&CommandView>, Vec<&CommandView>) =
        matches(&viewer.snapshot.commands, &palette.query)
            .into_iter()
            .partition(|row| menubar::action_for(row, viewer.snapshot, viewer.selection).is_some());
    let rows: Vec<CommandView> = runnable.into_iter().cloned().collect();
    palette.selected = palette.selected.min(rows.len().saturating_sub(1));
    // Step 2: Report each query change once, so a suite can wait for the
    // filter result instead of reading the screen. A matching command that the
    // model has no action for is named here and gets no row, so the popup
    // never offers a row it cannot run.
    if palette.logged_query.as_deref() != Some(palette.query.as_str()) {
        palette.logged_query = Some(palette.query.clone());
        for row in &unresolved {
            pill_core::warn!(
                target: telemetry_target::EDITOR,
                command = row.id.as_str(),
                "[editor] command has no action"
            );
        }
        pill_core::info!(
            target: telemetry_target::EDITOR,
            query = palette.query.as_str(),
            matches = rows.len(),
            first = rows.first().map_or("", |row| row.title.as_str()),
            "[editor] palette query"
        );
    }
    // Step 3: Draw the window, and follow the keys and clicks it reports.
    let mut clicked = None;
    let window = egui::Window::new("Command Palette")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, PALETTE_TOP))
        .default_width(PALETTE_WIDTH)
        .show(context, |ui| {
            ui.set_width(PALETTE_WIDTH);
            let field = ui.add(
                egui::TextEdit::singleline(&mut palette.query)
                    .hint_text("Search commands")
                    .desired_width(f32::INFINITY),
            );
            if !field.has_focus() {
                field.request_focus();
            }
            // The arrow keys move the selection and wrap around at either
            // end. Enter sets the same run request a click sets, and Escape
            // closes the popup.
            let (down, up, enter, escape) = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::ArrowDown),
                    input.key_pressed(egui::Key::ArrowUp),
                    input.key_pressed(egui::Key::Enter),
                    input.key_pressed(egui::Key::Escape),
                )
            });
            if down && !rows.is_empty() {
                palette.selected = (palette.selected + 1) % rows.len();
            }
            if up && !rows.is_empty() {
                palette.selected = (palette.selected + rows.len() - 1) % rows.len();
            }
            if enter {
                palette.run_requested = true;
            }
            if escape {
                palette.open = false;
                palette.run_requested = false;
            }
            ui.separator();
            for (index, row) in rows.iter().take(PALETTE_ROWS).enumerate() {
                let label = format!("{}  -  {}", row.title, row.id);
                if ui
                    .add(egui::Button::selectable(index == palette.selected, label))
                    .clicked()
                {
                    clicked = Some(index);
                }
            }
            if rows.is_empty() {
                ui.weak("No matching command");
            }
        });
    if let Some(index) = clicked {
        palette.selected = index;
        palette.run_requested = true;
    }
    // Step 4: Report the popup's rectangle as an overlay region, then run the
    // row the palette was asked to run.
    if let Some(response) = window {
        let rect = response.response.rect;
        viewer.overlay_regions.push(ShellRect {
            x: rect.min.x,
            y: rect.min.y,
            width: rect.width(),
            height: rect.height(),
        });
        pill_core::debug!(
            target: telemetry_target::EDITOR,
            x = rect.min.x,
            y = rect.min.y,
            width = rect.width(),
            height = rect.height(),
            "[editor] palette rect"
        );
    }
    if std::mem::take(&mut palette.run_requested) {
        run_selected(palette, session, viewer, &rows);
    }
}

/// Run the row at the palette's selection and close the popup.
///
/// Nothing else happens when no row is selected, which is the case while the
/// query matches no command that has an action.
pub fn run_selected(
    palette: &mut PaletteState,
    session: &mut MenubarSession,
    viewer: &mut EditorTabViewer<'_>,
    rows: &[CommandView],
) {
    let Some(row) = rows.get(palette.selected) else {
        palette.open = false;
        return;
    };
    let label = row.title.clone();
    let action = menubar::action_for(row, viewer.snapshot, viewer.selection);
    palette.open = false;
    pill_core::info!(
        target: telemetry_target::EDITOR,
        command = row.id.as_str(),
        "[editor] palette closed"
    );
    match action {
        Some(action) => run_menu_action("palette", label.as_str(), action, session, viewer),
        // Every row the palette lists has an action, so this arm never runs
        // from the popup. It stays for a caller that hands in rows of its own,
        // where a row with no action warns instead of silently doing nothing.
        None => pill_core::warn!(
            target: telemetry_target::EDITOR,
            command = row.id.as_str(),
            "[editor] command has no action"
        ),
    }
}
