#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

pub(crate) mod audio_system;
pub(crate) mod deferred_update_system;
pub(crate) mod input_system;
pub(crate) mod networking_system;
pub(crate) mod rendering_system;
mod system_manager;
pub(crate) mod time_system;

// --- Use ---

pub use system_manager::{SystemFunction, SystemManager, UpdatePhase};
