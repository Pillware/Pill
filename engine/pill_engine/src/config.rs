use crate::{
    ecs::{
        deferred_update_system, input_system, rendering_system, time_system,
        DeferredUpdateComponent, InputComponent, PlayerId, SystemFunction, TimeComponent,
        UpdatePhase,
    },
    graphics::{RendererMaterialHandle, RendererShaderHandle, RendererTextureHandle},
    resources::{MaterialHandle, ShaderHandle, TextureHandle, TextureType},
};

#[cfg(feature = "ui")]
use crate::ecs::egui_system;

#[cfg(not(target_arch = "wasm32"))]
use crate::ecs::{audio_system, haptics_system, AudioManagerComponent};

#[cfg(feature = "physics")]
use crate::ecs::physics_system;

use pill_core::PillSlotMapKeyData;

use lazy_static::lazy_static;
use std::any::TypeId;

// --- General ---

pub const PANIC_ON_GAME_ERRORS: bool = true;

// --- ECS ---

pub const MAX_ENTITIES: usize = 1000;
pub const MAX_CONCURRENT_2D_SOUNDS: usize = 10;
pub const MAX_CONCURRENT_3D_SOUNDS: usize = 10;
pub const MAX_CAMERAS: usize = 10;
pub const NUM_SUPPORTED_GAMEPADS: usize = PlayerId::Player4 as usize + 1; // Maximum number of supported gamepads

pub struct SystemConfig {
    pub name: &'static str,
    pub system_function: SystemFunction,
    pub update_phase: UpdatePhase,
}

pub const INPUT_SYSTEM: SystemConfig = SystemConfig {
    name: "input_system",
    system_function: input_system,
    update_phase: UpdatePhase::PreGame,
};

#[cfg(not(target_arch = "wasm32"))]
pub const HAPTICS_SYSTEM: SystemConfig = SystemConfig {
    name: "haptics_system",
    system_function: haptics_system,
    update_phase: UpdatePhase::PostGame,
};

pub const TIME_SYSTEM: SystemConfig = SystemConfig {
    name: "time_system",
    system_function: time_system,
    update_phase: UpdatePhase::PostGame,
};

#[cfg(not(target_arch = "wasm32"))]
pub const AUDIO_SYSTEM: SystemConfig = SystemConfig {
    name: "audio_system",
    system_function: audio_system,
    update_phase: UpdatePhase::PostGame,
};

pub const DEFERRED_UPDATE_SYSTEM: SystemConfig = SystemConfig {
    name: "deferred_update_system",
    system_function: deferred_update_system,
    update_phase: UpdatePhase::PostGame,
};

pub const RENDERING_SYSTEM: SystemConfig = SystemConfig {
    name: "rendering_system",
    system_function: rendering_system,
    update_phase: UpdatePhase::PostGame,
};

#[cfg(feature = "physics")]
pub const PHYSICS_SYSTEM: SystemConfig = SystemConfig {
    name: "physics_system",
    system_function: physics_system,
    update_phase: UpdatePhase::PostGame,
};

#[cfg(feature = "ui")]
pub const EGUI_SYSTEM: SystemConfig = SystemConfig {
    name: "egui_system",
    system_function: egui_system,
    update_phase: UpdatePhase::PostGame,
};

// --- Resources ---

pub const RESOURCE_VERSION_LIMIT: usize = 255;

pub const MAX_SHADERS: usize = 10;
pub const MAX_MATERIALS: usize = 10;
pub const MAX_TEXTURES: usize = 10;
pub const MAX_MESHES: usize = 10;
pub const MAX_SOUNDS: usize = 10;

// Convention: All resource names starting with "pill_engine_" are restricted, cannot be added and removed from game
pub const DEFAULT_RESOURCE_PREFIX: &str = "pill_engine";
pub const DEFAULT_COLOR_TEXTURE_NAME: &str = "pill_engine_default_color";
pub const DEFAULT_NORMAL_TEXTURE_NAME: &str = "pill_engine_default_normal";
pub const DEFAULT_METALLIC_ROUGHNESS_TEXTURE_NAME: &str = "pill_engine_default_metallic_roughness";
pub const DEFAULT_EMISSIVE_TEXTURE_NAME: &str = "pill_engine_default_emissive";

