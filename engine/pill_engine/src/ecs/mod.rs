#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

mod components;
mod entity;
mod scene;
mod scene_manager;
mod systems;

// --- Use ---

// - Components

pub use components::{
    Component, ComponentDestroyer, ComponentStorage, ConcreteComponentDestroyer, GlobalComponent,
    GlobalComponentStorage,
};

pub use components::camera_component::{
    get_renderer_resource_handle_from_camera_component, CameraAspectRatio, CameraComponent,
};

#[cfg(not(target_arch = "wasm32"))]
pub use components::audio_manager_component::{AudioManagerComponent, SoundType};

#[cfg(not(target_arch = "wasm32"))]
pub use components::audio_listener_component::AudioListenerComponent;

#[cfg(not(target_arch = "wasm32"))]
pub use components::audio_source_component::AudioSourceComponent;

pub use components::egui_manager_component::EguiManagerComponent;

pub use components::deferred_update_component::{
    DeferredUpdateComponent, DeferredUpdateComponentRequest, DeferredUpdateManagerPointer,
    DeferredUpdateRequest, DeferredUpdateResourceRequest,
};

pub use components::input_component::{
    GamepadAxis, GamepadButton, GamepadEvent, HapticCommand, InFlight, InputComponent, InputEvent,
    KeyboardEvent, MouseEvent, PlayerId,
};

pub use components::transform_component::{
    get_model_matrix, get_normal_matrix, update_transform_matrices, TransformComponent,
};

pub use components::mesh_rendering_component::MeshRenderingComponent;

pub use components::time_component::TimeComponent;

#[cfg(not(target_arch = "wasm32"))]
pub use components::network_manager_component::{
    ClientState, ConnectionState, NetworkManagerComponent, NetworkSide,
};

#[cfg(not(target_arch = "wasm32"))]
pub use components::network_state_component::{NetworkEntityState, NetworkStateComponent};

// - Systems

pub use systems::{SystemFunction, SystemManager, UpdatePhase};

pub use systems::rendering_system::rendering_system;

pub use systems::deferred_update_system::deferred_update_system;

pub use systems::input_system::input_system;

#[cfg(not(target_arch = "wasm32"))]
pub use systems::input_system::haptics_system;

pub use systems::time_system::time_system;

#[cfg(not(target_arch = "wasm32"))]
pub use systems::audio_system::audio_system;

#[cfg(not(target_arch = "wasm32"))]
pub use systems::networking_system::{
    client_go_offline, networking_system_client, networking_system_server, EntityUpdate,
    NetworkEntityAction, NetworkUpdatePayload,
};

// - Other

pub use entity::{Entity, EntityBuilder, EntityHandle};

pub use scene::Scene;

pub use scene_manager::{SceneHandle, SceneManager};
