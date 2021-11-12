// Modules (.rs files)
mod engine;
mod scene;
mod resources;
mod graphics;
mod input;
mod ecs;

#[cfg(feature = "game")]
pub mod game {
    pub use crate::{
        engine::{
            Engine,
            Pill_Game,
        },
        scene::SceneHandle,
        ecs::{
            MeshRenderingComponent,
            TransformComponent,
        },
    };

    extern crate pill_core;
    pub use pill_core::OBW;
}

#[cfg(feature = "internal")]
pub mod internal {
    pub use crate::{
        engine::{
            Engine,
            Pill_Game,
            
        },
        graphics::renderer::Pill_Renderer,
        graphics::renderer::RendererError,
        scene::Scene,
        ecs::{
            MeshRenderingComponent,
            TransformComponent,
        },
    };
}

