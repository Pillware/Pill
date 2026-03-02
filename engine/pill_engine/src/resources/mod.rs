#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]

mod material;
mod mesh;
mod resource;
mod resource_manager;
mod resource_storage;
mod shader;
mod sound;
mod texture;

// --- Use ---

pub use resource_manager::ResourceManager;

pub use resource::{Resource, ResourceLoader};

pub use resource_storage::ResourceStorage;

pub use sound::{Sound, SoundHandle};

pub use mesh::{Mesh, MeshData, MeshHandle, MeshVertex};

pub use texture::{Texture, TextureHandle, TextureType};

pub use material::{
    get_renderer_texture_handle_from_material_texture, Material, MaterialHandle, MaterialParameter,
    MaterialTexture,
};

pub use shader::{
    Shader, ShaderHandle, ShaderParameterSlot, ShaderParameterType, ShaderTextureSlot,
};
