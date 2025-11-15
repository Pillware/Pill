use super::resources::{
    RendererCamera, RendererMaterial, RendererMesh, RendererPipeline, RendererTexture,
};

use pill_core::PillSlotMap;

use crate::graphics::{
    RendererBufferHandle, RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle,
    RendererPipelineHandle, RendererPipelineV2Handle, RendererTextureHandle,
};

pub const MAX_PIPELINES: usize = 10;
pub const MAX_PIPELINES_V2: usize = 10;
pub const MAX_TEXTURES: usize = 10;
pub const MAX_MATERIALS: usize = 10;
pub const MAX_MESHES: usize = 10;
pub const MAX_CAMERAS: usize = 10;
pub const MAX_BUFFERS: usize = 64;

pub struct RendererResourceStorage {
    pub(crate) pipelines: PillSlotMap<RendererPipelineHandle, RendererPipeline>,
    pub(crate) pipelines_v2: PillSlotMap<RendererPipelineV2Handle, wgpu::RenderPipeline>,
    pub(crate) materials: PillSlotMap<RendererMaterialHandle, RendererMaterial>,
    pub(crate) textures: PillSlotMap<RendererTextureHandle, RendererTexture>,
    pub(crate) meshes: PillSlotMap<RendererMeshHandle, RendererMesh>,
    pub(crate) cameras: PillSlotMap<RendererCameraHandle, RendererCamera>,
    pub(crate) buffers: PillSlotMap<RendererBufferHandle, wgpu::Buffer>,
}

impl RendererResourceStorage {
    pub fn new(config: &crate::config::Config) -> Self {
        let max_pipeline_count = config
            .get_int("MAX_PIPELINES")
            .unwrap_or(MAX_PIPELINES as i64) as usize;
        let max_pipeline_v2_count = config
            .get_int("MAX_PIPELINES_V2")
            .unwrap_or(MAX_PIPELINES_V2 as i64) as usize;
        let max_texture_count = config
            .get_int("MAX_TEXTURES")
            .unwrap_or(MAX_TEXTURES as i64) as usize;
        let max_material_count = config
            .get_int("MAX_MATERIALS")
            .unwrap_or(MAX_MATERIALS as i64) as usize;
        let max_mesh_count = config.get_int("MAX_MESHS").unwrap_or(MAX_MESHES as i64) as usize;
        let max_camera_count = config.get_int("MAX_CAMERAS").unwrap_or(MAX_CAMERAS as i64) as usize;
        let max_buffer_count = config.get_int("MAX_BUFFERS").unwrap_or(MAX_BUFFERS as i64) as usize;

        RendererResourceStorage {
            pipelines:
                PillSlotMap::<RendererPipelineHandle, RendererPipeline>::with_capacity_and_key(
                    max_pipeline_count,
                ),
            pipelines_v2:
                PillSlotMap::<RendererPipelineV2Handle, wgpu::RenderPipeline>::with_capacity_and_key(
                    max_pipeline_v2_count,
                ),
            textures: PillSlotMap::<RendererTextureHandle, RendererTexture>::with_capacity_and_key(
                max_texture_count,
            ),
            materials:
                PillSlotMap::<RendererMaterialHandle, RendererMaterial>::with_capacity_and_key(
                    max_material_count,
                ),
            meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>::with_capacity_and_key(
                max_mesh_count,
            ),
            cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>::with_capacity_and_key(
                max_camera_count,
            ),
            buffers: PillSlotMap::<RendererBufferHandle, wgpu::Buffer>::with_capacity_and_key(
                max_buffer_count,
            ),
        }
    }
}
