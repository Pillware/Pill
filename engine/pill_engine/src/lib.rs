#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, mismatched_lifetime_syntaxes))]
mod engine;
mod resources;
mod graphics;
mod ecs;
mod config;

// --- Macros ---

pub use pill_core::PillTypeMapKey;
pub use ecs::{Component, GlobalComponent, ComponentStorage, GlobalComponentStorage};

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
        engine::{
            Engine,
            PillGame,
            KeyboardKey,
            MouseButton,
        },
        ecs::{
            SceneHandle,
            MeshRenderingComponent,
            TransformComponent,
            InputComponent,
            PlayerId,
            GamepadAxis,
            GamepadButton,
            CameraComponent,
            CameraAspectRatio,
            EntityHandle,
            AudioSourceComponent,
            AudioListenerComponent,
            TimeComponent,
            UpdatePhase,
            AudioManagerComponent,
            EguiManagerComponent,
            Component,
            ComponentStorage,
            GlobalComponent,
            GlobalComponentStorage,
            SoundType,
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
            ResourceLoader,
            Sound,
            Shader,
            ShaderParameterSlot,
            ShaderTextureSlot,
            ShaderParameterType,
        },
    };

    extern crate pill_core;
    pub use pill_core::{
        PillTypeMapKey,
        Vector2f,
        Vector3f,
        Color,
        Vector2i,
        create_game,
        define_new_pill_slotmap_key,
        DISTINCT_COLOR_PALETTE
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
            RenderQueueKey,
            RenderQueueItem,
            RenderQueueKeyFields,
            decompose_render_queue_key,
            RendererCameraHandle,
            RendererShaderHandle,
            RendererMaterialHandle,
            RendererMeshHandle,
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
            EguiManagerComponent,
            get_renderer_resource_handle_from_camera_component,
            update_transform_matrices,
            get_model_matrix,
            get_normal_matrix,
            NetworkStateComponent,
            networking_system_server,
            networking_system_client,
            NetworkManagerComponent,
            NetworkEntityState,
            EntityUpdate,
            NetworkUpdatePayload,
            NetworkEntityAction,
            NetworkSide,
            client_go_offline
        },
        resources::{
            Texture,
            TextureHandle,
            TextureType,

            Material,
            MaterialHandle,
            ShaderParameterSlot,
            ShaderTextureSlot,
            ShaderParameterType,

            Mesh,
            MeshHandle,
            MeshData,
            MeshVertex,

            ResourceLoader,
            ResourceManager,

            MaterialTexture,
            MaterialParameter,
            get_renderer_texture_handle_from_material_texture,
        },
    };
}

