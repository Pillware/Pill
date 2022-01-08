use crate::resources::{
    RendererPipeline, 
    RendererMaterial, 
    RendererTexture, 
    RendererMesh, 
    RendererCamera
};

use pill_core::PillSlotMap;

use pill_engine::internal::{
    RendererCameraHandle,
    RendererMaterialHandle,
    RendererMeshHandle,
    RendererPipelineHandle,
    RendererTextureHandle, 
};


pub struct RendererResourceStorage {
    pub(crate) pipelines: PillSlotMap::<RendererPipelineHandle, RendererPipeline>,
    pub(crate) materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>,
    pub(crate) textures: PillSlotMap<RendererTextureHandle, RendererTexture>,
    pub(crate) meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>,
    pub(crate) cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>,
}

impl RendererResourceStorage { // [TODO] move magic values to config
    pub fn new() -> Self {
        RendererResourceStorage {
            pipelines: PillSlotMap::<RendererPipelineHandle, RendererPipeline>::with_capacity_and_key(10), 
            textures: PillSlotMap::<RendererTextureHandle, RendererTexture>::with_capacity_and_key(10),
            materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>::with_capacity_and_key(10),
            meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>::with_capacity_and_key(10),
            cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>::with_capacity_and_key(10),
        }
    }
}