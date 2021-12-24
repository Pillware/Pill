use lazy_static::lazy_static;

// Convention: All resource names starting with "PillDefault" are restricted, cannot be added and removed from game
pub const DEFAULT_RESOURCE_PREFIX: &str = "PillDefaultColor";
pub const DEFAULT_COLOR_TEXTURE_NAME: &str = "PillDefaultColor";
pub const DEFAULT_NORMAL_TEXTURE_NAME: &str = "PillDefaultNormal";
pub const DEFAULT_MATERIAL_NAME: &str = "PillDefaultMaterial";

// Master material
pub const MASTER_SHADER_COLOR_TEXTURE_SLOT: &str = "Color";
pub const MASTER_SHADER_NORMAL_TEXTURE_SLOT: &str = "Normal";
pub const MASTER_SHADER_TINT_PARAMETER_SLOT: &str = "Tint";

// Render queue key
pub type RenderQueueKeyType = u64; // Defines size of renderer queue key (Should be u8, u16, u32, or u64)

pub const RENDER_QUEUE_KEY_ITEMS_LENGTH: [RenderQueueKeyType; 5] = [5, 8, 8, 8, 8]; // Defines size of next render queue key parts (bits from left to right)

// Indices of render queue key parts (maps RENDER_QUEUE_KEY_ITEMS_LENGTH)
pub const RENDER_QUEUE_KEY_ORDER_IDX: u8 = 0;
pub const RENDER_QUEUE_KEY_MATERIAL_INDEX_IDX: u8 = 1;
pub const RENDER_QUEUE_KEY_MATERIAL_VERSION_IDX: u8 = 2;
pub const RENDER_QUEUE_KEY_MESH_INDEX_IDX: u8 = 3;
pub const RENDER_QUEUE_KEY_MESH_VERSION_IDX: u8 = 4;

// This will be initialized in runtime instead of compile-time 
// (this is the cost of not using const function, const functions do not allow for generic variables bound by traits different than Sized)
lazy_static! { 
    
}

