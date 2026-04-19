// See pill_engine/src/lib.rs — wasm-only silence; native release still flags dead code.
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

pub mod config;
pub mod drawers;
pub mod instance;
pub mod renderer;
pub mod resources;
//pub mod profiler;

// --- Use ---

pub use renderer::*;

pub use instance::Instance;
