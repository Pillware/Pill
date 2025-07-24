#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

mod system_manager;
#[cfg(feature = "rendering")]
pub(crate) mod rendering_system;
pub(crate) mod deferred_update_system;
#[cfg(feature = "rendering")]
pub(crate) mod input_system;
pub(crate) mod time_system;
#[cfg(feature = "rendering")]
pub(crate) mod audio_system;

// --- Use ---

pub use system_manager::{
    SystemManager,
    UpdatePhase,
};
