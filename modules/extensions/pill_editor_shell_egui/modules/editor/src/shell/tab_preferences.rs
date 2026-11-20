//! The Preferences tab: the editor-wide preference rows and the project's
//! gizmo rows.
//!
//! # Responsibilities
//!
//! - `preferences_tab`: draws the editor-wide rows, then the project's name,
//!   its remembered and active gizmo settings, and the buttons that change the
//!   gizmo. The rows' text is kept in the session's draft: Save sends it as one
//!   batch, and a cancel drops it.
//!
//! # Design
//!
//! The globs below are absolute paths. A generated patch crate copies this
//! file's top-level `use` lines into a crate of its own, where a `super::`
//! path would have nothing to point at.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
use pill_editor_shell_egui_editor::shell::*;

/// Draws the Preferences tab: the editor-wide rows, then the project's name,
/// its remembered and active gizmo settings and the buttons that change the
/// gizmo.
///
/// The rows start from the text in the preferences file. Save queues one
/// read-modify-write batch holding every row, and Cancel drops the edits, so
/// the next frame starts from the file again. The gizmo buttons queue the same
/// `SetGizmoTool` commands the shortcuts, the menus and the scripted channel
/// queue.
#[pill_hot_fn]
pub fn preferences_tab(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    draft: &mut Option<preferences::PreferencesDraft>,
    pending: &mut PendingEditorCommands,
) {
    egui::ScrollArea::vertical().show(ui, |ui| {
        // Step 1: The editor-wide rows, drawn from the draft.
        let view = &snapshot.preferences;
        // `None` means the draft is re-read from the file. The first frame sets
        // it; a save or a cancel drops it, so the next frame starts from the
        // file again.
        let buffers = draft.get_or_insert_with(|| view.seed_buffers());
        ui.heading("Editor");
        ui.label(
            egui::RichText::new("Preferences file: blank keeps the default, Save writes the rows.")
                .weak(),
        );
        for field in preferences::PREFERENCE_FIELDS {
            let buffer = buffers.entry(field.key.to_string()).or_default();
            // The previews row is a checkbox, not a text box, so its state is
            // visible. Ticking it writes "true" or "false" back as the text the
            // preferences parser reads, and queues the change at once: the
            // asset tiles start rendering without a Save, while every other row
            // still waits for one.
            if field.key == pill_editor_core::previews::RENDER_PREVIEWS_KEY {
                let mut enabled = buffer.trim().eq_ignore_ascii_case("true");
                let mut changed = false;
                row_in(ui, WIDE_ROW_LABEL_WIDTH, field.label, None, &mut |ui| {
                    changed = ui.checkbox(&mut enabled, "").changed();
                });
                if changed {
                    *buffer = if enabled { "true" } else { "false" }.to_string();
                    pending.push(vec![EditorCommand::SetPreferences {
                        values: vec![(field.key.to_string(), buffer.clone())],
                    }]);
                    pill_core::info!(
                        target: telemetry_target::EDITOR,
                        enabled,
                        "[editor] preference applied"
                    );
                }
                continue;
            }
            row_in(ui, WIDE_ROW_LABEL_WIDTH, field.label, None, &mut |ui| {
                ui.add(egui::TextEdit::singleline(buffer).desired_width(ui.available_width()));
            });
        }
        // Step 2: The Save and Cancel buttons.
        let mut save = false;
        let mut cancel = false;
        ui.horizontal(|ui| {
            save = ui.button("Save preferences").clicked();
            cancel = ui.button("Cancel").clicked();
        });
        if save {
            let values = preferences::PREFERENCE_FIELDS
                .iter()
                .map(|field| {
                    (
                        field.key.to_string(),
                        buffers.get(field.key).cloned().unwrap_or_default(),
                    )
                })
                .collect::<Vec<_>>();
            pending.push(vec![EditorCommand::SetPreferences { values }]);
            // Drop the draft, so the next frame reads the file again and the
            // rows show the values the save wrote rather than the edited text.
            *draft = None;
            pill_core::info!(
                target: telemetry_target::EDITOR,
                "[editor] preferences save queued"
            );
        }
        if cancel {
            // A cancel throws the edits away. The next frame reads the file
            // again, just as it does after a save.
            *draft = None;
            pill_core::info!(
                target: telemetry_target::EDITOR,
                "[editor] preferences cancel"
            );
        }
        // Step 3: The project's name and remembered gizmo, the active gizmo,
        // then the buttons that change it.
        ui.separator();
        ui.heading("Project");
        value_row_in(
            ui,
            WIDE_ROW_LABEL_WIDTH,
            "project",
            view.project.as_deref().unwrap_or("(unknown)"),
        );
        if view.project_remembered {
            value_row_in(
                ui,
                WIDE_ROW_LABEL_WIDTH,
                "remembered mode",
                view.project_gizmo.mode.map_or("(unset)", gizmo_mode_label),
            );
            value_row_in(
                ui,
                WIDE_ROW_LABEL_WIDTH,
                "remembered space",
                view.project_gizmo
                    .space
                    .map_or("(unset)", gizmo_space_label),
            );
            value_row_in(
                ui,
                WIDE_ROW_LABEL_WIDTH,
                "remembered snap",
                &view
                    .project_gizmo
                    .snap
                    .map_or("(unset)".to_string(), |snap| snap.to_string()),
            );
        } else {
            row_in(ui, WIDE_ROW_LABEL_WIDTH, "remembered", None, &mut |ui| {
                ui.weak("none yet - a tool choice is remembered per project.");
            });
        }
        value_row_in(
            ui,
            WIDE_ROW_LABEL_WIDTH,
            "active mode",
            gizmo_mode_label(snapshot.gizmo.mode),
        );
        value_row_in(
            ui,
            WIDE_ROW_LABEL_WIDTH,
            "active space",
            gizmo_space_label(snapshot.gizmo.space),
        );
        value_row_in(
            ui,
            WIDE_ROW_LABEL_WIDTH,
            "active snap",
            &snapshot.gizmo.snap.to_string(),
        );
        row_in(ui, WIDE_ROW_LABEL_WIDTH, "gizmo", None, &mut |ui| {
            let state = toolbox::ToolboxState {
                gizmo: snapshot.gizmo,
                grid: snapshot.grid_enabled,
                gizmos: snapshot.gizmos_enabled,
                frame: snapshot.gizmo_frame.as_deref(),
                component_tools: &snapshot.component_tools,
            };
            for item in toolbox::TOOL_ROWS {
                let button = icons::IconButton::new(item.icon(&state), item.label(&state))
                    .active(item.active(&state));
                if ui.add(button).clicked() {
                    pending.push(vec![item.command(&state)]);
                }
            }
            // The row is narrow, so the spaces show their short names here
            // instead of the long labels the viewport menu uses.
            for row in toolbox::SPACE_ROWS {
                let button = icons::IconButton::new(row.icon, row.short)
                    .active(snapshot.gizmo.space == row.space);
                if ui.add(button).clicked() {
                    pending.push(vec![toolbox::space_command(row.space)]);
                }
            }
            let snap = icons::IconButton::new(
                toolbox::ToolboxItem::Snap.icon(&state),
                toolbox::ToolboxItem::Snap.label(&state),
            )
            .active(toolbox::ToolboxItem::Snap.active(&state));
            if ui.add(snap).clicked() {
                pending.push(vec![toolbox::ToolboxItem::Snap.command(&state)]);
            }
        });
        // Step 4: One switch per registered component gizmo. The choice is a
        // preference, so the executable writes it to the file and applies it
        // again after a reload.
        ui.separator();
        ui.heading("Gizmos");
        value_row_in(
            ui,
            WIDE_ROW_LABEL_WIDTH,
            "gizmos",
            if snapshot.gizmos_enabled { "on" } else { "off" },
        );
        if snapshot.gizmo_types.is_empty() {
            row_in(ui, WIDE_ROW_LABEL_WIDTH, "types", None, &mut |ui| {
                ui.weak("none - no loaded module declares a component gizmo.");
            });
            return;
        }
        for gizmo_type in &snapshot.gizmo_types {
            row_in(
                ui,
                WIDE_ROW_LABEL_WIDTH,
                &gizmo_type.type_name,
                None,
                &mut |ui| {
                    let mut visible = gizmo_type.visible;
                    if ui.checkbox(&mut visible, "").changed() {
                        pending.push(vec![EditorCommand::SetGizmoTypeVisible {
                            type_name: gizmo_type.type_name.clone(),
                            visible,
                        }]);
                        pill_core::info!(
                            target: telemetry_target::EDITOR,
                            gizmo_type = gizmo_type.type_name.as_str(),
                            visible,
                            "[editor] gizmo type row"
                        );
                    }
                },
            );
        }
    });
}
