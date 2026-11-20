//! The GPU textures the renderer samples and renders into.
//!
//! # Responsibilities
//!
//! - Own the texture, view, and sampler triple behind every texture handle,
//!   so a resource that exists is complete ([`RendererTexture`]).
//! - Turn decoded texels into a texture under the asset's name
//!   ([`RendererTexture::new_texture`]): RGBA8 colour and normal maps, a
//!   half-float equirectangular panorama, or a six-face half-float cubemap.
//! - Create offscreen colour targets at a caller-chosen format, and the
//!   surface-sized depth buffer ([`RendererTexture::new_render_target`],
//!   [`RendererTexture::new_depth_texture`]).
//!
//! # Design
//!
//! One type serves uploaded assets, offscreen passes, and depth because every
//! consumer needs the same three handles. The three constructors differ only
//! in how they describe the texture and its sampler; uploading an asset's
//! texels is the one extra step. The calls wgpu validates - creating the
//! texture, the upload and creating the sampler - go through
//! `capturing_validation`, so a refusal it can wait for comes back as an error
//! the caller reports against this resource, instead of a log line from the
//! device's uncaptured-error handler.

// Current crate
use crate::{
    assets::TextureType,
    error::{capturing_validation, Result},
};

// =============================================================================
// Handle
// =============================================================================

pill_core::define_slot_key!(RendererTextureHandle);

// =============================================================================
// RendererTexture
// =============================================================================

/// A texture together with the view and sampler that make it usable.
///
/// One type covers every texture the renderer holds: uploaded assets,
/// offscreen colour targets, and depth.
pub struct RendererTexture {
    /// The texture the view and the sampler belong to.
    pub texture: wgpu::Texture,
    /// The view a pass binds the texture through.
    pub texture_view: wgpu::TextureView,
    /// How a shader reads a texel: the filter and address modes in use.
    pub sampler: wgpu::Sampler,
    /// The shape the view was created with: a 2D image, or a cube.
    ///
    /// A texture slot declares the same shape. Binding a texture to a slot
    /// that wants the other shape is refused with both names, instead of
    /// failing later in wgpu's own validation message.
    pub view_dimension: wgpu::TextureViewDimension,
}

impl RendererTexture {
    /// Pixel format of the renderer's depth buffer.
    ///
    /// Pipeline state that renders into the buffer has to declare the same
    /// format the attachment was created with, so the value sits beside the
    /// constructor that creates it.
    pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

