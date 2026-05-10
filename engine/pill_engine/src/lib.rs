#![cfg_attr(
    any(debug_assertions, target_arch = "wasm32"),
    allow(dead_code, unused_imports, mismatched_lifetime_syntaxes)
)]
mod app_config;
mod config;
mod ecs;
mod engine;
mod graphics;
pub mod renderer;
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
            CameraAspectRatio, CameraComponent, Component, ComponentStorage, EntityHandle,
            GamepadAxis, GamepadButton, GlobalComponent, GlobalComponentStorage, InputComponent,
            MeshComponent, PbrRenderableComponent, PlayerId, RenderStateComponent, SceneHandle,
            TimeComponent, TransformComponent, UpdatePhase,
        },
        engine::{Engine, KeyboardKey, MouseButton, PillGame},
        graphics::RendererTextureHandle,
        resources::{
            Material, MaterialHandle, Mesh, MeshHandle, PBRMaterial, PBRMaterialHandle, Resource,
            ResourceLoader, ResourceStorage, Shader, ShaderParameterSlot, ShaderParameterType,
            ShaderTextureSlot, Texture, TextureHandle, TextureType,
        },
    };

    #[cfg(not(target_arch = "wasm32"))]
    pub use crate::{
        ecs::{AudioListenerComponent, AudioManagerComponent, AudioSourceComponent, SoundType},
        resources::Sound,
    };

    #[cfg(feature = "physics")]
    pub use crate::ecs::{
        ColliderComponent, LockedAxes, PhysicsWorldComponent, RigidBodyComponent, RigidBodyType,
        SharedShape,
    };

    #[cfg(feature = "ui")]
    pub use crate::ecs::EguiComponent;
    #[cfg(feature = "ui")]
    pub use egui;

    extern crate pill_core;
    pub use pill_core::{
        create_game, define_new_pill_slotmap_key, Color, PillTypeMapKey, Vector2f, Vector2i,
        Vector3f, DISTINCT_COLOR_PALETTE,
    };

    pub use pill_core::{ErrorContext, PillError, Result};
}

#[cfg(not(target_arch = "wasm32"))]
mod internal_mod {
    pub use crate::app_config::{BuildTarget, CompileMode, EngineConfig, EngineProcessInfo};
    #[cfg(feature = "ui")]
    pub use crate::ecs::EguiClient;
    #[cfg(feature = "ui")]
    pub use crate::ecs::EguiComponent;
    pub use crate::{
        config::*,
        ecs::{
            client_go_offline, get_model_matrix, get_normal_matrix,
            get_renderer_resource_handle_from_camera_component, networking_system_client,
            networking_system_server, update_transform_matrices, AudioListenerComponent,
            AudioManagerComponent, AudioSourceComponent, CameraAspectRatio, CameraComponent,
            ComponentStorage, EntityHandle, EntityUpdate, InputComponent, MeshComponent,
            NetworkEntityAction, NetworkEntityState, NetworkManagerComponent, NetworkSide,
            NetworkStateComponent, NetworkUpdatePayload, PbrRenderableComponent,
            RenderStateComponent, Scene, TimeComponent, TransformComponent,
        },
        engine::{Engine, PillGame},
        graphics::{
            decompose_render_queue_key, BufferDesc, Pass, PillRenderer, PipelineV2, PipelineV2Desc,
            RenderQueueItem, RenderQueueKey, RenderQueueKeyFields, RendererCameraHandle,
            RendererMaterialHandle, RendererMeshHandle, RendererShaderHandle, RendererTargetDesc,
            RendererTextureHandle, ShaderDesc, WorldQuery, RENDER_QUEUE_KEY_ORDER,
        },
        resources::{
            Material, MaterialHandle, MaterialParameter, MaterialTexture, Mesh, MeshData,
            MeshHandle, MeshVertex, PBRMaterial, PBRMaterialHandle, ResourceLoader,
            ResourceManager, ShaderParameterSlot, ShaderParameterType, ShaderTextureSlot, Texture,
            TextureHandle, TextureType,
        },
    };
}

#[cfg(target_arch = "wasm32")]
mod internal_mod {
    pub use crate::app_config::{BuildTarget, CompileMode, EngineConfig, EngineProcessInfo};
    #[cfg(feature = "ui")]
    pub use crate::ecs::EguiClient;
    #[cfg(feature = "ui")]
    pub use crate::ecs::EguiComponent;
    pub use crate::{
        config::*,
        ecs::{
            get_model_matrix, get_normal_matrix,
            get_renderer_resource_handle_from_camera_component, update_transform_matrices,
            CameraAspectRatio, CameraComponent, ComponentStorage, EntityHandle, InputComponent,
            MeshComponent, PbrRenderableComponent, RenderStateComponent, Scene, TimeComponent,
            TransformComponent,
        },
        engine::{Engine, PillGame},
        graphics::{
            decompose_render_queue_key, BufferDesc, Pass, PillRenderer, PipelineV2, PipelineV2Desc,
            RenderQueueItem, RenderQueueKey, RenderQueueKeyFields, RendererCameraHandle,
            RendererMaterialHandle, RendererMeshHandle, RendererShaderHandle, RendererTargetDesc,
            RendererTextureHandle, ShaderDesc, WorldQuery, RENDER_QUEUE_KEY_ORDER,
        },
        resources::{
            Material, MaterialHandle, MaterialParameter, MaterialTexture, Mesh, MeshData,
            MeshHandle, MeshVertex, PBRMaterial, PBRMaterialHandle, ResourceLoader,
            ResourceManager, ShaderParameterSlot, ShaderParameterType, ShaderTextureSlot, Texture,
            TextureHandle, TextureType,
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
