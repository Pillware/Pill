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

pub const MAX_PIPELINES: usize = 10;
pub const MAX_TEXTURES: usize = 10;
pub const MAX_MATERIALS: usize = 10;
pub const MAX_MESHES: usize = 10;
pub const MAX_CAMERAS: usize = 10;

pub struct RendererResourceStorage {
    pub(crate) pipelines: PillSlotMap::<RendererPipelineHandle, RendererPipeline>,
    pub(crate) materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>,
    pub(crate) textures: PillSlotMap<RendererTextureHandle, RendererTexture>,
    pub(crate) meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>,
    pub(crate) cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>,
}

impl RendererResourceStorage {
    pub fn new(config: &config::Config) -> Self {
        let max_pipeline_count = config.get_int("MAX_PIPELINES").unwrap_or(MAX_PIPELINES as i64) as usize;
        let max_texture_count = config.get_int("MAX_TEXTURES").unwrap_or(MAX_TEXTURES as i64) as usize;
        let max_material_count = config.get_int("MAX_MATERIALS").unwrap_or(MAX_MATERIALS as i64) as usize;
        let max_mesh_count = config.get_int("MAX_MESHS").unwrap_or(MAX_MESHES as i64) as usize;
        let max_camera_count = config.get_int("MAX_CAMERAS").unwrap_or(MAX_CAMERAS as i64) as usize;

        RendererResourceStorage {
            pipelines: PillSlotMap::<RendererPipelineHandle, RendererPipeline>::with_capacity_and_key(max_pipeline_count), 
            textures: PillSlotMap::<RendererTextureHandle, RendererTexture>::with_capacity_and_key(max_texture_count),
            materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>::with_capacity_and_key(max_material_count),
            meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>::with_capacity_and_key(max_mesh_count),
            cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>::with_capacity_and_key(max_camera_count),
        }
    }
}