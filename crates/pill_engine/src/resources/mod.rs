#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod resource_manager;
mod mesh;
mod texture;
mod resource_storage;
mod resource_map;
mod material;

pub use resource_manager::{
    ResourceManager,
    ResourceHandle,
};
pub use resource_storage::ResourceStorage;
pub use mesh::{Mesh, MeshData, MeshHandle};
pub use texture::{Texture, TextureHandle, TextureType};
pub use material::{Material, MaterialHandle};