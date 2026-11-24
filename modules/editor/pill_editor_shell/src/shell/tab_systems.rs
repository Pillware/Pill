//! The Systems tab: the engine's pause and single-step controls, then one
//! row per registered system with its recorded, undoable toggle.
//!
//! # Responsibilities
//!
//! - Draw [`systems_tab`]: the header's Pause/Resume and Step buttons, then
//!   the systems' rows.
//!
//! # Design
//!
//! A patch of this file replays its top-level `use` lines and adds the crate's
//! glob, so the two imports below are the whole rule. `shell::prelude::*`
//! brings in the shared model's names (icons, systems, the snapshot types) and
//! `shell::*` the sibling modules. A body that names one of them by bare name
//! compiles as a patch only while both are here.

// External crates
use pill_engine::pill_hot_fn;

// Current crate
use pill_editor_shell::shell::*;

/// Draws the Systems tab: the Pause/Resume and Step buttons and the count of
/// enabled systems, then one row per registered system. A row shows the
/// system's name and the module that owns it, with an `on`/`off` button that
/// queues the recorded, undoable toggle.
#[pill_hot_fn]
pub fn systems_tab(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    pending: &mut PendingEditorCommands,
) {
    ui.horizontal(|ui| {
        // Pause and Step live in the menu bar's play toolbar, above every
        // tab, so the tab only reports what the system list holds.
        let enabled_system_count = snapshot.systems.iter().filter(|row| row.enabled).count();
        ui.label(
            egui::RichText::new(format!(
                "{}/{} enabled",
                enabled_system_count,
                snapshot.systems.len()
            ))
            .weak(),
        );
    });
    ui.separator();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for system in &snapshot.systems {
            // The row label is `name (owner)`, and the owner label can be
            // long. The label column cuts text that does not fit, so the full
            // label is passed as a tooltip too.
            let row_label = systems::system_row_label(system);
            row_in(
                ui,
                WIDE_ROW_LABEL_WIDTH,
                &row_label,
                Some(row_label.as_str()),
                &mut |ui| {
                    if ui
                        .add(icons::IconButton::new(
                            icons::Icon::Power,
                            systems::system_state_label(system.enabled),
                        ))
                        .clicked()
                    {
                        let (command, label) =
                            systems::system_toggle(system.index, !system.enabled);
                        pending.push_recorded(vec![command], label, None);
                    }
                },
            );
        }
    });
}
