use crate::{
    resources::{ TextureHandle, MaterialHandle, TextureType }, 
    graphics::{ RendererTextureHandle, RendererMaterialHandle },
};

use pill_core::PillSlotMapKeyData;

use std::num::NonZeroU32;
use lazy_static::lazy_static;

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

// This will be initialized in runtime instead of compile-time 
// (this is the cost of not using const function, const functions do not allow for generic variables bound by traits different than Sized)
lazy_static! { 
    
}


