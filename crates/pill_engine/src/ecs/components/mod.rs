#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

mod component_storage;
mod component;
#[cfg(feature = "rendering")]
pub(crate) mod mesh_rendering_component;
pub(crate) mod transform_component;
#[cfg(feature = "rendering")]
pub(crate) mod camera_component;
pub(crate) mod deferred_update_component;
#[cfg(feature = "rendering")]
pub(crate) mod input_component;
pub(crate) mod time_component;
#[cfg(feature = "rendering")]
pub(crate) mod audio_listener_component;
#[cfg(feature = "rendering")]
pub(crate) mod audio_source_component;
#[cfg(feature = "rendering")]
pub(crate) mod audio_manager_component;
#[cfg(feature = "rendering")]
pub(crate) mod egui_manager_component;

// --- Use ---

pub use component:: {
    Component,
    GlobalComponent,
    ComponentDestroyer,
    ConcreteComponentDestroyer
};

pub use component_storage::{
    ComponentStorage,
    GlobalComponentStorage,
};
