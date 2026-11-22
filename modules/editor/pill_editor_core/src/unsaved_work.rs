//! Work that editor modules hold in memory and that would be lost on quit,
//! and the prompt that asks about it.
//!
//! # Responsibilities
//!
//! - Hold what each module reports as unsaved ([`UnsavedItem`]), replaced
//!   whole on every report, so an item disappears as soon as its module saves.
//! - Carry the quit prompt's state between the executable, which opens it
//!   instead of quitting, and the shell, which draws it and records the
//!   answer.
//! - Carry the "save everything" request to the modules, numbered so each
//!   module answers one request once.
//!
//! # Design
//!
//! The executable owns the decision to quit; a module owns how its work is
//! saved; the shell owns the dialog. None of them calls another: they meet in
//! this resource. A quit by the person (the File menu or the main window's
//! close button) opens the prompt while anything is unsaved, and a quit by a
//! script or the test deadline does not ask.

// External crates
use pill_engine::Resource;

// =============================================================================
// Types
// =============================================================================

/// One piece of unsaved work a module reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnsavedItem {
    /// The crate that reported it.
    pub source: String,
    /// What the prompt lists ("cube - edited outside the editor").
    pub label: String,
    /// The asset file the work belongs to, relative to `res`, when there is
    /// one: the Assets tab marks that file.
    pub asset_path: Option<String>,
}

/// How the person answered the quit prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitAnswer {
    /// Ask every module to save, then quit once nothing is unsaved.
    SaveAndQuit,
    /// Quit and lose the unsaved work.
    QuitWithoutSaving,
    /// Close the prompt and keep working.
    Cancel,
}

/// Where the quit prompt stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QuitPromptState {
    /// No prompt is shown.
    #[default]
    Closed,
    /// The prompt is shown and waits for an answer.
    Asking,
    /// The person chose to save; the editor quits once the modules saved.
    Saving {
        /// Frames waited for the modules so far.
        frames_waited: u32,
    },
}

/// The unsaved work the modules reported and the quit prompt's state.
#[derive(Debug, Default)]
pub struct UnsavedWork {
    /// Every module's items, in report order.
    items: Vec<UnsavedItem>,
    /// The prompt's state.
    prompt: QuitPromptState,
    /// The answer the shell recorded and the executable has not taken yet.
    answer: Option<QuitAnswer>,
    /// The number of the newest "save everything" request; zero means none
    /// was ever made.
    save_request_number: u64,
}

impl UnsavedWork {
    /// Replaces every item `source` reported before with `items`.
    pub fn report(&mut self, source: &str, items: Vec<UnsavedItem>) {
        self.items.retain(|item| item.source != source);
        self.items.extend(items);
    }

    /// Forgets every item `source` reported; the executable calls this when
    /// that module reloads, so a module that stops reporting leaves nothing.
    pub fn forget(&mut self, source: &str) {
        self.items.retain(|item| item.source != source);
    }

    /// Returns every reported item.
    pub fn items(&self) -> &[UnsavedItem] {
        &self.items
    }

    /// Returns whether any module holds unsaved work.
    pub fn has_unsaved(&self) -> bool {
        !self.items.is_empty()
    }

    /// Returns whether the asset at `path` (relative to `res`) has unsaved
    /// work.
    pub fn is_unsaved_path(&self, path: &str) -> bool {
        self.items
            .iter()
            .any(|item| item.asset_path.as_deref() == Some(path))
    }

    /// Returns the prompt's state.
    pub fn prompt(&self) -> QuitPromptState {
        self.prompt
    }

    /// Sets the prompt's state; the executable drives it.
    pub fn set_prompt(&mut self, prompt: QuitPromptState) {
        self.prompt = prompt;
    }

    /// Records the person's answer; the shell calls this from the prompt.
    pub fn answer(&mut self, answer: QuitAnswer) {
        self.answer = Some(answer);
    }

    /// Takes the recorded answer, once.
    pub fn take_answer(&mut self) -> Option<QuitAnswer> {
        self.answer.take()
    }

    /// Asks every module to save everything it reported.
    pub fn request_save(&mut self) {
        self.save_request_number += 1;
    }

    /// Returns the newest save request's number. A module compares it with
    /// the last number it handled, so each request is answered once.
    pub fn save_request_number(&self) -> u64 {
        self.save_request_number
    }

    /// Inserts the resource into `world` when it holds none.
    pub fn ensure(world: &mut pill_engine::World) {
        world.register_resource::<UnsavedWork>();
        if world.get_resource::<UnsavedWork>().is_none() {
            world.insert_resource(UnsavedWork::default());
        }
    }
}

impl Resource for UnsavedWork {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn item(source: &str, path: &str) -> UnsavedItem {
        UnsavedItem {
            source: source.to_string(),
            label: path.to_string(),
            asset_path: Some(path.to_string()),
        }
    }

    #[test]
    fn a_report_replaces_only_its_own_source() {
        let mut unsaved_work = UnsavedWork::default();
        unsaved_work.report("a", vec![item("a", "one.obj"), item("a", "two.obj")]);
        unsaved_work.report("b", vec![item("b", "three.obj")]);
        unsaved_work.report("a", Vec::new());
        assert_eq!(unsaved_work.items().len(), 1);
        assert!(unsaved_work.is_unsaved_path("three.obj"));
        assert!(!unsaved_work.is_unsaved_path("one.obj"));
    }

    #[test]
    fn an_answer_is_taken_once() {
        let mut unsaved_work = UnsavedWork::default();
        unsaved_work.answer(QuitAnswer::Cancel);
        assert_eq!(unsaved_work.take_answer(), Some(QuitAnswer::Cancel));
        assert_eq!(unsaved_work.take_answer(), None);
    }
}
