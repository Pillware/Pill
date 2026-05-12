use crate::{
    ecs::{
        deferred_update_system, input_system, rendering_system, time_system,
        DeferredUpdateComponent, InputComponent, PlayerId, SystemFunction,
        TimeComponent, UpdatePhase,
    },
    graphics::{RendererMaterialHandle, RendererShaderHandle, RendererTextureHandle},
    resources::{MaterialHandle, ShaderHandle, TextureHandle, TextureType},
};

#[cfg(not(target_arch = "wasm32"))]
use crate::ecs::{audio_system, haptics_system, AudioManagerComponent};

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

// Default lit shader
pub const DEFAULT_LIT_SHADER_NAME: &str = "pill_engine_default_lit_shader";
pub const DEFAULT_LIT_SHADER_COLOR_TEXTURE_SLOT_NAME: &str = "color";
pub const DEFAULT_LIT_SHADER_COLOR_TEXTURE_SLOT_BINDINGS: (u32, u32) = (0, 1);
pub const DEFAULT_LIT_SHADER_NORMAL_TEXTURE_SLOT_NAME: &str = "normal";
pub const DEFAULT_LIT_SHADER_NORMAL_TEXTURE_SLOT_BINDINGS: (u32, u32) = (2, 3);
pub const DEFAULT_LIT_SHADER_TINT_PARAMETER_SLOT_NAME: &str = "tint";
pub const DEFAULT_LIT_SHADER_SPECULARITY_PARAMETER_SLOT_NAME: &str = "specularity";
pub const DEFAULT_LIT_MATERIAL_NAME: &str = "pill_engine_default_lit_material";

pub const DEFAULT_UNLIT_SHADER_NAME: &str = "pill_engine_default_unlit_shader";
pub const DEFAULT_UNLIT_SHADER_COLOR_TEXTURE_SLOT_NAME: &str = "color";
pub const DEFAULT_UNLIT_SHADER_COLOR_TEXTURE_SLOT_BINDINGS: (u32, u32) = (0, 1);
pub const DEFAULT_UNLIT_SHADER_TINT_PARAMETER_SLOT_NAME: &str = "tint";
pub const DEFAULT_UNLIT_MATERIAL_NAME: &str = "pill_engine_default_unlit_material";

// Render queue key
pub type RenderQueueKeyType = u64; // Defines size of renderer queue key (Should be u8, u16, u32, or u64)

pub const RENDER_QUEUE_KEY_ITEMS_LENGTH: [RenderQueueKeyType; 7] = [5, 8, 8, 8, 8, 8, 8]; // Defines size of next render queue key parts (bits from left to right)

// 64-bit render sort key layout (u64)
// +---------+--------------+---------------------------------------------------------+
// | Bits    |   Field      |  Description                                            |
// +---------+--------------+---------------------------------------------------------+
// |  0..4  | Order        |  5 bits: render order (max 32)                           |
// |  5..12 | Shader Idx   |  8 bits: shader index (max 256)                          |
// | 13..20 | Shader Ver   |  8 bits: shader version (max 256)                        |
// | 21..28 | Material Idx |  8 bits: material index (max 256)                        |
// | 29..36 | Material Ver |  8 bits: material version (max 256)                      |
// | 37..44 | Mesh Idx     |  8 bits: mesh index (max 256)                            |
// | 45..52 | Mesh Ver     |  8 bits: mesh version (max 256)                          |
// | 53..63 | Free         | 11 bits: free for future use                             |
// +---------+--------------+---------------------------------------------------------+

// Indices of render queue key parts (maps RENDER_QUEUE_KEY_ITEMS_LENGTH)
pub const RENDER_QUEUE_KEY_RENDERING_ORDER_IDX: u8 = 0;
pub const RENDER_QUEUE_KEY_SHADER_INDEX_IDX: u8 = 1;
pub const RENDER_QUEUE_KEY_SHADER_VERSION_IDX: u8 = 2;
pub const RENDER_QUEUE_KEY_MATERIAL_INDEX_IDX: u8 = 3;
pub const RENDER_QUEUE_KEY_MATERIAL_VERSION_IDX: u8 = 4;
pub const RENDER_QUEUE_KEY_MESH_INDEX_IDX: u8 = 5;
pub const RENDER_QUEUE_KEY_MESH_VERSION_IDX: u8 = 6;

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
        let mut v = vec![
            TypeId::of::<InputComponent>(),
            TypeId::of::<TimeComponent>(),
            TypeId::of::<DeferredUpdateComponent>(),
        ];
        #[cfg(not(target_arch = "wasm32"))]
        v.push(TypeId::of::<AudioManagerComponent>());
        #[cfg(feature = "debug_ui")]
        v.push(TypeId::of::<crate::ecs::EguiManagerComponent>());
        v
    };
}
