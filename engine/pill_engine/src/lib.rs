#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod engine;
mod resources;
mod ecs;
mod config;

#[cfg(feature = "rendering")]
mod graphics;

// ───────────────────────────────────────── Public top-level ────────────────────
pub use engine::{Engine, PillGame};

#[cfg(feature = "net")]
pub use crate::ecs::{
    NetState, NetStats, NetSide,
    NetworkStateComponent, NetEntityState,
    networking_system_server, networking_system_client,
};

// Needed unconditionally by net + gameplay code
pub use crate::ecs::TransformComponent;

// Low-level Pill type-map helpers
pub use pill_core::PillTypeMapKey;
pub use ecs::{Component, GlobalComponent, ComponentStorage, GlobalComponentStorage};

// ───────────────────────────────────────── Helper macros ───────────────────────
#[macro_export]
macro_rules! define_component {
    (
        $name:ident { $( $field_name:ident : $field_ty:ty ),* $(,)? }
    ) => {
        pub struct $name { $( pub $field_name: $field_ty, )* }

        impl $crate::PillTypeMapKey for $name {
            type Storage = $crate::ComponentStorage<$name>;
        }
        impl $crate::Component for $name {}
    };
}

#[macro_export]
macro_rules! define_global_component {
    (
        $name:ident { $( $field_name:ident : $field_ty:ty ),* $(,)? }
    ) => {
        pub struct $name { $( pub $field_name: $field_ty ),* }

        impl $crate::PillTypeMapKey for $name {
            type Storage = $crate::GlobalComponentStorage<$name>;
        }
        impl $crate::GlobalComponent for $name {}
    };
}

// -----------------------------------------------------------------------------
// GAME-SIDE convenience re-exports
// -----------------------------------------------------------------------------
#[cfg(feature = "game")]
pub mod game {
    // --- engine ---------------------------------------------------------------
    pub use crate::engine::{Engine, PillGame};

    // Keyboard / mouse are only present when the rendering feature is on
    #[cfg(feature = "rendering")]
    pub use crate::engine::{KeyboardKey, MouseButton};

    // --- ECS (always available) ----------------------------------------------
    pub use crate::ecs::{
        SceneHandle, EntityHandle, TimeComponent,
        TransformComponent,                    // used by almost every game
        InputComponent, GamepadAxis,           // new game-pad enum
        Component, ComponentStorage,
        GlobalComponent, GlobalComponentStorage,
        SoundType,
    };

    // Extra ECS goodies only when rendering is compiled
    #[cfg(feature = "rendering")]
    pub use crate::ecs::{
        MeshRenderingComponent,
        CameraComponent, CameraAspectRatio,
        AudioSourceComponent,
        AudioListenerComponent,
        AudioManagerComponent,
        EguiManagerComponent,
        get_renderer_resource_handle_from_camera_component,
    };

    // --- Resources ------------------------------------------------------------
    pub use crate::resources::{
        Resource, ResourceStorage,
        Texture, TextureHandle, TextureType,
        Material, MaterialHandle,
        Mesh, MeshHandle,
        ResourceLoadType,
        Sound,
    };

    // Rendering-specific resource types
    #[cfg(feature = "rendering")] pub use crate::resources::{MeshData, MeshVertex};
    #[cfg(feature = "rendering")] pub use crate::resources::{MaterialTexture, MaterialTextureMap};
    #[cfg(feature = "rendering")] pub use crate::resources::{MaterialParameter, MaterialParameterMap};

    // --- pill_core re-exports --------------------------------------------------
    extern crate pill_core;
    pub use pill_core::{
        PillTypeMapKey,
        Vector2f, Vector3f, Color,
        Direction,
        Vector2i, Vector3i,
        define_new_pill_slotmap_key,
        create_game,
    };

    // --- anyhow ---------------------------------------------------------------
    extern crate anyhow;
    pub use anyhow::{Context, Result, Error};
}

// -----------------------------------------------------------------------------
// INTERNAL renderer / tooling re-exports (needs both features)
// -----------------------------------------------------------------------------
#[cfg(all(feature = "internal", feature = "rendering"))]
pub mod internal {
    pub use crate::{
        // engine / config ------------------------------------------------------
        engine::{Engine, PillGame},
        config::*,

        // graphics -------------------------------------------------------------
        graphics::{
            PillRenderer,
            RenderQueueKey, RenderQueueItem, RenderQueueKeyFields,
            decompose_render_queue_key, RENDER_QUEUE_KEY_ORDER,
            RendererCameraHandle, RendererMaterialHandle,
            RendererMeshHandle, RendererPipelineHandle, RendererTextureHandle,
        },

        // ECS ------------------------------------------------------------------
        ecs::{
            Scene, ComponentStorage,
            TransformComponent, MeshRenderingComponent,
            CameraComponent, CameraAspectRatio,
            InputComponent, GamepadAxis,
            EntityHandle,
            TimeComponent,
            AudioSourceComponent, AudioListenerComponent,
            AudioManagerComponent, EguiManagerComponent,
            get_renderer_resource_handle_from_camera_component,
            update_transform_matrices,
            get_model_matrix, get_normal_matrix,
            PostprocessParams,
        },

        // resources ------------------------------------------------------------
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

