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

pub use components::input_component::{
    InputComponent,
    InputEvent,
};

pub use components::transform_component::{
    TransformComponent,
    update_transform_matrices,
    get_model_matrix,
    get_normal_matrix,
};

pub use components::mesh_rendering_component::{
    MeshRenderingComponent,
};

pub use components::time_component::{
    TimeComponent,
};

pub use components::net_components::{
    NetState,
    NetSide,
};

pub use components::network_state_component::{
    NetworkStateComponent,
    NetEntityState,
};

// - Systems

pub use systems::{
    SystemManager,
    UpdatePhase,
    SystemFunction
};

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

pub use systems::networking_system::{
    networking_system_client,
    networking_system_server,
    EntityUpdate,
    NetworkUpdatePayload,
    NetEntityAction
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