// RTEX layout: b"RTEX" | u32LE version=1 | u32LE width | u32LE height | raw RGBA bytes
pub const DEFAULT_COLOR_TEXTURE_BYTES: [u8; 20] = [
    b'R', b'T', b'E', b'X', 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 255, 255, 255,
    255, // white albedo
];
pub const DEFAULT_NORMAL_TEXTURE_BYTES: [u8; 20] = [
    b'R', b'T', b'E', b'X', 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 128, 128, 255,
    255, // flat normal (0,0,1)
];
// Neutral metallic_roughness: G=255 (max roughness → scalar passes through), B=0 (metallic=0)
pub const DEFAULT_METALLIC_ROUGHNESS_TEXTURE_BYTES: [u8; 20] = [
    b'R', b'T', b'E', b'X', 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 255, 0, 255,
];
// Black emissive: no emission by default
pub const DEFAULT_EMISSIVE_TEXTURE_BYTES: [u8; 20] = [
    b'R', b'T', b'E', b'X', 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 255,
];

// 1×1 Rgba32Float white — PassBackground equirect fallback; bg_color tints it to the desired solid color
// 1.0f32 LE = 00 00 80 3F
pub const DEFAULT_EQUIRECT_FALLBACK_PIXEL: &[u8] = &[
    0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x80, 0x3F, 0x00, 0x00, 0x80, 0x3F,
];
// 1×1 Rgba8Unorm neutral-gray — IBL diffuse/specular fallback: RGBA = [77, 77, 77, 255] ≈ 0.3 linear
pub const DEFAULT_IBL_FALLBACK_PIXEL: &[u8] = &[77, 77, 77, 255];
// 1×1 Rgba8Unorm — BRDF LUT fallback: R=F0_scale=0.5, G=F0_bias=0.5
pub const DEFAULT_BRDF_LUT_FALLBACK_PIXEL: &[u8] = &[128, 128, 0, 255];

// Default lit shader
pub const DEFAULT_LIT_SHADER_NAME: &str = "pill_engine_default_lit_shader";
pub const DEFAULT_LIT_SHADER_COLOR_TEXTURE_SLOT_NAME: &str = "color";
pub const DEFAULT_LIT_SHADER_COLOR_TEXTURE_SLOT_BINDINGS: (u32, u32) = (0, 1);
pub const DEFAULT_LIT_SHADER_NORMAL_TEXTURE_SLOT_NAME: &str = "normal";
pub const DEFAULT_LIT_SHADER_NORMAL_TEXTURE_SLOT_BINDINGS: (u32, u32) = (2, 3);
pub const DEFAULT_LIT_SHADER_METALLIC_ROUGHNESS_TEXTURE_SLOT_NAME: &str = "metallic_roughness";
pub const DEFAULT_LIT_SHADER_METALLIC_ROUGHNESS_TEXTURE_SLOT_BINDINGS: (u32, u32) = (4, 5);
pub const DEFAULT_LIT_SHADER_EMISSIVE_TEXTURE_SLOT_NAME: &str = "emissive";
pub const DEFAULT_LIT_SHADER_EMISSIVE_TEXTURE_SLOT_BINDINGS: (u32, u32) = (6, 7);
pub const DEFAULT_LIT_SHADER_TINT_PARAMETER_SLOT_NAME: &str = "tint";
pub const DEFAULT_LIT_SHADER_SPECULARITY_PARAMETER_SLOT_NAME: &str = "specularity";
pub const DEFAULT_LIT_SHADER_METALLIC_FACTOR_PARAMETER_SLOT_NAME: &str = "metallic_factor";
pub const DEFAULT_LIT_MATERIAL_NAME: &str = "pill_engine_default_lit_material";

pub const DEFAULT_UNLIT_SHADER_NAME: &str = "pill_engine_default_unlit_shader";
pub const DEFAULT_UNLIT_SHADER_COLOR_TEXTURE_SLOT_NAME: &str = "color";
pub const DEFAULT_UNLIT_SHADER_COLOR_TEXTURE_SLOT_BINDINGS: (u32, u32) = (0, 1);
pub const DEFAULT_UNLIT_SHADER_TINT_PARAMETER_SLOT_NAME: &str = "tint";
pub const DEFAULT_UNLIT_MATERIAL_NAME: &str = "pill_engine_default_unlit_material";

// Render queue key
pub type RenderQueueKeyType = u64;

// 64-bit render sort key layout (MSB → LSB):
// bit: 63        59 58       51 50       43 42       35 34       27 26       19 18       11 10        0
//      [  order   ] [shader_idx] [shader_ver] [ mat_idx ] [ mat_ver ] [mesh_idx ] [mesh_ver ] [  unused  ]
//      [  5 bits  ] [  8 bits  ] [  8 bits  ] [  8 bits  ] [  8 bits  ] [  8 bits  ] [  8 bits  ] [ 11 bits ]

