// Modules (.rs files)
mod engine;
mod scene;
mod resources;
mod graphics;
mod input;
mod ecs;

// #[macro_use]
// extern crate cfg_if;


// cfg_if! {
//     if #[cfg(feature = "game")] {
//         extern crate pill_core;
//         pub mod game {
//             pub use crate::{
//                 engine::{
//                     Engine,
//                     Pill_Game,
//                 },
//                 scene::SceneHandle,
//                 graphics::renderer::{
//                     Pill_Renderer,
//                     RendererError,
//                 },
//                 ecs::{
//                     MeshRenderingComponent,
//                     TransformComponent,
//                 }
//             };
//             pub use pill_core::OBW as ooo;
           
//         }
//         //pub extern crate pill_core;
       

//     }
// }


// cfg_if! {
//     if #[cfg(feature = "internal")] {
//         pub mod internal {
//             pub use crate::{
//                 engine::{
//                     Engine,
//                     Pill_Game,
//                 },
//                 scene::Scene,
//                 graphics::renderer::{
//                     Pill_Renderer,
//                     RendererError,
//                 },
//                 ecs::{
//                     MeshRenderingComponent,
//                     TransformComponent,
//                 }
//             };
//         }
//     }
// }





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


// #[cfg(feature = "game")]
// pub use self::game::*;

// //#[cfg(not(feature = "game"))]
// #[cfg(feature = "internal")]
// mod internal {
//     pub use crate::{
//         engine::{
//             Engine,
//             Pill_Game,
//         },
//         scene::Scene,
//         graphics::renderer::{
//             Pill_Renderer,
//             RendererError,
//         },
//         ecs::{
//             MeshRenderingComponent,
//             TransformComponent,
//         }
//     };
// }

// #[cfg(feature = "internal")]
// pub use self::internal::*;

// #[cfg(feature = "internal")]
// pub use self::graphics::renderer::Pill_Renderer;
