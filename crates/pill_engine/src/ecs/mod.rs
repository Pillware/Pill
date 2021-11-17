#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod mesh_rendering_component;
mod transform_component;
mod entity;
mod component_map;
mod component_storage;
mod scene;
mod scene_manager;
mod system_manager;
mod allocator;
mod name_component;
mod health_component;

pub use scene::{
    Scene,
    SceneHandle,
};

pub use component_map::{ 
    ComponentMap, 
    Component,
};

pub use component_storage:: {
    ComponentStorage,
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

pub use name_component::{
    NameComponent,
};

pub use health_component::{
    HealthComponent,
};

pub use scene_manager::{
    SceneManager,
};

pub use system_manager::{
    SystemManager,
    UpdatePhase,
};

pub use allocator::{
    Generation,
    Allocator,
};