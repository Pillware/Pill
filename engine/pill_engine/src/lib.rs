#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod config;
mod ecs;
mod engine;
mod graphics;
mod resources;

// --- Macros ---

pub use ecs::{Component, ComponentStorage, GlobalComponent, GlobalComponentStorage};
pub use pill_core::PillTypeMapKey;

#[macro_export]
macro_rules! define_component {
    (
        $name:ident {
            $( $field_name:ident : $field_ty:ty ),* $(,)?
        }
    ) => {
        pub struct $name {
            $( pub $field_name: $field_ty, )*
        }

        impl $crate::PillTypeMapKey for $name {
            type Storage = $crate::ComponentStorage<$name>;
        }

        impl $crate::Component for $name {}
    };
}

#[macro_export]
macro_rules! define_global_component {
    (
        $name:ident {
            $( $field_name:ident : $field_ty:ty ),* $(,)?
        }
    ) => {
        pub struct $name {
            $( pub $field_name: $field_ty ),*
        }

        impl $crate::PillTypeMapKey for $name {
            type Storage = $crate::GlobalComponentStorage<$name>;
        }

        impl $crate::GlobalComponent for $name {}
    };
}

// --- Use ---

#[cfg(feature = "game")]
pub mod game {
    pub use crate::{
        ecs::{
            AudioListenerComponent, AudioManagerComponent, AudioSourceComponent, CameraAspectRatio,
            CameraComponent, Component, ComponentStorage, EguiManagerComponent, EntityHandle,
            GlobalComponent, GlobalComponentStorage, InputComponent, MeshRenderingComponent,
            SceneHandle, SoundType, TimeComponent, TransformComponent, UpdatePhase,
        },
        engine::{Engine, KeyboardKey, MouseButton, PillGame},
        resources::{
            Material, MaterialHandle, Mesh, MeshHandle, Resource, ResourceLoadType,
            ResourceStorage, Sound, Texture, TextureHandle, TextureType,
        },
    };

    extern crate pill_core;
    pub use pill_core::{
        create_game, define_new_pill_slotmap_key, Color, PillTypeMapKey, Vector2f, Vector2i,
        Vector3f, Vector3fExt, Vector3i,
    };

    extern crate anyhow;
    pub use anyhow::{Context, Error, Result};
}

#[cfg(feature = "internal")]
pub mod internal {
    pub use crate::{
        config::*,
        ecs::{
            get_model_matrix, get_normal_matrix,
            get_renderer_resource_handle_from_camera_component, update_transform_matrices,
            AudioListenerComponent, AudioManagerComponent, AudioSourceComponent, CameraAspectRatio,
            CameraComponent, ComponentStorage, EguiManagerComponent, EntityHandle, InputComponent,
            MeshRenderingComponent, Scene, TimeComponent, TransformComponent,
        },
        engine::{Engine, PillGame},
        graphics::{
            decompose_render_queue_key, BufferDesc, PillRenderer, PipelineV2, PipelineV2Desc,
            RenderQueueItem, RenderQueueKey, RenderQueueKeyFields, RendererBufferHandle,
            RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle,
            RendererPipelineHandle, RendererPipelineV2Handle, RendererTextureHandle, ShaderDesc,
            RENDER_QUEUE_KEY_ORDER,
        },
        resources::{
            get_renderer_texture_handle_from_material_texture, Material, MaterialHandle,
            MaterialParameter, MaterialParameterMap, MaterialTexture, MaterialTextureMap, Mesh,
            MeshData, MeshHandle, MeshVertex, ResourceLoadType, ResourceManager, Texture,
            TextureHandle, TextureType,
        },
    };
}
