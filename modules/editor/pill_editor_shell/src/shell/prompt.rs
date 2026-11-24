//! A modal prompt: a question with a body the caller draws and a row of
//! answers, which every editor prompt is built from.
//!
//! # Responsibilities
//!
//! - [`Prompt`] and [`PromptButton`]: what one prompt shows and offers, as
//!   plain data - its title, width, buttons, the buttons Enter and Escape
//!   press, and a busy line that stands in for the buttons while the work an
//!   answer started is still running.
//! - [`prompt_window`]: draw a prompt as an egui modal around the caller's
//!   body, and report the button the person chose this frame.
//!
//! # Design
//!
//! A prompt answers with the index of a button in its own list, not with a
//! value of the caller's type: the shell declares no generic functions, so
//! every one of them stays patchable by address, and each caller maps the
//! index to its own answer (the quit prompt to a `QuitAnswer`, a panel modal
//! to accepted or not). What an answer means, and the log line it leaves, stay
//! with the caller.
//!
//! The globs below are absolute paths, so a patch crate resolves them too.

// Current crate
// Unused in this build on purpose: a patch crate replays this file's opening
// `use` lines, and the file's own public items resolve through this glob
// there.
#[allow(unused_imports)]
use pill_editor_shell::shell::*;

// =============================================================================
// Types
// =============================================================================

/// One answer a prompt offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptButton<'a> {
    /// The button's text.
    pub label: &'a str,
    /// Whether the answer throws work away; the button is drawn in the danger
    /// colour so it does not read like the safe choice.
    pub destructive: bool,
}

impl<'a> PromptButton<'a> {
    /// A button whose answer loses nothing.
    pub const fn new(label: &'a str) -> Self {
        Self {
            label,
            destructive: false,
        }
    }

    /// A button whose answer throws work away.
    pub const fn destructive(label: &'a str) -> Self {
        Self {
            label,
            destructive: true,
        }
    }
}

/// What one prompt shows and offers.
///
/// The body is not part of it: [`prompt_window`] takes it as a closure, so a
/// prompt can show anything from one sentence to a list of what would be lost.
#[derive(Clone, Copy, Debug)]
pub struct Prompt<'a> {
    /// The modal's identity. It must stay the same from frame to frame while
    /// the prompt is open, and differ between prompts open at once.
    pub id: egui::Id,
    /// The heading above the body.
    pub title: &'a str,
    /// The prompt's width, in points.
    pub width: f32,
    /// The answers, left to right. A prompt answers with an index into this
    /// list.
    pub buttons: &'a [PromptButton<'a>],
    /// The button Enter presses, when one does.
    pub enter_button: Option<usize>,
    /// The button Escape presses, when one does.
    pub escape_button: Option<usize>,
    /// While set, the buttons are replaced by a spinner and this line, and the
    /// prompt answers nothing: the work an earlier answer started is still
    /// running, and the caller closes the prompt when it ends.
    pub busy_text: Option<&'a str>,
}

impl<'a> Prompt<'a> {
    /// A prompt with `buttons` that no key answers and that is not busy.
    pub fn new(id: egui::Id, title: &'a str, width: f32, buttons: &'a [PromptButton<'a>]) -> Self {
        Self {
            id,
            title,
            width,
            buttons,
            enter_button: None,
            escape_button: None,
            busy_text: None,
        }
    }

    /// Lets Enter press the button at `button_index`.
    pub fn with_enter(mut self, button_index: usize) -> Self {
        self.enter_button = Some(button_index);
        self
    }

    /// Lets Escape press the button at `button_index`.
    pub fn with_escape(mut self, button_index: usize) -> Self {
        self.escape_button = Some(button_index);
        self
    }

    /// Shows `text` beside a spinner in place of the buttons, so the prompt
    /// answers nothing while it is busy.
    pub fn with_busy_text(mut self, text: &'a str) -> Self {
        self.busy_text = Some(text);
        self
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Draws `prompt` as an egui modal: its title, then the caller's `body`, then
/// its buttons. Returns the index of the button the person chose this frame,
/// by click or by the key bound to it.
///
/// The modal darkens the editor and takes its input, so the caller publishes
/// no overlay rectangle for it. A busy prompt draws the body and its busy line
/// and returns `None`. A key bound to an index the prompt has no button for
/// answers nothing.
pub fn prompt_window(
    context: &egui::Context,
    prompt: &Prompt<'_>,
    body: &mut dyn FnMut(&mut egui::Ui),
) -> Option<usize> {
    egui::Modal::new(prompt.id)
        .show(context, |ui| {
            // Step 1: the question and what it is about.
            ui.set_width(prompt.width);
            ui.heading(prompt.title);
            ui.separator();
            body(ui);
            ui.separator();
            // Step 2: a busy prompt only says so; its caller ends the work and
            // closes it.
            if let Some(busy_text) = prompt.busy_text {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(busy_text);
                });
                return None;
            }
            // Step 3: the buttons, then the keys bound to them.
            let clicked_button = prompt_buttons(ui, prompt.buttons);
            let (enter_pressed, escape_pressed) = ui.input(|input| {
                (
                    input.key_pressed(egui::Key::Enter),
                    input.key_pressed(egui::Key::Escape),
                )
            });
            let bound_index = |button_index: Option<usize>| {
                button_index.filter(|&index| index < prompt.buttons.len())
            };
            clicked_button
                .or(bound_index(prompt.enter_button).filter(|_| enter_pressed))
                .or(bound_index(prompt.escape_button).filter(|_| escape_pressed))
        })
        .inner
}

/// Draws a prompt's buttons in one row and returns the index of the one
/// clicked this frame.
pub fn prompt_buttons(ui: &mut egui::Ui, buttons: &[PromptButton<'_>]) -> Option<usize> {
    ui.horizontal(|ui| {
        let mut clicked_button = None;
        for (button_index, button) in buttons.iter().enumerate() {
            let text = if button.destructive {
                egui::RichText::new(button.label).color(style::danger())
            } else {
                egui::RichText::new(button.label)
            };
            if ui.button(text).clicked() {
                clicked_button = Some(button_index);
            }
        }
        clicked_button
    })
    .inner
}
