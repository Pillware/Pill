pub const MAX_INSTANCE_PER_DRAWCALL_COUNT: usize = 10000;
pub const INITIAL_INSTANCE_VECTOR_CAPACITY: usize = 10000;

// Order of bind group layouts in all shaders
pub const PARAMETERS_BIND_GROUP_LAYOUT_INDEX: u32 = 0; // (set = 0, binding = X)
pub const TEXTURES_BIND_GROUP_LAYOUT_INDEX: u32 = 1;   // (set = 1, binding = X)

// Parameter bindings indices
pub const ENGINE_PARAMETERS_BINDING_INDEX: u32 = 0;    // (set = 0, binding = 0)
pub const CAMERA_PARAMETERS_BINDING_INDEX: u32 = 1;    // (set = 0, binding = 1)
pub const MATERIAL_PARAMETERS_BINDING_INDEX: u32 = 2;  // (set = 0, binding = 2)