    /// Creates a texture, view, and sampler from decoded texels.
    ///
    /// `texel_bytes` holds the texels the way the texture asset stores them
    /// for `texture_type`. `name` labels the texture and appears in every
    /// error that mentions it.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::RendererError::Other`] naming the asset when
    /// `texel_bytes` does not match the declared size and type, or when the
    /// driver refuses the texture, the upload, or the sampler.
    pub fn new_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: Option<&str>,
        texel_bytes: &[u8],
        width: u32,
        height: u32,
        texture_type: TextureType,
    ) -> Result<Self> {
        // Step 1: Check that the bytes match the declared size and type.
        // `write_texture` fails validation when they do not, with a report
        // about the copy itself; this check names the asset and gives the
        // expected byte count instead.
        let layer_count = texture_type.layer_count();
        let bytes_per_texel = texture_type.bytes_per_texel() as u32;
        let expected_bytes =
            width as usize * height as usize * bytes_per_texel as usize * layer_count as usize;
        if texel_bytes.len() != expected_bytes {
            return Err(crate::error::RendererError::Other {
                detail: format!(
                    "texture `{}` declares {width}x{height} {texture_type:?} but carries {} bytes (expected {expected_bytes})",
                    name.unwrap_or("<unnamed>"),
                    texel_bytes.len()
                ),
            });
        }

        // Step 2: Describe the texture. A cubemap is six layers of one square
        // face.
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: layer_count,
        };

        let format = match texture_type {
            TextureType::Color => wgpu::TextureFormat::Rgba8UnormSrgb,
            TextureType::Normal => wgpu::TextureFormat::Rgba8Unorm,
            // An environment's range runs well past 1.0, and a half float is
            // the widest format every backend can filter.
            TextureType::Equirect | TextureType::Cubemap => wgpu::TextureFormat::Rgba16Float,
        };

        // Step 3: Create the texture and upload its texels. Both calls run
        // inside an error scope, so a refusal comes back as an error naming
        // this texture instead of a log line from the device's error handler.
        let texture = capturing_validation(device, || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: name,
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        })
        .map_err(|detail| texture_failure(name, "creation", detail))?;

        capturing_validation(device, || {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                texel_bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_texel * width),
                    rows_per_image: Some(height),
                },
                size,
            )
        })
        .map_err(|detail| texture_failure(name, "upload", detail))?;

        // Step 4: Create the view. A cubemap's six layers are one cube to a
        // shader.
        let view_dimension = match texture_type {
            TextureType::Cubemap => wgpu::TextureViewDimension::Cube,
            _ => wgpu::TextureViewDimension::D2,
        };
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(view_dimension),
            ..Default::default()
        });

        // Step 5: Create the sampler. Magnification and minification both
        // filter linearly, so a texel is blended in either direction. The
        // texture has no mip levels, so the mipmap filter never applies.
        // Images tile in both directions; a panorama repeats around its seam
        // and stops at its poles; a cube's faces stop at their own edges,
        // where the hardware crosses to the next face.
        let (address_u, address_v) = match texture_type {
            TextureType::Equirect => (wgpu::AddressMode::Repeat, wgpu::AddressMode::ClampToEdge),
            TextureType::Cubemap => (
                wgpu::AddressMode::ClampToEdge,
                wgpu::AddressMode::ClampToEdge,
            ),
            _ => (wgpu::AddressMode::Repeat, wgpu::AddressMode::Repeat),
        };
        let sampler = capturing_validation(device, || {
            device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: address_u,
                address_mode_v: address_v,
                address_mode_w: address_v,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                // Both clamps are required by wgpu's validation even when the
                // texture has a single mip level.
                lod_min_clamp: 0.0,
                lod_max_clamp: 100.0,
                ..Default::default()
            })
        })
        .map_err(|detail| texture_failure(name, "sampler", detail))?;

        Ok(Self {
            texture,
            texture_view,
            sampler,
            view_dimension,
        })
    }

    /// An offscreen colour target: written by one pass, read by the next.
    ///
    /// The caller picks the format because a chain that ends in tonemapping
    /// has to carry the range the scene produced until its last step. A
    /// format that cannot hold that range clips the picture on the way
    /// through.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::RendererError::Other`] naming the target when
    /// the driver refuses the texture or its sampler.
    pub fn new_render_target(
        device: &wgpu::Device,
        label: &str,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Result<Self> {
        // Step 1: Describe the target. It is one layer, in the caller's
        // format.
        let size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };

        // Step 2: Create the texture.
        let texture = capturing_validation(device, || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        })
        .map_err(|detail| crate::error::RendererError::Other {
            detail: format!("offscreen target `{label}` texture: {detail}"),
        })?;

        // Step 3: Create the view the passes bind.
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Step 4: Create the sampler. A post pass samples the frame it is
        // handed at whatever uv its own shader asks for, and wrapping there
        // would fold the opposite edge of the picture back into it.
        let sampler = capturing_validation(device, || {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        })
        .map_err(|detail| crate::error::RendererError::Other {
            detail: format!("offscreen target `{label}` sampler: {detail}"),
        })?;

        Ok(Self {
            texture,
            texture_view,
            sampler,
            view_dimension: wgpu::TextureViewDimension::D2,
        })
    }

    /// Creates the surface-sized depth buffer, with a sampler for reading it.
    ///
    /// Taking the surface configuration rather than a size keeps the buffer
    /// and the swapchain in step. The renderer rebuilds the depth texture on
    /// every resize by passing the new configuration through here.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::RendererError::Other`] naming the buffer when
    /// the driver refuses the texture or its sampler.
    pub fn new_depth_texture(
        device: &wgpu::Device,
        surface_configuration: &wgpu::SurfaceConfiguration,
        label: &str,
    ) -> Result<Self> {
        // Step 1: Match the surface's size, so the buffer follows the
        // swapchain through every resize.
        let size = wgpu::Extent3d {
            width: surface_configuration.width,
            height: surface_configuration.height,
            depth_or_array_layers: 1,
        };

        // Step 2: Create the texture at the depth format.
        let texture = capturing_validation(device, || {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: Self::DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        })
        .map_err(|detail| crate::error::RendererError::Other {
            detail: format!("depth texture `{label}`: {detail}"),
        })?;

        // Step 3: Create the view a pass binds.
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Step 4: Create a sampler that reads depth values. It is a read
        // sampler, not a comparison one: the depth slot's layout declares a
        // non-filtering sampler, so a comparison sampler could not bind. A
        // lookup reads the depth of the texel under the uv instead of blending
        // across a depth edge.
        let sampler = capturing_validation(device, || {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("depth_read_sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Nearest,
                min_filter: wgpu::FilterMode::Nearest,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                lod_min_clamp: 0.0,
                lod_max_clamp: 100.0,
                ..Default::default()
            })
        })
        .map_err(|detail| crate::error::RendererError::Other {
            detail: format!("depth texture `{label}` sampler: {detail}"),
        })?;

        Ok(Self {
            texture,
            texture_view,
            sampler,
            view_dimension: wgpu::TextureViewDimension::D2,
        })
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Turns a captured wgpu failure into an error naming the texture and the
/// stage of its creation that failed.
fn texture_failure(name: Option<&str>, stage: &str, detail: String) -> crate::error::RendererError {
    crate::error::RendererError::Other {
        detail: format!(
            "texture `{}` failed at {stage}: {detail}",
            name.unwrap_or("<unnamed>")
        ),
    }
}
