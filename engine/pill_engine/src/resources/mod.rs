#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]

mod gpu_resources;
mod material;
mod mesh;
mod model;
mod resource;
mod resource_manager;
mod resource_storage;
mod sound;
mod texture;

// --- Use ---

pub use resource_manager::ResourceManager;

pub use gpu_resources::GpuResources;
pub use resource::{Resource, ResourceLoadType};

pub use resource_storage::ResourceStorage;

pub use sound::{Sound, SoundHandle};

pub use mesh::{Mesh, MeshData, MeshHandle, MeshVertex};

pub use texture::{Texture, TextureHandle, TextureType};

pub use material::{PBRMaterial, PBRMaterialHandle};

pub use model::{Model, ModelHandle, ModelMaterialSlot};
