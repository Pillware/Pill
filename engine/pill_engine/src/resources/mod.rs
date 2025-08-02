#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]

mod resource_manager;
#[cfg(feature = "rendering")]
mod mesh;
#[cfg(feature = "rendering")]
mod texture;
mod resource_storage;
#[cfg(feature = "rendering")]
mod material;
mod resource;
#[cfg(feature = "rendering")]
mod sound;

// --- Use ---

pub use resource_manager::ResourceManager;

pub use resource::{
    Resource,
    ResourceLoadType,
};

pub use resource_storage::ResourceStorage;

#[cfg(feature = "rendering")]
pub use sound::{
    Sound,
    SoundHandle,
};

#[cfg(feature = "rendering")]
pub use mesh::{
    Mesh,
    MeshData,
    MeshVertex,
    MeshHandle
};

#[cfg(feature = "rendering")]
pub use texture::{
    Texture,
    TextureType,
    TextureHandle
};

#[cfg(feature = "rendering")]
pub use material::{
    Material,
    MaterialTextureMap,
    MaterialParameter,
    MaterialTexture,
    MaterialParameterMap,
    MaterialHandle,
    get_renderer_texture_handle_from_material_texture,
};
