#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod audio_listener_component;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod audio_manager_component;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod audio_source_component;
pub(crate) mod camera_component;
mod component;
mod component_storage;
pub(crate) mod deferred_update_component;
#[cfg(feature = "debug_ui")]
pub(crate) mod egui_manager_component;
pub(crate) mod input_component;
pub(crate) mod mesh_rendering_component;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod network_manager_component;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod network_state_component;
pub(crate) mod time_component;
pub(crate) mod transform_component;

// --- Use ---

pub use component::{Component, ComponentDestroyer, ConcreteComponentDestroyer, GlobalComponent};

pub use component_storage::{ComponentStorage, GlobalComponentStorage};
