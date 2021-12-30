#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod engine;
mod resources;
mod graphics;
mod ecs;
mod input;
mod config;
mod time;

#[cfg(feature = "game")]
pub mod game {
    pub use crate::{
        engine::{
            Engine,
            PillGame,
            Key,
            Mouse,
        },
        ecs::{
            Scene,
            SceneHandle,
            MeshRenderingComponent,
            TransformComponent,
            CameraComponent,
            CameraAspectRatio,
            EntityHandle,
        },
        resources::{
            Texture, 
            TextureHandle,
            TextureType,
            Material,
            MaterialHandle,
            Mesh,
            MeshHandle,
            ResourceLoadType,
        },

    };
    
    extern crate pill_core;
    pub use pill_core::{ Vector2f, Vector3f, Color, Vector2i, Vector3i };

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
        config::*,
        graphics::{
            PillRenderer,
            RendererError,
            RenderQueueKey,
            RenderQueueItem,
            RenderQueueKeyFields,
            decompose_render_queue_key,

            RendererCameraHandle,
            RendererMaterialHandle,
            RendererMeshHandle,
            RendererPipelineHandle,
            RendererTextureHandle,
            RENDER_QUEUE_KEY_ORDER
        },
        ecs::{
            Scene,
            ComponentStorage,
            MeshRenderingComponent,
            TransformComponent,
            CameraComponent,
            EntityHandle,
            CameraAspectRatio,
            //DeferredUpdateRequest,
        },
        resources::{
            Texture, 
            TextureHandle,
            TextureType,

            Material,
            MaterialHandle,

            Mesh,
            MeshHandle,
            MeshData,
            MeshVertex,    

            ResourceLoadType,
            ResourceManager,
            MaterialTexture,
            MaterialTextureMap,
            MaterialParameter,
            MaterialParameterMap
        },
        input::{
            InputComponent
        },
        time::{
            TimeComponent
        }
    };
}

