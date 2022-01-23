#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

mod entity;
mod component_storage;
mod scene;
mod scene_manager;
mod system_manager;
mod component;
mod entity_fetcher;

mod components;
mod systems;

// --- Use ---

// - Components

pub use components::camera_component::{
    CameraComponent,
    CameraAspectRatio,
    get_renderer_resource_handle_from_camera_component,
};

pub use components::audio_manager_component::{
    AudioManagerComponent,
    SoundType,
};

pub use components::audio_listener_component::{
    AudioListenerComponent,
};

pub use components::audio_source_component::{
    AudioSourceComponent
};

pub use components::deferred_update_component::{
    DeferredUpdateComponent,
    DeferredUpdateManager,
    DeferredUpdateManagerPointer,
    DeferredUpdateRequest,
    DeferredUpdateComponentRequest,
    DeferredUpdateResourceRequest
};

pub use components::input_component::{
    InputComponent,
    InputEvent,
};

pub use components::transform_component::{
    TransformComponent,
};

pub use components::mesh_rendering_component::{
    MeshRenderingComponent,
};

pub use components::time_component::{
    TimeComponent,
};

// - Systems

pub use systems::rendering_system::{
    rendering_system,
};

pub use systems::deferred_update_system::{
    deferred_update_system,
};

pub use systems::input_system::{
    input_system,
};

pub use systems::time_system::{
    time_system,
};

pub use systems::audio_system::{
    audio_system,
};

// - Other

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

pub use entity_fetcher::{
    EntityFetcher,
};

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

pub use system_manager::{
    SystemManager,
    UpdatePhase,
};