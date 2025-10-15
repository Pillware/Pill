#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

pub(crate) mod audio_listener_component;
pub(crate) mod audio_manager_component;
pub(crate) mod audio_source_component;
pub(crate) mod camera_component;
mod component;
mod component_storage;
pub(crate) mod deferred_update_component;
pub(crate) mod egui_manager_component;
pub(crate) mod input_component;
pub(crate) mod mesh_rendering_component;
pub(crate) mod render_state_component;
pub(crate) mod time_component;
pub(crate) mod transform_component;

// --- Use ---

pub use component::{Component, ComponentDestroyer, ConcreteComponentDestroyer, GlobalComponent};

pub use component_storage::{ComponentStorage, GlobalComponentStorage};
