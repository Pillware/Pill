//! The Project Settings tab: every setting the project and its modules
//! declare, with where each value comes from.
//!
//! # Responsibilities
//!
//! - `project_settings_tab`: draws one section per entry - the modules the
//!   project lists, then `engine`, `host` and `editor` - and one row per
//!   setting: its provenance glyph with a tooltip, its name, its value in
//!   effect, and the three actions an edit has.
//! - The row actions queue the commands: write to `project_settings.yaml`,
//!   write to the local override, or reset the key out of both.
//!
//! # Design
//!
//! The values and their sources come from the executable, which resolves them
//! against the schemas the host and the loaded modules declare; the tab only
//! draws them and queues writes. A row's edited text lives in egui's temporary
//! memory under the setting's own path, so switching tabs drops it and the row
//! reads the file again - the same rule the Preferences tab's draft follows.
//!
//! The globs below are absolute paths. A generated patch crate copies this
//! file's top-level `use` lines into a crate of its own, where a `super::`
//! path would have nothing to point at.

// Current crate
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::*;

/// The point size of the provenance glyphs, in the panel's rows and legend.
const SETTING_ICON_SIZE: f32 = 14.0;

/// Draws the Project Settings tab.
#[pill_hot_fn]
pub fn project_settings_tab(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    pending: &mut PendingEditorCommands,
) {
    let view = &snapshot.project_settings;
    let Some(directory) = view.directory.clone() else {
        ui.vertical_centered(|ui| {
            ui.add_space(12.0);
            ui.strong("No project is open");
            ui.weak("A project's settings are listed once the editor runs one.");
        });
        return;
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading("Project settings");
        ui.label(
            egui::RichText::new(
                "An edit writes project_settings.yaml; the local column writes the override beside it and never commits.",
            )
            .weak(),
        );
        if let Some(refusal) = view.refusal.as_deref() {
            ui.colored_label(ui.visuals().error_fg_color, refusal);
        }
        // Step 1: what each glyph means, once, above the rows that use them.
        ui.horizontal_wrapped(|ui| {
            for provenance in [
                project_settings::SettingProvenance::Default,
                project_settings::SettingProvenance::Project,
                project_settings::SettingProvenance::Local,
                project_settings::SettingProvenance::Environment,
            ] {
                ui.label(egui::RichText::new(provenance.icon()).weak());
                ui.label(
                    egui::RichText::new(provenance.glyph().glyph().to_string())
                        .family(egui::FontFamily::Name(icons::FAMILY.into()))
                        .size(SETTING_ICON_SIZE),
                )
                .on_hover_text(provenance.tooltip(pill_engine::ApplyTiming::Startup));
            }
        });
        // Step 2: one section per entry, in the view's own order.
        for entry in view.entries() {
            ui.separator();
            ui.horizontal(|ui| {
                ui.strong(entry);
                if entry_is_off(view, entry) {
                    ui.weak("(off)");
                }
            });
            for row in view.rows_for(entry) {
                project_setting_row(
                    ui,
                    row,
                    directory.as_str(),
                    pending,
                );
            }
        }
    });
}

/// Draws one setting's row: the provenance glyph, the setting's name and what
/// a change does, the value in a text field, and the three actions.
pub fn project_setting_row(
    ui: &mut egui::Ui,
    row: &project_settings::ProjectSettingRow,
    directory: &str,
    pending: &mut PendingEditorCommands,
) {
    row_in(ui, WIDE_ROW_LABEL_WIDTH, row.label(), None, &mut |ui| {
        ui.label(
            egui::RichText::new(row.provenance.glyph().glyph().to_string())
                .family(egui::FontFamily::Name(icons::FAMILY.into()))
                .size(SETTING_ICON_SIZE),
        )
        .on_hover_text(row.tooltip());
    });
    // The buffer survives between frames in egui's temporary memory, keyed by
    // the setting's own path; it starts from the value in effect.
    let buffer_id = egui::Id::new(("project_setting", row.full_path.as_str()));
    let mut buffer = ui.data_mut(|data| {
        data.get_temp_mut_or(buffer_id, row.effective_text.clone())
            .clone()
    });
    row_in(ui, WIDE_ROW_LABEL_WIDTH, "", None, &mut |ui| {
        let field = ui.add(egui::TextEdit::singleline(&mut buffer).desired_width(120.0));
        field.on_hover_text(row.tooltip());
        if ui
            .small_button("set")
            .on_hover_text("Write this value to project_settings.yaml.")
            .clicked()
        {
            queue_write(pending, row, directory, buffer.as_str(), false);
        }
        if ui
            .small_button("local")
            .on_hover_text(
                "Write this value to project_settings.local.yaml, the override that never commits.",
            )
            .clicked()
        {
            queue_write(pending, row, directory, buffer.as_str(), true);
        }
        if ui
            .add_enabled(
                row.project_text.is_some() || row.local_text.is_some(),
                egui::Button::new("reset").small(),
            )
            .on_hover_text("Remove the key from whichever settings file holds it.")
            .clicked()
        {
            pending.push(vec![EditorCommand::ResetProjectSetting {
                directory: directory.to_string(),
                path: row.full_path.clone(),
            }]);
            ui.data_mut(|data| {
                data.remove::<String>(buffer_id);
            });
        }
    });
    ui.data_mut(|data| {
        data.insert_temp(buffer_id, buffer);
    });
}

/// Queues one write, and reports what it will do in the log.
pub fn queue_write(
    pending: &mut PendingEditorCommands,
    row: &project_settings::ProjectSettingRow,
    directory: &str,
    value: &str,
    local: bool,
) {
    pending.push(vec![EditorCommand::SetProjectSetting {
        directory: directory.to_string(),
        path: row.full_path.clone(),
        value: value.to_string(),
        local,
    }]);
    pill_core::info!(
        target: telemetry_target::EDITOR,
        setting = row.full_path.as_str(),
        local,
        "[editor] project setting edit queued"
    );
}

/// Whether the entry's own `enabled` row reads off, which is what the section
/// heading's `(off)` marks.
pub fn entry_is_off(view: &project_settings::ProjectSettingsView, entry: &str) -> bool {
    view.rows_for(entry)
        .find(|row| row.path == pill_engine::ENABLED_KEY)
        .is_some_and(|row| row.effective_text == "off" || row.effective_text == "false")
}
