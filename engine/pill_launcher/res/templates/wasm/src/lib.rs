//! WASM entry point - scaffolded by `PillLauncher create`.
//!
//! This is a minimal shim that wires together:
//! - The project's `Project` struct (from `src/project.rs`).
//! - The project's `config.ini` (embedded at compile time - WASM has no filesystem).
//! - The shared `pill_web::run` runtime (panic hook, canvas, event loop).
//!
//! Keeping this file tiny ensures the per-project `.wasm` binary stays small.

use project::Project;
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
#[global_allocator]
// SAFETY: wasm32 is single-threaded, so assuming single-threaded allocator access is sound.
static ALLOC: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };

#[wasm_bindgen(start)]
pub fn wasm_main() {
    pill_web::run(Box::new(Project {}), include_str!("../config.ini"));
}
