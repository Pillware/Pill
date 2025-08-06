#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]

mod resource_manager;
mod mesh;
mod texture;
mod resource_storage;
mod material;
mod resource;
mod sound;

// --- Use ---

pub use resource_manager::ResourceManager;

pub use resource::{
    Resource,
    ResourceLoadType,
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
    MaterialTextureMap, 
    MaterialParameter, 
    MaterialTexture, 
    MaterialParameterMap, 
    MaterialHandle,
    get_renderer_texture_handle_from_material_texture,
};