// Default resource handle - Color texture
pub const DEFAULT_COLOR_TEXTURE_HANDLE: TextureHandle = TextureHandle(PillSlotMapKeyData {
    index: 1,
    version: std::num::NonZeroU32::new(1).unwrap(),
});
pub const DEFAULT_RENDERER_COLOR_TEXTURE_HANDLE: RendererTextureHandle =
    RendererTextureHandle(PillSlotMapKeyData {
        index: 1,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

// Default resource handle - Normal texture
pub const DEFAULT_NORMAL_TEXTURE_HANDLE: TextureHandle = TextureHandle(PillSlotMapKeyData {
    index: 2,
    version: std::num::NonZeroU32::new(1).unwrap(),
});
pub const DEFAULT_RENDERER_NORMAL_TEXTURE_HANDLE: RendererTextureHandle =
    RendererTextureHandle(PillSlotMapKeyData {
        index: 2,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

// Default resource handle - MetallicRoughness texture
pub const DEFAULT_METALLIC_ROUGHNESS_TEXTURE_HANDLE: TextureHandle =
    TextureHandle(PillSlotMapKeyData {
        index: 3,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });
pub const DEFAULT_RENDERER_METALLIC_ROUGHNESS_TEXTURE_HANDLE: RendererTextureHandle =
    RendererTextureHandle(PillSlotMapKeyData {
        index: 3,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

// Default resource handle - Emissive texture
pub const DEFAULT_EMISSIVE_TEXTURE_HANDLE: TextureHandle = TextureHandle(PillSlotMapKeyData {
    index: 4,
    version: std::num::NonZeroU32::new(1).unwrap(),
});
pub const DEFAULT_RENDERER_EMISSIVE_TEXTURE_HANDLE: RendererTextureHandle =
    RendererTextureHandle(PillSlotMapKeyData {
        index: 4,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

pub fn get_default_texture_handles(
    texture_type: TextureType,
) -> (TextureHandle, RendererTextureHandle) {
    match texture_type {
        TextureType::Color => (
            DEFAULT_COLOR_TEXTURE_HANDLE,
            DEFAULT_RENDERER_COLOR_TEXTURE_HANDLE,
        ),
        TextureType::Normal => (
            DEFAULT_NORMAL_TEXTURE_HANDLE,
            DEFAULT_RENDERER_NORMAL_TEXTURE_HANDLE,
        ),
        TextureType::MetallicRoughness => (
            DEFAULT_METALLIC_ROUGHNESS_TEXTURE_HANDLE,
            DEFAULT_RENDERER_METALLIC_ROUGHNESS_TEXTURE_HANDLE,
        ),
        TextureType::Emissive => (
            DEFAULT_EMISSIVE_TEXTURE_HANDLE,
            DEFAULT_RENDERER_EMISSIVE_TEXTURE_HANDLE,
        ),
    }
}

// Default resource handle - Shader
pub const DEFAULT_LIT_SHADER_HANDLE: ShaderHandle = ShaderHandle(PillSlotMapKeyData {
    index: 1,
    version: std::num::NonZeroU32::new(1).unwrap(),
});

pub const DEFAULT_LIT_RENDERER_SHADER_HANDLE: RendererShaderHandle =
    RendererShaderHandle(PillSlotMapKeyData {
        index: 1,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

pub fn get_default_lit_shader_handles() -> (ShaderHandle, RendererShaderHandle) {
    (
        DEFAULT_LIT_SHADER_HANDLE,
        DEFAULT_LIT_RENDERER_SHADER_HANDLE,
    )
}

// Default resource handle - Material
pub const DEFAULT_MATERIAL_HANDLE: MaterialHandle = MaterialHandle(PillSlotMapKeyData {
    index: 1,
    version: std::num::NonZeroU32::new(1).unwrap(),
});
pub const DEFAULT_RENDERER_MATERIAL_HANDLE: RendererMaterialHandle =
    RendererMaterialHandle(PillSlotMapKeyData {
        index: 1,
        version: std::num::NonZeroU32::new(1).unwrap(),
    });

pub fn get_default_material_handles() -> (MaterialHandle, RendererMaterialHandle) {
    (DEFAULT_MATERIAL_HANDLE, DEFAULT_RENDERER_MATERIAL_HANDLE)
}

lazy_static! {
    pub static ref ENGINE_GLOBAL_COMPONENTS: Vec<TypeId> = {
        #[allow(unused_mut)]
        let mut component_types = vec![
            TypeId::of::<InputComponent>(),
            TypeId::of::<TimeComponent>(),
            TypeId::of::<DeferredUpdateComponent>(),
        ];
        #[cfg(not(target_arch = "wasm32"))]
        component_types.push(TypeId::of::<AudioManagerComponent>());
        #[cfg(feature = "ui")]
        component_types.push(TypeId::of::<crate::ecs::EguiComponent>());
        component_types
    };
}
