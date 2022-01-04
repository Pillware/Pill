#![cfg_attr(debug_assertions, allow(dead_code, unused_variables))]

mod entity;
mod component_storage;
mod scene;
mod scene_manager;
mod system_manager;
mod component;
mod bitmask_controller;
mod bitmask_map;
mod entity_fetcher;
mod entity_builder;

mod components;
mod systems;

// --- Use ---

// - Components

pub use components::camera_component::{
    CameraComponent,
    CameraAspectRatio,
    get_renderer_resource_handle_from_camera_component,
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

// - Other

// pub use allocator::{
//     Allocator
// };

pub use bitmask_controller::{
    Bitmask,
    BitmaskController
};

pub use bitmask_map::{
    BitmaskMap
};

pub use entity_fetcher::{
    EntityFetcher
};

pub use entity_builder::{
    EntityBuilder
};

pub use component:: {
    Component,
};

pub use component_storage::{
    ComponentStorage,
    GlobalComponentStorage,
};

pub use entity::{
    Entity,
    EntityHandle
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