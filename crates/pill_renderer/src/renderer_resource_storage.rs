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

pub const MAX_PIPELINE_COUNT: usize = 10;
pub const MAX_TEXTURE_COUNT: usize = 10;
pub const MAX_MATERIAL_COUNT: usize = 10;
pub const MAX_MESH_COUNT: usize = 10;
pub const MAX_CAMERA_COUNT: usize = 10;

pub struct RendererResourceStorage {
    pub(crate) pipelines: PillSlotMap::<RendererPipelineHandle, RendererPipeline>,
    pub(crate) materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>,
    pub(crate) textures: PillSlotMap<RendererTextureHandle, RendererTexture>,
    pub(crate) meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>,
    pub(crate) cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>,
}

impl RendererResourceStorage {
    pub fn new(config: &config::Config) -> Self {
        let max_pipeline_count = config.get_int("MAX_PIPELINE_COUNT").unwrap_or(MAX_PIPELINE_COUNT as i64) as usize;
        let max_texture_count = config.get_int("MAX_TEXTURE_COUNT").unwrap_or(MAX_TEXTURE_COUNT as i64) as usize;
        let max_material_count = config.get_int("MAX_MATERIAL_COUNT").unwrap_or(MAX_MATERIAL_COUNT as i64) as usize;
        let max_mesh_count = config.get_int("MAX_MESH_COUNT").unwrap_or(MAX_MESH_COUNT as i64) as usize;
        let max_camera_count = config.get_int("MAX_CAMERA_COUNT").unwrap_or(MAX_CAMERA_COUNT as i64) as usize;

        RendererResourceStorage {
            pipelines: PillSlotMap::<RendererPipelineHandle, RendererPipeline>::with_capacity_and_key(max_pipeline_count), 
            textures: PillSlotMap::<RendererTextureHandle, RendererTexture>::with_capacity_and_key(max_texture_count),
            materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>::with_capacity_and_key(max_material_count),
            meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>::with_capacity_and_key(max_mesh_count),
            cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>::with_capacity_and_key(max_camera_count),
        }
    }
}