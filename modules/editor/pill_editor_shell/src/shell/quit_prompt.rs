//! The quit prompt: what is unsaved, and whether to save it first.
//!
//! # Responsibilities
//!
//! - [`quit_prompt_window`]: while the executable holds a quit back for
//!   unsaved work, list that work and record the person's answer - save and
//!   quit, quit without saving, or keep working.
//! - Show the save in progress once the person chose to save, until the
//!   executable quits or gives up.
//!
//! # Design
//!
//! The window is a [`Prompt`]: this module supplies its body (the unsaved
//! items), its buttons and the answer each one stands for, and the generic
//! prompt draws the modal and reads the keys.
//!
//! The globs below are absolute paths, so a patch crate resolves them too.

// Current crate
use pill_editor_shell::shell::prelude::*;
// Unused in this build on purpose: a patch crate replays this file's opening
// `use` lines, and the file's own public items resolve through this glob
// there.
#[allow(unused_imports)]
use pill_editor_shell::shell::*;

// =============================================================================
// Constants
// =============================================================================

/// The prompt's width, in points.
pub const QUIT_PROMPT_WIDTH: f32 = 380.0;

/// How many unsaved items the prompt lists before it sums up the rest.
pub const QUIT_PROMPT_ITEMS_SHOWN: usize = 8;

/// The prompt's buttons, the safe answer first. Each one answers with the
/// entry at the same index of [`QUIT_PROMPT_ANSWERS`]; both arrays have the
/// same fixed length, so they cannot drift apart.
pub const QUIT_PROMPT_BUTTONS: [PromptButton<'static>; 3] = [
    PromptButton::new("Save and quit"),
    PromptButton::destructive("Quit without saving"),
    PromptButton::new("Cancel"),
];

/// The answer each of [`QUIT_PROMPT_BUTTONS`] stands for, by index.
pub const QUIT_PROMPT_ANSWERS: [QuitAnswer; 3] = [
    QuitAnswer::SaveAndQuit,
    QuitAnswer::QuitWithoutSaving,
    QuitAnswer::Cancel,
];

/// The button Enter presses: save, the answer that loses nothing.
pub const QUIT_PROMPT_ENTER_BUTTON: usize = 0;

/// The button Escape presses: keep working.
pub const QUIT_PROMPT_ESCAPE_BUTTON: usize = 2;

// =============================================================================
// Free Functions
// =============================================================================

/// Draws the quit prompt while the executable holds a quit back, and records
/// the answer in `unsaved_work` for the executable to act on.
///
/// Enter saves, Escape cancels. Once the person chose to save, the prompt
/// shows the save in progress and takes no answer until the executable quits
/// or gives up.
pub fn quit_prompt_window(context: &egui::Context, unsaved_work: &mut UnsavedWork) {
    let prompt_state = unsaved_work.prompt();
    if prompt_state == QuitPromptState::Closed {
        return;
    }
    // Step 1: the prompt, busy while a save it started runs.
    let mut prompt = Prompt::new(
        egui::Id::new("pill_editor.quit_prompt"),
        "Unsaved changes",
        QUIT_PROMPT_WIDTH,
        &QUIT_PROMPT_BUTTONS,
    )
    .with_enter(QUIT_PROMPT_ENTER_BUTTON)
    .with_escape(QUIT_PROMPT_ESCAPE_BUTTON);
    if let QuitPromptState::Saving { .. } = prompt_state {
        prompt = prompt.with_busy_text("Saving…");
    }
    // Step 2: its body lists what would be lost.
    let chosen_button = prompt_window(context, &prompt, &mut |ui| {
        unsaved_items_body(ui, unsaved_work.items());
    });
    // Step 3: the button becomes the answer the executable acts on.
    let Some(answer) =
        chosen_button.and_then(|button_index| QUIT_PROMPT_ANSWERS.get(button_index).copied())
    else {
        return;
    };
    pill_core::info!(
        target: telemetry_target::EDITOR,
        answer = ?answer,
        "[editor] quit prompt answered"
    );
    unsaved_work.answer(answer);
}

/// Draws the quit prompt's body: how many items would be lost, then the first
/// [`QUIT_PROMPT_ITEMS_SHOWN`] of them and a count of the rest.
pub fn unsaved_items_body(ui: &mut egui::Ui, items: &[UnsavedItem]) {
    ui.label(format!(
        "{} item{} would be lost if the editor quits now:",
        items.len(),
        if items.len() == 1 { "" } else { "s" }
    ));
    for item in items.iter().take(QUIT_PROMPT_ITEMS_SHOWN) {
        ui.label(format!("  •  {}", item.label));
    }
    if items.len() > QUIT_PROMPT_ITEMS_SHOWN {
        ui.label(format!(
            "  …and {} more",
            items.len() - QUIT_PROMPT_ITEMS_SHOWN
        ));
    }
}
