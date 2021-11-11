// Modules (.rs files)
mod engine;
mod scene;
mod resources;
mod graphics;
mod input;

// Structs available for other crates
pub use self::engine::Engine;
pub use self::engine::Pill_Game;
pub use self::scene::Scene;
pub use self::graphics::renderer::Pill_Renderer;
pub use self::graphics::renderer::RendererError;

pub use self::ecs::{
    MeshRenderingComponent,
    TransformComponent,
};

//pub extern crate core::types::XXX_core;

pub use self::input::{
    input_event
};


pub extern crate pill_core;


//pub use  self::core::XXX_core;

// pub mod aa {
//     pub use crate:: {
//         resources::resource_manager,
//     }
// }

mod ecs;