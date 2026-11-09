use crate::graphics::RendererTextureHandle;
use crate::resources::{Resource, ResourceStorage, TextureType};

use pill_core::{PillTypeMapKey, Result};

pub struct RendererTexture {
    pub name: String,
    pub texture: wgpu::Texture,
    pub texture_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

impl RendererTexture {
    pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

    pub fn new_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        rgba: &[u8],
        width: u32,
        height: u32,
        texture_type: TextureType,
    ) -> Result<Self> {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let format = match texture_type {
            TextureType::Color => wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureType::Normal => wgpu::TextureFormat::Rgba8Unorm,
            TextureType::MetallicRoughness => wgpu::TextureFormat::Rgba8Unorm,
            TextureType::Emissive => wgpu::TextureFormat::Rgba8UnormSrgb,
        };

        let mip_count = mip_level_count(width, height);

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(name),
            size,
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Mip chain built on the CPU, each level from the previous one
        let mut level = std::borrow::Cow::Borrowed(rgba);
        let (mut level_width, mut level_height) = (width, height);
        for mip in 0..mip_count {
            if mip > 0 {
                level = downsample(&level, level_width, level_height, texture_type).into();
                (level_width, level_height) = ((level_width / 2).max(1), (level_height / 2).max(1));
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &level,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * level_width),
                    rows_per_image: Some(level_height),
                },
                wgpu::Extent3d {
                    width: level_width,
                    height: level_height,
                    depth_or_array_layers: 1,
                },
            );
        }

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Trilinear
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            lod_min_clamp: 0.0,
            lod_max_clamp: (mip_count - 1) as f32,
            ..Default::default()
        });

        Ok(Self {
            name: name.to_string(),
            texture,
            texture_view,
            sampler,
        })
    }

    pub fn new_depth_texture(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        label: &str,
    ) -> Result<Self> {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
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
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            lod_min_clamp: 0.0,
            lod_max_clamp: 100.0,
            ..Default::default()
        });

        Ok(Self {
            name: label.to_string(),
            texture,
            texture_view,
            sampler,
        })
    }

    /// Uploads a 2D texture; `mip_pixels[i]` holds mip `i`, sized
    /// `(base_width >> i) * (base_height >> i) * bytes_per_texel(format)`.
    pub fn new_from_pixels(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        mip_pixels: &[&[u8]],
        base_width: u32,
        base_height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let mip_count = mip_pixels.len() as u32;
        let bytes_per_texel = format
            .block_copy_size(None)
            .expect("new_from_pixels: format must be a color format");

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(name),
            size: wgpu::Extent3d {
                width: base_width,
                height: base_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        for (mip, &mip_data) in mip_pixels.iter().enumerate() {
            let mip = mip as u32;
            let mip_w = (base_width >> mip).max(1);
            let mip_h = (base_height >> mip).max(1);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: mip,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                mip_data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(mip_w * bytes_per_texel),
                    rows_per_image: Some(mip_h),
                },
                wgpu::Extent3d {
                    width: mip_w,
                    height: mip_h,
                    depth_or_array_layers: 1,
                },
            );
        }

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            lod_min_clamp: 0.0,
            lod_max_clamp: 16.0,
            ..Default::default()
        });

        Ok(Self {
            name: name.to_string(),
            texture,
            texture_view,
            sampler,
        })
    }

    pub fn new_render_target(
        device: &wgpu::Device,
        label: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
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
            name: label.to_string(),
            texture,
            texture_view,
            sampler,
        })
    }
}

/// Full chain down to 1×1.
fn mip_level_count(width: u32, height: u32) -> u32 {
    u32::BITS - width.max(height).max(1).leading_zeros()
}

/// Next mip level of RGBA8 pixels: 2×2 box filter (an odd last row/column repeats). Color textures
/// are averaged in linear light (sRGB approximated as gamma 2); normal maps are renormalized.
fn downsample(rgba: &[u8], width: u32, height: u32, texture_type: TextureType) -> Vec<u8> {
    let (next_width, next_height) = ((width / 2).max(1), (height / 2).max(1));
    let srgb = matches!(texture_type, TextureType::Color | TextureType::Emissive);
    let texel = |x: u32, y: u32| {
        let index = ((y.min(height - 1) * width + x.min(width - 1)) * 4) as usize;
        &rgba[index..index + 4]
    };
    let mut output = Vec::with_capacity((next_width * next_height * 4) as usize);
    for y in 0..next_height {
        for x in 0..next_width {
            let block = [
                texel(2 * x, 2 * y),
                texel(2 * x + 1, 2 * y),
                texel(2 * x, 2 * y + 1),
                texel(2 * x + 1, 2 * y + 1),
            ];
            let mut average = [0.0_f32; 4];
            for (channel, value) in average.iter_mut().enumerate() {
                let linear = |byte: u8| {
                    let value = byte as f32 / 255.0;
                    if srgb && channel < 3 {
                        value * value
                    } else {
                        value
                    }
                };
                *value = block
                    .iter()
                    .map(|texel| linear(texel[channel]))
                    .sum::<f32>()
                    / 4.0;
                if srgb && channel < 3 {
                    *value = value.sqrt();
                }
            }
            if let TextureType::Normal = texture_type {
                let normal = [0, 1, 2].map(|channel| average[channel] * 2.0 - 1.0);
                let length = normal.iter().map(|value| value * value).sum::<f32>().sqrt();
                if length > 1e-4 {
                    for channel in 0..3 {
                        average[channel] = normal[channel] / length * 0.5 + 0.5;
                    }
                }
            }
            output.extend(average.map(|value| (value * 255.0 + 0.5) as u8));
        }
    }
    output
}

impl PillTypeMapKey for RendererTexture {
    type Storage = ResourceStorage<RendererTexture>;
}

impl Resource for RendererTexture {
    type Handle = RendererTextureHandle;

    fn get_name(&self) -> String {
        self.name.clone()
    }
}
