// Modules (.rs files)
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
            Pill_Game,
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
            Pill_Game,
        },
        graphics::{
            Pill_Renderer,
            RendererError
        },
        ecs::{
            Scene,
            MeshRenderingComponent,
            TransformComponent,
        },
    };
}

