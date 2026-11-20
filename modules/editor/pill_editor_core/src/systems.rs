//! The Systems tab's shared logic: the row and button labels, and the editor
//! command each control queues.
//!
//! # Responsibilities
//!
//! - [`system_row_label`]: the row text `name (owner)`, with `[ambiguous]`
//!   added when another system has the same name.
//! - [`system_state_label`] and [`system_toggle`]: the `on`/`off` button text
//!   and the recorded `SetSystemEnabled` edit it queues, under the history
//!   label `system {index} on|off`.
//! - [`pause_button_label`] and [`pause_toggle_command`]: the Pause/Resume
//!   control. Its button, the View menu's row and the scripted channel all
//!   read the engine's current pause state and queue the opposite.
//!
//! # Design
//!
//! The tab's widgets and the scripted channel call these same helpers. A
//! state button's click and a scripted `edit system` therefore record the
//! same undo entry, under the same label. The pause control likewise changes
//! the same pause flag whichever entry point asked. The commands ride the
//! shared [`PendingEditorCommands`](crate::commands::PendingEditorCommands)
//! queue and apply at the frame boundary like every other editor edit.

// Current crate
use crate::editor_state::{EditorCommand, SystemSummary};

// =============================================================================
// Free Functions
// =============================================================================

/// The row label for one system: `name (owner)`, marked `[ambiguous]` when
/// another system shares the name.
pub fn system_row_label(system: &SystemSummary) -> String {
    if system.name_is_ambiguous {
        format!("{} ({}) [ambiguous]", system.name, system.owner_label)
    } else {
        format!("{} ({})", system.name, system.owner_label)
    }
}

/// The state button's text for a system: `on` or `off`.
pub fn system_state_label(enabled: bool) -> &'static str {
    if enabled {
        "on"
    } else {
        "off"
    }
}

/// The recorded toggle a state button queues: a `SetSystemEnabled` edit
/// paired with the label its undo entry takes.
pub fn system_toggle(index: usize, enabled: bool) -> (EditorCommand, String) {
    (
        EditorCommand::SetSystemEnabled { index, enabled },
        format!("system {index} {}", system_state_label(enabled)),
    )
}

/// The Pause/Resume button's text for the engine's current pause state.
pub fn pause_button_label(paused: bool) -> &'static str {
    if paused {
        "Resume"
    } else {
        "Pause"
    }
}

/// The pause toggle the Pause/Resume control queues: a `SetPaused` edit that
/// flips the engine's pause flag. It applies without being recorded, so
/// pausing never becomes an undo entry.
pub fn pause_toggle_command(paused: bool) -> EditorCommand {
    EditorCommand::SetPaused { paused: !paused }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(name: &str, owner_label: &str, name_is_ambiguous: bool) -> SystemSummary {
        SystemSummary {
            index: 0,
            name: name.to_string(),
            owner_label: owner_label.to_string(),
            enabled: true,
            hot_patchable: false,
            name_is_ambiguous,
        }
    }

    #[test]
    fn the_row_label_carries_the_owner_and_the_ambiguity_marker() {
        assert_eq!(
            system_row_label(&summary("tick", "project", false)),
            "tick (project)"
        );
        assert_eq!(
            system_row_label(&summary("tick", "pill_spline", true)),
            "tick (pill_spline) [ambiguous]"
        );
    }

    #[test]
    fn the_toggle_records_the_native_label() {
        let (command, label) = system_toggle(2, false);
        assert_eq!(
            command,
            EditorCommand::SetSystemEnabled {
                index: 2,
                enabled: false
            }
        );
        assert_eq!(label, "system 2 off");
        let (_, label) = system_toggle(0, true);
        assert_eq!(label, "system 0 on");
    }

    #[test]
    fn the_pause_control_flips_the_state_it_reads() {
        assert_eq!(
            pause_toggle_command(false),
            EditorCommand::SetPaused { paused: true }
        );
        assert_eq!(
            pause_toggle_command(true),
            EditorCommand::SetPaused { paused: false }
        );
        assert_eq!(pause_button_label(false), "Pause");
        assert_eq!(pause_button_label(true), "Resume");
    }
}
