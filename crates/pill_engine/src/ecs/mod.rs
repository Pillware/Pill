#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod mesh_rendering_component;
mod transform_component;
mod entity;
mod component_map;
mod component_storage;
mod scene;
mod scene_manager;
mod system_manager;
mod camera_component;
mod rendering_system;
mod component;
mod deferred_update_component;
mod deferred_update_system;
mod allocator;
mod bitmask_controller;
mod bitmask_map;
mod entity_fetcher;
mod entity_builder;

pub use allocator::{
    Allocator
};

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

pub use rendering_system::{
    rendering_system,
};

pub use scene::{
    Scene,
};

pub use component_map::{ 
    ComponentMap, 
    Component,
};

pub use component_storage::{
    ComponentStorage,
};

pub use camera_component::{
    CameraComponent,
    CameraAspectRatio,
};

pub use entity::{
    Entity,
    EntityHandle
};

pub use mesh_rendering_component::{
    MeshRenderingComponent,
};

pub use transform_component::{
    TransformComponent,
};

pub use scene_manager::{
    SceneManager,
    SceneHandle,
};

pub use system_manager::{
    SystemManager,
    UpdatePhase,
};

pub use deferred_update_component::{
    DeferredUpdateGlobalComponent,
    DeferredUpdateManager,
    DeferredUpdateManagerPointer,
};

// pub use deferred_update_system::{
    
// };