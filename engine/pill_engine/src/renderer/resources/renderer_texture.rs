use crate::resources::TextureType;
use anyhow::Result;
use image::{DynamicImage, GenericImageView};
use wgpu::{Device, Queue, SurfaceConfiguration, TextureFormat};

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
        let size = wgpu::Extent3d {
            width: dimensions.0,
            height: dimensions.1,
            depth_or_array_layers: 1,
        };

        let format = match texture_type {
            TextureType::Gamma => wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureType::Linear => wgpu::TextureFormat::Rgba8Unorm,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: name,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Write with proper row alignment to avoid stretching/scrambling
        // WebGPU requires bytes_per_row to be COPY_BYTES_PER_ROW_ALIGNMENT (256) aligned.
        let bpp: u32 = 4;
        let unpadded_bytes_per_row = (dimensions.0 * bpp) as usize;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
        let padded_bytes_per_row = ((unpadded_bytes_per_row + align - 1) / align) * align;
        if padded_bytes_per_row == unpadded_bytes_per_row {
            // Fast path: already aligned
            queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(unpadded_bytes_per_row as u32),
                    rows_per_image: Some(dimensions.1),
                },
                size,
            );
        } else {
            // Pad each row up to alignment
            let height = dimensions.1 as usize;
            let mut padded = vec![0u8; padded_bytes_per_row * height];
            let src = rgba.as_raw();
            for y in 0..height {
                let src_offset = y * unpadded_bytes_per_row;
                let dst_offset = y * padded_bytes_per_row;
                padded[dst_offset..dst_offset + unpadded_bytes_per_row]
                    .copy_from_slice(&src[src_offset..src_offset + unpadded_bytes_per_row]);
            }
            queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &padded,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row as u32),
                    rows_per_image: Some(dimensions.1),
                },
                size,
            );
        }

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Ok(Self {
            texture,
            texture_view,
            sampler,
        })
    }

    pub fn new_depth_texture(
        device: &wgpu::Device,
        surface_config: &SurfaceConfiguration,
        name: &str,
    ) -> Result<Self> {
        let size = wgpu::Extent3d {
            width: surface_config.width,
            height: surface_config.height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(name),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: Self::DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        Ok(Self {
            texture,
            texture_view,
            sampler,
        })
    }

    pub fn new_render_target(
        device: &wgpu::Device,
        surface_config: &SurfaceConfiguration,
        format: TextureFormat,
        name: &str,
    ) -> Result<Self> {
        let size = wgpu::Extent3d {
            width: surface_config.width,
            height: surface_config.height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(name),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Ok(Self {
            texture,
            texture_view,
            sampler,
        })
    }
}
