//! The scripted channel: the page's one entry point for driving the editor
//! without input events.
//!
//! # Responsibilities
//!
//! - [`editor_command`]: the wasm export the editor page calls with one
//!   scripted line per call.
//! - [`take_scripted_lines`]: the function a running runner calls once a
//!   frame to take the queued lines.
//!
//! # Design
//!
//! There is one queue. An editor build links exactly one runner, so nothing
//! else ever reads it, and the page keeps a single `editor_command` export.
//! Like every wasm module, this runs on one thread.

// Standard library
use std::cell::RefCell;

// External crates
use wasm_bindgen::prelude::wasm_bindgen;

// The lines the page has scripted; the next frame takes them all.
thread_local! {
    static SCRIPTED_LINES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Queues one scripted line for the next frame of whichever editor runner
/// this build links.
#[wasm_bindgen]
pub fn editor_command(line: &str) {
    SCRIPTED_LINES.with(|lines| lines.borrow_mut().push(line.to_owned()));
}

/// Takes the lines queued since the last frame, leaving the queue empty.
pub fn take_scripted_lines() -> Vec<String> {
    SCRIPTED_LINES.with(|lines| std::mem::take(&mut *lines.borrow_mut()))
}
