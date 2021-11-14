#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod engine;
mod resources;
mod graphics;
mod ecs;
mod input;

#[cfg(feature = "game")]
pub mod game {
    pub use crate::{
        engine::{
            Engine,
            PillGame,
        },
        ecs::{
            SceneHandle,
            MeshRenderingComponent,
            TransformComponent,
        },
    };
    
    extern crate pill_core;
    pub use pill_core::Vector2f;

    extern crate anyhow;
    pub use anyhow::{ Context, Result, Error };
}

#[cfg(feature = "internal")]
pub mod internal {
    pub use crate::{
        engine::{
            Engine,
            PillGame,
        },
        graphics::{
            PillRenderer,
            RendererError
        },
        ecs::{
            Scene,
            MeshRenderingComponent,
            TransformComponent,
        },
    };
}

