//! The Problems tab: one row per problem, grouped by the module or function
//! that reported it.
//!
//! # Responsibilities
//!
//! - Draw one row per problem the executable collected from a failed build, a
//!   failed patch or a rollback, grouped under the module or function that
//!   reported it.
//! - Turn a click on a row that names a file into an open request for the
//!   executable, which knows the `code_editor_command` preference and may
//!   spawn a process.
//!
//! # Design
//!
//! The rows arrive in the frame's snapshot (`EditorSnapshot.problems`). The
//! executable builds both the rows and the toasts that report a failed build
//! from the same host notices, so a row and its toast never say different
//! things. The tab is read-only apart from the request it leaves in the
//! session.

// Current crate
use pill_editor_shell_egui_editor::shell::prelude::*;
// Deliberately unused in this compilation. A patch crate repeats this file's
// top-level `use` lines, so a name the body takes from a sibling module still
// resolves in the patch crate.
#[allow(unused_imports)]
use pill_editor_shell_egui_editor::shell::*;

/// Draws the Problems tab: one section per module or function, each with its
/// rows.
///
/// `request` is set to the index of the row the user clicked in
/// `snapshot.problems`, when that row names a file. It is left as it was
/// otherwise. The executable turns the index into the location it opens in
/// the code editor.
pub fn problems_tab(ui: &mut egui::Ui, snapshot: &EditorSnapshot, request: &mut Option<usize>) {
    // Step 1: Show a note when the executable reported nothing.
    if snapshot.problems.is_empty() {
        ui.weak("No problems");
        return;
    }
    // Step 2: Group the rows by the module or function that reported them,
    // keeping the order the rows have in the snapshot. A module that fails
    // again drops its old rows and gets its new ones at the end.
    let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();
    for (index, row) in snapshot.problems.iter().enumerate() {
        match groups.iter_mut().find(|(module, _)| *module == row.module) {
            Some((_, indices)) => indices.push(index),
            None => groups.push((row.module.as_str(), vec![index])),
        }
    }
    // Step 3: Draw one section per group, with the name and row count on a
    // heading line and the rows below it.
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (module, indices) in groups {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{module} ({})", indices.len()))
                            .strong()
                            .color(style::text()),
                    );
                });
                for index in indices {
                    let row = &snapshot.problems[index];
                    // Show `file:line` when the compiler named both, the file
                    // alone when it named no line, and `-` when it named no
                    // file. Clicking a row without a file cannot open
                    // anything, so only a row with a file leaves a request.
                    let location = match (&row.problem.file, row.problem.line) {
                        (Some(file), Some(line)) => format!("{file}:{line}"),
                        (Some(file), None) => file.clone(),
                        _ => "-".to_string(),
                    };
                    let label = format!(
                        "  {}  {location}  {}",
                        row.problem.severity.name(),
                        row.problem.message
                    );
                    let response =
                        ui.add(egui::Button::new(egui::RichText::new(label).monospace()).wrap());
                    if response.clicked() && row.problem.file.is_some() {
                        *request = Some(index);
                    }
                }
                ui.separator();
            }
        });
}
