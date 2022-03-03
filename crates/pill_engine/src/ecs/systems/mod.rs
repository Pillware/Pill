#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

mod system_manager;
pub(crate) mod rendering_system;
pub(crate) mod deferred_update_system;
pub(crate) mod input_system;
pub(crate) mod time_system;
pub(crate) mod audio_system;
pub(crate) mod egui_system;

// --- Use ---

pub use system_manager::{
    SystemManager,
    UpdatePhase,
};