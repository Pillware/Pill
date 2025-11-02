use std::collections::HashMap;

use pill_core::{
    Handle, RendererBufferTag, RendererCameraTag, RendererMaterialTag, RendererMeshTag,
    RendererPipelineTag, RendererTextureTag, ResourcePool,
};
use pill_engine::internal::{
    RendererBufferHandle, RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle,
    RendererPipelineHandle, RendererTextureHandle,
};

use crate::resources::{
    RendererCamera, RendererMaterial, RendererMesh, RendererPipeline, RendererTexture,
};
use image::{DynamicImage, Rgba, RgbaImage};

#[derive(Hash, PartialEq, Eq, Clone, Copy)]
pub struct PipelineKey(pub u64);

pub struct PipelineCache {
    key_to_handle: HashMap<PipelineKey, RendererPipelineHandle>,
}

impl PipelineCache {
    pub fn new() -> Self {
        Self {
            key_to_handle: HashMap::new(),
        }
    }

    pub fn lookup(&self, key: &PipelineKey) -> Option<RendererPipelineHandle> {
        self.key_to_handle.get(key).copied()
    }

    pub fn insert(&mut self, key: PipelineKey, h: RendererPipelineHandle) {
        self.key_to_handle.insert(key, h);
    }
}

pub struct ResourceManager {
    pub buffers: ResourcePool<RendererBufferTag, wgpu::Buffer>,
    pub pipelines: ResourcePool<RendererPipelineTag, RendererPipeline>,
    pub textures: ResourcePool<RendererTextureTag, RendererTexture>,
    pub materials: ResourcePool<RendererMaterialTag, RendererMaterial>,
    pub meshes: ResourcePool<RendererMeshTag, RendererMesh>,
    pub cameras: ResourcePool<RendererCameraTag, RendererCamera>,
    pub pipeline_cache: PipelineCache,
    // cached defaults
    pub default_color_tex: Option<RendererTextureHandle>,
    pub default_normal_tex: Option<RendererTextureHandle>,
}

impl ResourceManager {
    pub fn new() -> Self {
        Self {
            buffers: ResourcePool::new(),
            pipelines: ResourcePool::new(),
            textures: ResourcePool::new(),
            materials: ResourcePool::new(),
            meshes: ResourcePool::new(),
            cameras: ResourcePool::new(),
            pipeline_cache: PipelineCache::new(),
            default_color_tex: None,
            default_normal_tex: None,
        }
    }

    #[inline]
    pub fn buffer(&self, h: RendererBufferHandle) -> &wgpu::Buffer {
        self.buffers.get(h).expect("Invalid buffer handle")
    }

    #[inline]
    pub fn pipeline(&self, h: RendererPipelineHandle) -> &RendererPipeline {
        self.pipelines.get(h).expect("Invalid pipeline handle")
    }

    pub fn ensure_default_textures(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> (RendererTextureHandle, RendererTextureHandle) {
        if self.default_color_tex.is_none() {
            let mut img = RgbaImage::new(1, 1);
            img.put_pixel(0, 0, Rgba([255, 255, 255, 255]));
            let dyn_img = DynamicImage::ImageRgba8(img);
            let tex = RendererTexture::new_texture(
                device,
                queue,
                Some("default_color"),
                &dyn_img,
                pill_engine::internal::TextureType::Color,
            )
            .expect("create default color tex");
            let h = self.textures.insert(tex);
            self.default_color_tex = Some(h);
        }
        if self.default_normal_tex.is_none() {
            let mut img = RgbaImage::new(1, 1);
            // normal map flat (128,128,255)
            img.put_pixel(0, 0, Rgba([128, 128, 255, 255]));
            let dyn_img = DynamicImage::ImageRgba8(img);
            let tex = RendererTexture::new_texture(
                device,
                queue,
                Some("default_normal"),
                &dyn_img,
                pill_engine::internal::TextureType::Normal,
            )
            .expect("create default normal tex");
            let h = self.textures.insert(tex);
            self.default_normal_tex = Some(h);
        }
        (
            self.default_color_tex.unwrap(),
            self.default_normal_tex.unwrap(),
        )
    }
}
