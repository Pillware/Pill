#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]

mod resource_manager;
mod mesh;
mod texture;
mod resource_storage;
mod material;
mod resource;
mod sound;
mod shader;
mod material_parameter_store;

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

pub use material_parameter_store::{
    MaterialParametersStore,
    ValueParameter,
    TextureParameter,
    MaterialParameter,
    get_renderer_texture_handle_from_texture_parameter
};

pub use material::{ 
    Material, 
    MaterialHandle,
};

pub use shader::{ 
    Shader, 
    ShaderHandle, 
    ShaderTextureParameterSlot,
    ShaderValueParameterSlot,
    ShaderValueParameterType,
    ShaderType,
};