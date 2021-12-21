#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod resource_manager;
mod mesh;
mod texture;
mod resource_storage;
//mod resource_map;
mod material;

pub use resource_manager::{
    ResourceManager,

    MaterialHandle,
    TextureHandle,
    MeshHandle,

    RendererCameraHandle,
    RendererMaterialHandle,
    RendererMeshHandle,
    RendererTextureHandle,
    RendererPipelineHandle,
    Resource,
    ResourceLoadType,
};
pub use resource_storage::ResourceStorage;
pub use mesh::{Mesh, MeshData, MeshVertex};
pub use texture::{Texture, TextureType};
pub use material::{Material, TextureMap, ParameterMap, MaterialParameter, MaterialTexture, MaterialParameterMap};
//pub use resource_mapxxx::Resource;
