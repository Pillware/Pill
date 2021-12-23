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



// This will be initialized in runtime instead of compile-time 
// (this is the cost of not using const function, const functions do not allow for generic variables bound by traits different than Sized)
lazy_static! { 
    
}

