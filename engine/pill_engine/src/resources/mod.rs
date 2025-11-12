#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]

mod resource_manager;
mod mesh;
mod texture;
mod resource_storage;
mod material;
mod resource;
mod sound;
mod shader;

// --- Use ---

pub use resource_manager::ResourceManager;

pub use resource::{
    Resource,
    ResourceLoader,
};

pub use resource_storage::ResourceStorage;

pub use sound::{
    Sound,
    SoundHandle,
};

pub use mesh::{ 
    Mesh, 
    MeshData, 
    MeshVertex, 
    MeshHandle 
};

pub use texture::{ 
    Texture, 
    TextureType, 
    TextureHandle 
};

pub use material::{ 
    Material, 
    MaterialParameter, 
    MaterialTexture, 
    MaterialHandle,
    get_renderer_texture_handle_from_material_texture,
};

pub use shader::{ 
    Shader, 
    ShaderHandle, 
    ShaderTextureSlot,
    ShaderParameterSlot,
    ShaderParameterType,
};