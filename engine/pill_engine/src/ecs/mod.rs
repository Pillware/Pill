#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

mod components;
mod entity;
mod scene;
mod scene_manager;
mod systems;
mod egui_client;

// --- Use ---

// - Components

pub use components::{
    Component, ComponentDestroyer, ComponentStorage, ConcreteComponentDestroyer, GlobalComponent,
    GlobalComponentStorage,
};

pub use components::camera_component::{
    get_renderer_resource_handle_from_camera_component, CameraAspectRatio, CameraComponent,
};

pub use components::audio_manager_component::{AudioManagerComponent, SoundType};

pub use components::audio_listener_component::AudioListenerComponent;

pub use components::audio_source_component::AudioSourceComponent;

pub use components::egui_manager_component::EguiManagerComponent;

pub use components::deferred_update_component::{
    DeferredUpdateComponent, DeferredUpdateComponentRequest, DeferredUpdateManager,
    DeferredUpdateManagerPointer, DeferredUpdateRequest, DeferredUpdateResourceRequest,
};

pub use components::input_component::{InputComponent, InputEvent};

pub use components::transform_component::{
    get_model_matrix, get_normal_matrix, update_transform_matrices, TransformComponent,
};

pub use components::mesh_rendering_component::MeshRenderingComponent;

pub use components::time_component::TimeComponent;

pub use components::render_state_component::RenderStateComponent;

// - Systems

pub use systems::{SystemFunction, SystemManager, UpdatePhase};

// - Egui client (UI input/closure mailbox)
pub use egui_client::EguiClient;

pub use systems::rendering_system::rendering_system;

pub use systems::deferred_update_system::deferred_update_system;

pub use systems::input_system::input_system;

pub use systems::time_system::time_system;

pub use systems::audio_system::audio_system;

// - Other

pub use entity::{Entity, EntityBuilder, EntityHandle};

pub use scene::Scene;

pub use scene_manager::{SceneHandle, SceneManager};
