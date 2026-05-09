#![cfg_attr(
    any(debug_assertions, target_arch = "wasm32"),
    allow(dead_code, unused_imports, mismatched_lifetime_syntaxes)
)]
mod app_config;
mod config;
mod ecs;
mod engine;
mod graphics;
mod resources;

// --- Macros ---

pub use ecs::{Component, ComponentStorage, GlobalComponent, GlobalComponentStorage};
pub use pill_core::PillTypeMapKey;

#[cfg(feature = "headless")]
pub use graphics::DummyRenderer;

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
            CameraAspectRatio, CameraComponent, Component, ComponentStorage,
            EntityHandle, GamepadAxis, GamepadButton, GlobalComponent, GlobalComponentStorage,
            InputComponent, MeshRenderingComponent, PlayerId, SceneHandle, TimeComponent,
            TransformComponent, UpdatePhase,
        },
        engine::{Engine, KeyboardKey, MouseButton, PillGame},
        resources::{
            Material, MaterialHandle, Mesh, MeshHandle, Resource, ResourceLoader, ResourceStorage,
            Shader, ShaderParameterSlot, ShaderParameterType, ShaderTextureSlot, Texture,
            TextureHandle, TextureType,
        },
    };

    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::{
        ecs::{AudioListenerComponent, AudioManagerComponent, AudioSourceComponent, SoundType},
        resources::Sound,
    };

    extern crate pill_core;
    pub use pill_core::{
        create_game, define_new_pill_slotmap_key, Color, PillTypeMapKey, Vector2f, Vector2i,
        Vector3f, DISTINCT_COLOR_PALETTE,
    };

    extern crate anyhow;
    pub use anyhow::{Context, Error, Result};
}

#[cfg(not(target_arch = "wasm32"))]
mod internal_mod {
    pub use crate::app_config::EngineConfig;
    pub use crate::{
        config::*,
        ecs::{
            client_go_offline, get_model_matrix, get_normal_matrix,
            get_renderer_resource_handle_from_camera_component, networking_system_client,
            networking_system_server, update_transform_matrices, AudioListenerComponent,
            AudioManagerComponent, AudioSourceComponent, CameraAspectRatio, CameraComponent,
            ComponentStorage, EntityHandle, EntityUpdate, InputComponent,
            MeshRenderingComponent, NetworkEntityAction, NetworkEntityState,
            NetworkManagerComponent, NetworkSide, NetworkStateComponent, NetworkUpdatePayload,
            Scene, TimeComponent, TransformComponent,
        },
        engine::{Engine, PillGame},
        graphics::{
            decompose_render_queue_key, PillRenderer, RenderQueueItem, RenderQueueKey,
            RenderQueueKeyFields, RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle,
            RendererShaderHandle, RendererTextureHandle, RENDER_QUEUE_KEY_ORDER,
        },
        resources::{
            get_renderer_texture_handle_from_material_texture, Material, MaterialHandle,
            MaterialParameter, MaterialTexture, Mesh, MeshData, MeshHandle, MeshVertex,
            ResourceLoader, ResourceManager, ShaderParameterSlot, ShaderParameterType,
            ShaderTextureSlot, Texture, TextureHandle, TextureType,
        },
    };
}

#[cfg(target_arch = "wasm32")]
mod internal_mod {
    pub use crate::app_config::EngineConfig;
    pub use crate::{
        config::*,
        ecs::{
            get_model_matrix, get_normal_matrix,
            get_renderer_resource_handle_from_camera_component, update_transform_matrices,
            CameraAspectRatio, CameraComponent, ComponentStorage,
            EntityHandle, InputComponent, MeshRenderingComponent, Scene, TimeComponent,
            TransformComponent,
        },
        engine::{Engine, PillGame},
        graphics::{
            decompose_render_queue_key, PillRenderer, RenderQueueItem, RenderQueueKey,
            RenderQueueKeyFields, RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle,
            RendererShaderHandle, RendererTextureHandle, RENDER_QUEUE_KEY_ORDER,
        },
        resources::{
            get_renderer_texture_handle_from_material_texture, Material, MaterialHandle,
            MaterialParameter, MaterialTexture, Mesh, MeshData, MeshHandle, MeshVertex,
            ResourceLoader, ResourceManager, ShaderParameterSlot, ShaderParameterType,
            ShaderTextureSlot, Texture, TextureHandle, TextureType,
        },
    };
}

#[cfg(feature = "internal")]
pub mod internal {
    pub use super::internal_mod::*;
}

#[cfg(not(feature = "internal"))]
pub(crate) mod internal {
    pub use super::internal_mod::*;
}
