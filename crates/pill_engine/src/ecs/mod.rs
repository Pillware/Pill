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
mod allocator;
mod bitmask_controller;
mod bitmask_map;
mod entity_fetcher;

pub(crate) mod entity_builder;

pub use entity_fetcher::*;

pub use entity_builder::{
    EntityBuilder
};

pub use allocator::{
    Allocator,
    Generation
};

pub use rendering_system::{
    rendering_system,
};

pub use scene::{
    Scene,
    SceneHandle,
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
};

pub use entity::{
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
};

pub use system_manager::{
    SystemManager,
    UpdatePhase,
};

pub use bitmask_controller::{
    BitmaskController,
};

pub use bitmask_map::*;