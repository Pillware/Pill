use pill_engine::internal::TextureType;

use anyhow::*;
use image::GenericImageView;
use std::{ num::NonZeroU32, path::{Path, PathBuf} };

// --- Texture ---

pill_core::define_new_pill_slotmap_key! { 
    pub struct RendererTextureHandle;
}

pub struct RendererTexture {
    pub texture: wgpu::Texture,
    pub texture_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

impl RendererTexture {
    pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

    pub fn new_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: Option<&str>,
        image_data: &image::DynamicImage,
        texture_type: TextureType,
    ) -> Result<Self> {
        let dimensions = image_data.dimensions();
        let rgba = image_data.to_rgba8();

        // Get size
        let size = wgpu::Extent3d {
            width: dimensions.0,
            height: dimensions.1,
            depth_or_array_layers: 1,
        };

        // Specify texture format
        let format = match texture_type {
            TextureType::Color => wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureType::Normal => wgpu::TextureFormat::Rgba8Unorm,
        };

        // Create texture
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: name,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        });

        // Write data to texture
        queue.write_texture(
            wgpu::ImageCopyTexture {
                aspect: wgpu::TextureAspect::All,
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
            },
            &rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: NonZeroU32::new(4 * dimensions.0),
                rows_per_image: NonZeroU32::new(dimensions.1),
            },
            size,
        );

        // Create texture view
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        
        // Create sampler
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // Create final texture
        let texture  = Self {
            texture,
            texture_view,
            sampler,
        };

        Ok(texture)
    }

    pub fn new_depth_texture(
        device: &wgpu::Device,
        surface_configuration: &wgpu::SurfaceConfiguration,
        label: &str,
    ) -> Result<Self> {

        // Get size
        let size = wgpu::Extent3d { // Depth texture needs to be the same size as window
            width: surface_configuration.width,
            height: surface_configuration.height,
            depth_or_array_layers: 1,
        };

         // Create texture
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: Self::DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, // Rendering to this texture so RENDER_ATTACHMENT flag is needed
        });

        // Create texture view
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Create sampler
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            lod_min_clamp: -100.0,
            lod_max_clamp: 100.0,
            ..Default::default()
        });

        // Create final texture
        let texture  = Self {
            texture,
            texture_view,
            sampler,
        };

        Ok(texture)
    }
}