use crate::resources::{
    EngineParameters, 
    RendererCamera, 
    RendererMaterial, 
    RendererMesh, 
    RendererShader, 
    RendererTexture
};

use pill_engine::internal::{
    RendererCameraHandle, 
    RendererMaterialHandle, 
    RendererMeshHandle, 
    RendererShaderHandle, 
    RendererTextureHandle 
};

use pill_core::PillSlotMap;
use anyhow::{Context, Error, Ok, Result};


pub const MAX_SHADERS: usize = 10;
pub const MAX_TEXTURES: usize = 10;
pub const MAX_MATERIALS: usize = 10;
pub const MAX_MESHES: usize = 10;
pub const MAX_CAMERAS: usize = 10;

pub struct RendererResourceStorage {
    pub(crate) shaders: PillSlotMap::<RendererShaderHandle, RendererShader>,
    pub(crate) materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>,
    pub(crate) textures: PillSlotMap<RendererTextureHandle, RendererTexture>,
    pub(crate) meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>,
    pub(crate) cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>,
    pub(crate) engine_parameters: EngineParameters,
}

impl RendererResourceStorage {
    pub fn new(device: &wgpu::Device, config: &config::Config) -> Result<Self> {
        let max_shader_count = config.get_int("MAX_SHADERS").unwrap_or(MAX_SHADERS as i64) as usize;
        let max_texture_count = config.get_int("MAX_TEXTURES").unwrap_or(MAX_TEXTURES as i64) as usize;
        let max_material_count = config.get_int("MAX_MATERIALS").unwrap_or(MAX_MATERIALS as i64) as usize;
        let max_mesh_count = config.get_int("MAX_MESHES").unwrap_or(MAX_MESHES as i64) as usize;
        let max_camera_count = config.get_int("MAX_CAMERAS").unwrap_or(MAX_CAMERAS as i64) as usize;

        Ok(RendererResourceStorage {
            shaders: PillSlotMap::<RendererShaderHandle, RendererShader>::with_capacity_and_key(max_shader_count),
            textures: PillSlotMap::<RendererTextureHandle, RendererTexture>::with_capacity_and_key(max_texture_count),
            materials: PillSlotMap::<RendererMaterialHandle, RendererMaterial>::with_capacity_and_key(max_material_count),
            meshes: PillSlotMap::<RendererMeshHandle, RendererMesh>::with_capacity_and_key(max_mesh_count),
            cameras: PillSlotMap::<RendererCameraHandle, RendererCamera>::with_capacity_and_key(max_camera_count),
            engine_parameters: EngineParameters::new(device)?,
        })
    }
}