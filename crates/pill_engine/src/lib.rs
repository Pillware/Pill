#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod engine;
mod resources;
mod ecs;
mod config;

#[cfg(feature = "rendering")]
mod graphics;

pub use engine::{Engine, PillGame};

#[cfg(feature = "rendering")]
pub use engine::{KeyboardKey, MouseButton};

#[cfg(feature = "net")]
pub mod net;

#[cfg(feature = "game")]
pub mod game {
    pub use crate::engine::{Engine, PillGame};

    #[cfg(feature = "rendering")]
    pub use crate::engine::{KeyboardKey, MouseButton};

    pub use crate::ecs::{
        SceneHandle,
        TransformComponent,
        EntityHandle,
        TimeComponent,
        Component, ComponentStorage,
        GlobalComponent, GlobalComponentStorage,
        SoundType
    };

    #[cfg(feature = "rendering")]
    pub use crate::ecs::{
        MeshRenderingComponent,
        CameraComponent,
        CameraAspectRatio,
        InputComponent,
        AudioSourceComponent,
        AudioListenerComponent,
        AudioManagerComponent,
        EguiManagerComponent,
        get_renderer_resource_handle_from_camera_component,
    };

    pub use crate::resources::{
        Resource, ResourceStorage,
        Texture, TextureHandle, TextureType,
        Material, MaterialHandle,
        Mesh, MeshHandle,
        ResourceLoadType,
        Sound,
    };

    #[cfg(feature = "rendering")] pub use crate::resources::MeshData;
    #[cfg(feature = "rendering")] pub use crate::resources::MeshVertex;
    #[cfg(feature = "rendering")] pub use crate::resources::MaterialTexture;
    #[cfg(feature = "rendering")] pub use crate::resources::MaterialTextureMap;
    #[cfg(feature = "rendering")] pub use crate::resources::MaterialParameter;
    #[cfg(feature = "rendering")] pub use crate::resources::MaterialParameterMap;

    extern crate pill_core;
    pub use pill_core::{
        PillTypeMapKey,
        Vector2f, Vector3f, Color,
        Vector2i, Vector3i,
        define_new_pill_slotmap_key,
    };

    extern crate anyhow;
    pub use anyhow::{ Context, Result, Error };
}

// -----------------------------------------------------------------------------
// INTERNAL convenience re-exports. Only build if BOTH internal + rendering
#[cfg(all(feature = "internal", feature = "rendering"))]
pub mod internal {
    pub use crate::{
        engine::{Engine, PillGame},
        config::*,
        graphics::{
            PillRenderer, RendererError,
            RenderQueueKey, RenderQueueItem, RenderQueueKeyFields,
            decompose_render_queue_key,
            RendererCameraHandle, RendererMaterialHandle,
            RendererMeshHandle, RendererPipelineHandle, RendererTextureHandle,
            RENDER_QUEUE_KEY_ORDER,
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
            EguiManagerComponent,
            get_renderer_resource_handle_from_camera_component,
        },
        resources::{
            Texture, TextureHandle, TextureType,
            Material, MaterialHandle,
            Mesh, MeshHandle, MeshData, MeshVertex,
            ResourceLoadType, ResourceManager,
            MaterialTexture, MaterialTextureMap,
            MaterialParameter, MaterialParameterMap,
            get_renderer_texture_handle_from_material_texture,
        },
    };
}

