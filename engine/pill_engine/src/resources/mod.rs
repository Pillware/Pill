#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]

mod material;
mod mesh;
mod resource;
mod resource_manager;
mod resource_storage;
#[cfg(not(target_arch = "wasm32"))]
mod sound;
mod texture;

// --- Use ---

pub use resource_manager::ResourceManager;

pub use resource::{Resource, ResourceLoader};

pub use resource_storage::ResourceStorage;

#[cfg(not(target_arch = "wasm32"))]
pub use sound::{Sound, SoundHandle};

pub use mesh::{Mesh, MeshData, MeshHandle, MeshVertex};

pub(crate) use texture::parse_cooked_tex;
pub use texture::{Texture, TextureHandle, TextureType};

pub use material::{Material, MaterialHandle};
