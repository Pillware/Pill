//! The Statistics tab: what one frame costs, and what each tab costs.
//!
//! # Responsibilities
//!
//! - Draw the frame's numbers: frame rate, frame time, the shell pass's time
//!   and pass count, and the counts of entities, systems, console lines and
//!   problems.
//! - Draw one row per tab drawn since the session began, with that tab's
//!   smoothed body time and its pass count.
//!
//! # Design
//!
//! The times are measured before the tab draws: `run_frame` times the whole
//! pass, and `TabViewer::ui` times each tab body. The frame rate comes from
//! the editor through the shell input, and the counts come from the frame's
//! snapshot. This tab only formats these numbers; it records nothing.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
use pill_editor_shell_egui_editor::shell::*;

/// The Statistics tab: the frame's timings and the editor's counts, then one
/// row per drawn tab.
///
/// `frames_per_second` is the editor's smoothed frame rate, and the frame time
/// row is computed from it. `stats` holds what the shell recorded: the whole
/// pass's time, and one timing per tab drawn so far.
pub fn statistics_tab(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    stats: &StatisticsSession,
    frames_per_second: f32,
) {
    let frame_micros = if frames_per_second > 0.0 {
        1_000_000.0 / frames_per_second as f64
    } else {
        0.0
    };
    value_row_in(
        ui,
        WIDE_ROW_LABEL_WIDTH,
        "frames per second",
        &format!("{frames_per_second:.1}"),
    );
    value_row_in(
        ui,
        WIDE_ROW_LABEL_WIDTH,
        "frame time",
        &format!("{frame_micros:.0} us"),
    );
    value_row_in(
        ui,
        WIDE_ROW_LABEL_WIDTH,
        "shell pass",
        &format!(
            "{:.0} us ({} passes)",
            stats.shell_micros, stats.shell_passes
        ),
    );
    value_row_in(
        ui,
        WIDE_ROW_LABEL_WIDTH,
        "entities",
        &snapshot.entities.len().to_string(),
    );
    value_row_in(
        ui,
        WIDE_ROW_LABEL_WIDTH,
        "systems",
        &snapshot.systems.len().to_string(),
    );
    value_row_in(
        ui,
        WIDE_ROW_LABEL_WIDTH,
        "console lines",
        &snapshot.console.total.to_string(),
    );
    value_row_in(
        ui,
        WIDE_ROW_LABEL_WIDTH,
        "problems",
        &snapshot.problems.len().to_string(),
    );
    ui.separator();
    if stats.tabs.is_empty() {
        ui.weak("No tab measured yet");
        return;
    }
    ui.label(egui::RichText::new("per-tab body time").strong());
    for entry in &stats.tabs {
        row_in(
            ui,
            WIDE_ROW_LABEL_WIDTH,
            &entry.tab.title(),
            None,
            &mut |ui| {
                ui.monospace(format!("{:.0} us", entry.micros));
                ui.weak(format!("{} passes", entry.passes));
            },
        );
    }
}
