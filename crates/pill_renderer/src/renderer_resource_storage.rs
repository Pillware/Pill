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
    pub fn new(max_pipelines_count: usize, max_textures_count: usize, max_materials_count: usize, max_meshes_count: usize, max_cameras_count: usize) -> Self {
        RendererResourceStorage {
            pipelines: PillSlotMap::<RendererPipelineHandle, RendererPipeline>::with_capacity_and_key(max_pipelines_count), 
            textures: PillSlotMap::<RendererTextureHandle, RendererTexture>::with_capacity_and_key(max_textures_count),
            materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>::with_capacity_and_key(max_materials_count),
            meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>::with_capacity_and_key(max_meshes_count),
            cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>::with_capacity_and_key(max_cameras_count),
        }
    }
}