#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod resource_manager;
mod mesh;
mod texture;
mod resource_storage;
mod material;
mod resource;

pub use resource_manager::ResourceManager;
pub use resource::{
    Resource,
    ResourceLoadType,
};


pub use resource_storage::ResourceStorage;
pub use mesh::{Mesh, MeshData, MeshVertex, MeshHandle};
pub use texture::{Texture, TextureType, TextureHandle};
pub use material::{Material, MaterialTextureMap, MaterialParameter, MaterialTexture, MaterialParameterMap, MaterialHandle};
