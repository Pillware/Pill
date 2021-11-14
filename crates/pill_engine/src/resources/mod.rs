#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod resource_manager;
mod mesh;
mod texture;
mod resource_storage;

pub use resource_manager::ResourceManager;
pub use resource_storage::ResourceStorage;