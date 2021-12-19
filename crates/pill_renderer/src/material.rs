use std::path::Path;
use std::path::PathBuf;
use anyhow::{Result, Context, Error};

use pill_core::na::SliceRange;

use pill_engine::internal::{
    RendererMaterialHandle,
    RendererTextureHandle,
    RendererPipelineHandle,
};

use crate::texture::RendererTexture;


//use crate::texture::RendererTextureHandle;

// #[derive(Clone, Copy)]
// pub struct RendererMaterialHandle {
//     pub index: u32,
// }

// impl ResourceHandle for RendererMaterialHandle
// {
//     fn get_index(&self) -> u32 {
//         self.index
//     }
// }

pub struct RendererMaterial {
    pub pipeline_handle: RendererPipelineHandle,
    pub color_texture_handle: RendererTextureHandle,
    pub normal_texture_handle: RendererTextureHandle,
    pub bind_group: wgpu::BindGroup,
}

impl RendererMaterial {
    pub fn new(
        device: &wgpu::Device,
        name: &str,
        pipeline_handle: RendererPipelineHandle,
        color_texture: &RendererTexture,
        color_texture_handle: RendererTextureHandle,
        normal_texture: &RendererTexture,
        normal_texture_handle: RendererTextureHandle,
        texture_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Result<Self> {

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_texture.texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&color_texture.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&normal_texture.texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&normal_texture.sampler),
                },
            ],
            label: Some(name),
        });

        let renderer_material = Self {
            pipeline_handle,
            color_texture_handle,
            normal_texture_handle,
            bind_group,
        };

        Ok(renderer_material)
    }
}
