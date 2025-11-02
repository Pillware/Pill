use pill_core::{ResourcePool, RendererBufferTag, RendererCameraTag, RendererMaterialTag, RendererMeshTag, RendererPipelineTag, RendererTextureTag};

use crate::resources::{RendererCamera, RendererMaterial, RendererMesh, RendererTexture, RendererPipeline};
use crate::resource_manager::ResourceManager;

pub struct ResourceSnapshot {
    pub buffers: ResourcePool<RendererBufferTag, wgpu::Buffer>,
    pub pipelines: ResourcePool<RendererPipelineTag, RendererPipeline>,
    pub textures: ResourcePool<RendererTextureTag, RendererTexture>,
    pub materials: ResourcePool<RendererMaterialTag, RendererMaterial>,
    pub meshes: ResourcePool<RendererMeshTag, RendererMesh>,
    pub cameras: ResourcePool<RendererCameraTag, RendererCamera>,
}

impl From<ResourceManager> for ResourceSnapshot {
    fn from(m: ResourceManager) -> Self {
        Self {
            buffers: m.buffers,
            pipelines: m.pipelines,
            textures: m.textures,
            materials: m.materials,
            meshes: m.meshes,
            cameras: m.cameras,
        }
    }
}


