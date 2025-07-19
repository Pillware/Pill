use crate::{
    ecs::{ AudioManagerComponent, DeferredUpdateComponent, EguiManagerComponent, InputComponent, TimeComponent }, 
    graphics::{ RendererMaterialHandle, RendererTextureHandle }, 
    resources::{ MaterialHandle, TextureHandle, TextureType }
};

use pill_core::PillSlotMapKeyData;

use std::{num::NonZeroU32, any::TypeId};
use lazy_static::lazy_static;

// --- General ---

pub const PANIC_ON_GAME_ERRORS: bool = true;

// --- ECS ---

pub const MAX_ENTITIES: usize = 1000;
pub const MAX_CONCURRENT_2D_SOUNDS: usize = 10;
pub const MAX_CONCURRENT_3D_SOUNDS: usize = 10;
pub const MAX_CAMERAS: usize = 10;

// --- Resources ---

pub const RESOURCE_VERSION_LIMIT: usize = 255;

pub const MAX_PIPELINES: usize = 10;
pub const MAX_TEXTURES: usize = 10;
pub const MAX_MATERIALS: usize = 10;
pub const MAX_MESHES: usize = 10;
pub const MAX_SOUNDS: usize = 10;

// Convention: All resource names starting with "PillDefault" are restricted, cannot be added and removed from game
pub const DEFAULT_RESOURCE_PREFIX: &str = "PillDefault";
pub const DEFAULT_COLOR_TEXTURE_NAME: &str = "PillDefaultColor";
pub const DEFAULT_NORMAL_TEXTURE_NAME: &str = "PillDefaultNormal";
pub const DEFAULT_MATERIAL_NAME: &str = "PillDefaultMaterial";

// Master material
pub const MASTER_SHADER_COLOR_TEXTURE_SLOT: &str = "Color";
pub const MASTER_SHADER_NORMAL_TEXTURE_SLOT: &str = "Normal";
pub const MASTER_SHADER_TINT_PARAMETER_SLOT: &str = "Tint";
pub const MASTER_SHADER_SPECULARITY_PARAMETER_SLOT: &str = "Specularity";

// Render queue key
pub type RenderQueueKeyType = u64; // Defines size of renderer queue key (Should be u8, u16, u32, or u64)

pub const RENDER_QUEUE_KEY_ITEMS_LENGTH: [RenderQueueKeyType; 5] = [5, 8, 8, 8, 8]; // Defines size of next render queue key parts (bits from left to right)

// Indices of render queue key parts (maps RENDER_QUEUE_KEY_ITEMS_LENGTH)
pub const RENDER_QUEUE_KEY_ORDER_IDX: u8 = 0;
pub const RENDER_QUEUE_KEY_MATERIAL_INDEX_IDX: u8 = 1;
pub const RENDER_QUEUE_KEY_MATERIAL_VERSION_IDX: u8 = 2;
pub const RENDER_QUEUE_KEY_MESH_INDEX_IDX: u8 = 3;
pub const RENDER_QUEUE_KEY_MESH_VERSION_IDX: u8 = 4;

// Default resource handle - Color texture
pub const DEFAULT_COLOR_TEXTURE_HANDLE: TextureHandle = TextureHandle { 
    0: PillSlotMapKeyData { index: 1, version: unsafe { std::num::NonZeroU32::new_unchecked(1) } } 
};
pub const DEFAULT_RENDERER_COLOR_TEXTURE_HANDLE: RendererTextureHandle = RendererTextureHandle { 
    0: PillSlotMapKeyData { index: 1, version: unsafe { std::num::NonZeroU32::new_unchecked(1) } } 
};

// Default resource handle - Normal texture
pub const DEFAULT_NORMAL_TEXTURE_HANDLE: TextureHandle = TextureHandle { 
    0: PillSlotMapKeyData { index: 2, version: unsafe { std::num::NonZeroU32::new_unchecked(1) } } 
};
pub const DEFAULT_RENDERER_NORMAL_TEXTURE_HANDLE: RendererTextureHandle = RendererTextureHandle { 
    0: PillSlotMapKeyData { index: 2, version: unsafe { std::num::NonZeroU32::new_unchecked(1) } } 
};

pub fn get_default_texture_handles(texture_type: TextureType) -> (TextureHandle, RendererTextureHandle) {
    match texture_type {
        TextureType::Color => (DEFAULT_COLOR_TEXTURE_HANDLE, DEFAULT_RENDERER_COLOR_TEXTURE_HANDLE),
        TextureType::Normal => (DEFAULT_NORMAL_TEXTURE_HANDLE, DEFAULT_RENDERER_NORMAL_TEXTURE_HANDLE),
    }
}


// Default resource handle - Material
pub const DEFAULT_MATERIAL_HANDLE: MaterialHandle = MaterialHandle { 
    0: PillSlotMapKeyData { index: 1, version: unsafe { std::num::NonZeroU32::new_unchecked(1) } } 
};
pub const DEFAULT_RENDERER_MATERIAL_HANDLE: RendererMaterialHandle = RendererMaterialHandle { 
    0: PillSlotMapKeyData { index: 1, version: unsafe { std::num::NonZeroU32::new_unchecked(1) } } 
};

pub fn get_default_material_handles() -> (MaterialHandle, RendererMaterialHandle) {
    (DEFAULT_MATERIAL_HANDLE, DEFAULT_RENDERER_MATERIAL_HANDLE)
}

lazy_static! {
    pub static ref ENGINE_GLOBAL_COMPONENTS: Vec<TypeId> = vec!(
        TypeId::of::<InputComponent>(),
        TypeId::of::<TimeComponent>(),
        TypeId::of::<AudioManagerComponent>(),
        TypeId::of::<DeferredUpdateComponent>(),
        TypeId::of::<EguiManagerComponent>()
    );
}