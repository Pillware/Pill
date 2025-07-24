#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

mod entity;
mod scene;
mod scene_manager;
mod components;
mod systems;

// --- Use ---

// - Components

pub use components:: {
    Component,
    GlobalComponent,
    ComponentDestroyer,
    ConcreteComponentDestroyer,
    ComponentStorage,
    GlobalComponentStorage,
};

#[cfg(feature = "rendering")]
pub use components::camera_component::{
    CameraComponent,
    CameraAspectRatio,
    get_renderer_resource_handle_from_camera_component,
};

#[cfg(feature = "rendering")]
pub use components::audio_manager_component::{
    AudioManagerComponent,
    SoundType,
};

#[cfg(feature = "rendering")]
pub use components::audio_listener_component::{
    AudioListenerComponent,
};

#[cfg(feature = "rendering")]
pub use components::audio_source_component::{
    AudioSourceComponent
};

#[cfg(feature = "rendering")]
pub use components::egui_manager_component::{
    EguiManagerComponent,
};

pub use components::deferred_update_component::{
    DeferredUpdateComponent,
    DeferredUpdateManager,
    DeferredUpdateManagerPointer,
    DeferredUpdateRequest,
    DeferredUpdateComponentRequest,
    DeferredUpdateResourceRequest
};

#[cfg(feature = "rendering")]
pub use components::input_component::{
    InputComponent,
    InputEvent,
};

pub use components::transform_component::{
    TransformComponent,
};

#[cfg(feature = "rendering")]
pub use components::mesh_rendering_component::{
    MeshRenderingComponent,
};

pub use components::time_component::{
    TimeComponent,
};

// - Systems

pub use systems::{
    SystemManager,
    UpdatePhase,
};

#[cfg(feature = "rendering")]
pub use systems::rendering_system::{
    rendering_system,
};

pub use systems::deferred_update_system::{
    deferred_update_system,
};

#[cfg(feature = "rendering")]
pub use systems::input_system::{
    input_system,
};

pub use systems::time_system::{
    time_system,
};

#[cfg(feature = "rendering")]
pub use systems::audio_system::{
    audio_system,
};

// - Other

pub use entity::{
    Entity,
    EntityHandle,
    EntityBuilder,
};

pub use scene::{
    Scene,
};

pub use scene_manager::{
    SceneManager,
    SceneHandle,
};
