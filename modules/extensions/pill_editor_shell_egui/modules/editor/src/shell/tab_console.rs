//! The Console tab: the engine's and the project's recent log lines.
//!
//! # Responsibilities
//!
//! - Draw the filter row - the level buttons, the channel switch and the
//!   search field - and the rows the filters leave.
//! - Edit the filters in the shared session. The executable applies them to
//!   the history on the next frame, so the tab never touches the world.
//! - Report how many lines the filters leave and how many the history holds.
//!
//! # Design
//!
//! The rows come from `ConsoleView` in the frame's snapshot: the newest
//! `CONSOLE_VIEW_ROWS` lines the filters leave, already owned strings. The
//! tab only reads that snapshot and edits the session's filter copy, so
//! drawing it never needs the history the executable keeps.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
// Deliberately unused in this compilation: a patch crate repeats this file's
// top-level `use` lines, so a name the body takes from a sibling module still
// resolves in the patch crate.
#[allow(unused_imports)]
use pill_editor_shell_egui_editor::shell::*;

/// The levels the filter row offers, lowest first, each as a `(level, label)`
/// pair.
pub const CONSOLE_LEVELS: [(u8, &str); 5] = [
    (1, "trace"),
    (2, "debug"),
    (3, "info"),
    (4, "warn"),
    (5, "error"),
];

/// Draws the Console tab: the filter row and the counts line above, the
/// newest rows below.
pub fn console_tab(
    ui: &mut egui::Ui,
    snapshot: &EditorSnapshot,
    filters: &mut ConsoleFilterSettings,
) {
    let view = &snapshot.console;
    // Step 1: The filter row - the level buttons, the channel switch and the
    // search field.
    ui.horizontal_wrapped(|ui| {
        ui.label("Level");
        for (level, name) in CONSOLE_LEVELS {
            let selected = filters.minimum_level == level;
            if ui
                .add(egui::Button::selectable(selected, name))
                .on_hover_text(format!("show {name} and above"))
                .clicked()
            {
                filters.minimum_level = level;
            }
        }
        ui.separator();
        ui.label("Channel");
        for (name, channel) in [
            ("both", None),
            ("engine", Some(ConsoleChannel::Engine)),
            ("game", Some(ConsoleChannel::Game)),
        ] {
            let selected = filters.channel == channel;
            if ui.add(egui::Button::selectable(selected, name)).clicked() {
                filters.channel = channel;
            }
        }
        ui.separator();
        ui.label("Search");
        ui.add(
            egui::TextEdit::singleline(&mut filters.search)
                .hint_text("substring")
                .desired_width(160.0),
        );
    });
    ui.separator();
    // Step 2: The counts line reads as `visible of total`: how many lines the
    // filters leave out of the history. The history drops its oldest lines at
    // its cap, so `total` counts only the lines it kept.
    ui.label(
        egui::RichText::new(format!(
            "{} of {} lines (level {}+, channel {}, search {})",
            view.visible,
            view.total,
            console::level_name(filters.minimum_level),
            filters.channel.map_or("both", ConsoleChannel::name),
            if filters.search.trim().is_empty() {
                "-"
            } else {
                filters.search.as_str()
            }
        ))
        .small()
        .color(style::text()),
    );
    // Step 3: The newest rows the filters leave, oldest first, in a list that
    // stays scrolled to the bottom.
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for line in &view.lines {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(line.level_name())
                            .monospace()
                            .color(console_level_colour(line.level)),
                    );
                    ui.label(
                        egui::RichText::new(format!("{}: {}", line.target, line.message))
                            .monospace(),
                    );
                });
            }
            if view.lines.is_empty() {
                ui.weak("No lines match");
            }
        });
}

/// Returns the colour of a level's tag: the palette's log colours, the same
/// three colours a toast uses for its notice level, so one level reads the
/// same everywhere.
pub fn console_level_colour(level: u8) -> egui::Color32 {
    match level {
        4 => style::log_warn(),
        5 => style::log_error(),
        _ => style::log_info(),
    }
}
