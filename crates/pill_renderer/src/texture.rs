use anyhow::*;
use image::GenericImageView;
use pill_engine::internal::{RendererTextureHandle, TextureType};
use std::{num::NonZeroU32, path::{Path, PathBuf}};

// #[derive(Clone, Copy)]
// pub struct RendererTextureHandle {
//     pub index: u32,
// }

// impl ResourceHandle for RendererTextureHandle
// {
//     fn get_index(&self) -> u32 {
//         self.index
//     }
// }


pub struct RendererTexture {
    pub texture: wgpu::Texture,
    pub texture_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

impl RendererTexture {
    pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

    pub fn new_texture_from_image(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: &PathBuf,
        name: &str,
        texture_type: TextureType,
    ) -> Result<Self> {
        let image = image::open(path)?;
        Self::create_texture(device, queue, &image, Some(name), texture_type)
    }

    pub fn new_texture_from_bytes(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bytes: &[u8],
        name: &str,
        texture_type: TextureType,
    ) -> Result<Self> {
        let image = image::load_from_memory(bytes)?;
        Self::create_texture(device, queue, &image, Some(name), texture_type)
    }

    pub fn new_depth_texture(
        device: &wgpu::Device,
        surface_configuration: &wgpu::SurfaceConfiguration,
        name: &str,
    ) -> Result<Self> {
        Self::create_depth_texture(device, surface_configuration, name)
    }


    fn create_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        image: &image::DynamicImage,
        name: Option<&str>,
        texture_type: TextureType,
    ) -> Result<Self> {
        let dimensions = image.dimensions();
        let rgba = image.to_rgba();

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
            usage: wgpu::TextureUsages::TEXTURE_BINDING  | wgpu::TextureUsages::COPY_DST,
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
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
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

    pub fn create_depth_texture(
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
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
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