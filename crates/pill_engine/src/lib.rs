#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod engine;
mod resources;
mod graphics;
mod ecs;
mod config;

// --- Use ---

#[cfg(feature = "game")]
pub mod game {
    pub use crate::{
        engine::{
            Engine,
            PillGame,
            KeyboardKey,
            MouseButton,
        },
        ecs::{
            Scene,
            SceneHandle,
            MeshRenderingComponent,
            TransformComponent,
            InputComponent,
            CameraComponent,
            CameraAspectRatio,
            EntityHandle,
            AudioSourceComponent,
            AudioListenerComponent,
            TimeComponent,
            AudioManagerComponent,
            Component,
            ComponentStorage,
            GlobalComponent,
            GlobalComponentStorage,
        },
        resources::{
            Resource,
            ResourceStorage,
            Texture, 
            TextureHandle,
            TextureType,
            Material,
            MaterialHandle,
            Mesh,
            MeshHandle,
            ResourceLoadType,
            Sound
        },

    };
    
    extern crate pill_core;
    pub use pill_core::{ 
        PillTypeMapKey, 
        Vector2f, 
        Vector3f, 
        Color, 
        Vector2i, 
        Vector3i,
        define_new_pill_slotmap_key,
    };
  
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
            InputComponent,
            TimeComponent,
            CameraAspectRatio,
            AudioSourceComponent,
            AudioListenerComponent,
            AudioManagerComponent,
            get_renderer_resource_handle_from_camera_component,
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
            MaterialParameterMap,
            get_renderer_texture_handle_from_material_texture,
        },
    };
}

