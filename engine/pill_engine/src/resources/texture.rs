#[cfg(not(feature = "headless"))]
use crate::resources::material::set_renderer_textures;
use crate::{
    engine::Engine,
    renderer::resources::RendererTexture,
    resources::{Material, Resource, ResourceLoader, ResourceStorage},
};

use std::borrow::Cow;

use pill_core::{get_type_name, ErrorContext, PillSlotMapKey, PillStyle, PillTypeMapKey, Result};
pill_core::define_new_pill_slotmap_key! {
    pub struct TextureHandle;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureType {
    Color,
    Normal,
    MetallicRoughness,
    Emissive,
}

#[readonly::make]
pub struct Texture {
    #[readonly]
    pub name: String,
    #[readonly]
    pub resource_loader: ResourceLoader,
    #[readonly]
    pub texture_type: TextureType,
}

impl Texture {
    pub fn new(name: &str, texture_type: TextureType, resource_loader: ResourceLoader) -> Self {
        Self {
            name: name.to_string(),
            resource_loader,
            texture_type,
        }
    }

    pub fn from_bytes(name: &str, texture_type: TextureType, bytes: &[u8]) -> Self {
        Self::from_bytes_owned(name, texture_type, bytes.to_vec())
    }

    /// Like `from_bytes`, taking ownership so large runtime-decoded textures aren't copied.
    pub fn from_bytes_owned(name: &str, texture_type: TextureType, bytes: Vec<u8>) -> Self {
        Self::new(
            name,
            texture_type,
            ResourceLoader::Bytes(bytes.into_boxed_slice()),
        )
    }
}

/// Decoded `.cooked_tex`: b"RTEX" | u32 version | u32 width | u32 height (LE); v1 sRGB RGBA8,
/// v2 Rgba32Float, v4 u32 mip_count + Rgba32Float mips (see pill_assets `util/rtex.rs`).
pub(crate) struct CookedTexture<'a> {
    pub(crate) version: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format: wgpu::TextureFormat,
    pub(crate) mips: Vec<&'a [u8]>,
}

pub(crate) fn parse_cooked_tex(bytes: &[u8]) -> Result<CookedTexture<'_>> {
    let invalid = |reason: &str| -> pill_core::PillError {
        format!("not a valid .cooked_tex file: {reason}").into()
    };
    if bytes.len() < 16 || &bytes[0..4] != b"RTEX" {
        return Err(invalid("bad magic or truncated header"));
    }
    let read_u32 =
        |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let version = read_u32(4);
    let width = read_u32(8);
    let height = read_u32(12);

    let (format, mip_count, data_offset) = match version {
        1 => (wgpu::TextureFormat::Rgba8UnormSrgb, 1, 16),
        2 => (wgpu::TextureFormat::Rgba32Float, 1, 16),
        4 => {
            if bytes.len() < 20 {
                return Err(invalid("truncated v4 header"));
            }
            (wgpu::TextureFormat::Rgba32Float, read_u32(16), 20)
        }
        other => return Err(invalid(&format!("unsupported version {other}"))),
    };
    // Validate the header before sizing anything from it (files can be truncated or corrupt)
    if width == 0 || height == 0 {
        return Err(invalid("zero width or height"));
    }
    let max_mip_count = 32 - width.max(height).leading_zeros();
    if mip_count == 0 || mip_count > max_mip_count {
        return Err(invalid(&format!(
            "invalid mip count {mip_count} for {width}x{height}"
        )));
    }
    let bytes_per_texel = format.block_copy_size(None).unwrap() as usize;

    let mut mips = Vec::with_capacity(mip_count as usize);
    let mut offset: usize = data_offset;
    for mip in 0..mip_count {
        let mip_data = ((width >> mip).max(1) as usize)
            .checked_mul((height >> mip).max(1) as usize)
            .and_then(|texels| texels.checked_mul(bytes_per_texel))
            .and_then(|mip_bytes| offset.checked_add(mip_bytes))
            .and_then(|end| bytes.get(offset..end))
            .ok_or_else(|| invalid("pixel data shorter than header declares"))?;
        offset += mip_data.len();
        mips.push(mip_data);
    }

    Ok(CookedTexture {
        version,
        width,
        height,
        format,
        mips,
    })
}

fn owned((pixels, width, height): (Cow<'_, [u8]>, u32, u32)) -> (Cow<'static, [u8]>, u32, u32) {
    (Cow::Owned(pixels.into_owned()), width, height)
}

/// Material textures are RGBA8; float RTEX files are environment/IBL maps for
/// `Engine::load_cooked_texture`.
fn decode_cooked_tex(bytes: &[u8]) -> Result<(Cow<'_, [u8]>, u32, u32)> {
    let cooked = parse_cooked_tex(bytes)?;
    if cooked.version != 1 {
        return Err(pill_core::PillError::from(format!(
            "RTEX v{} is a float texture; load it with Engine::load_cooked_texture into RenderStateComponent",
            cooked.version
        )));
    }
    Ok((Cow::Borrowed(cooked.mips[0]), cooked.width, cooked.height))
}

#[cfg(not(target_arch = "wasm32"))]
fn decode_png(bytes: &[u8]) -> Result<(Cow<'static, [u8]>, u32, u32)> {
    let decoder = png::Decoder::new(bytes);
    let mut reader = decoder
        .read_info()
        .map_err(|e| -> pill_core::PillError { e.to_string().into() })?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| -> pill_core::PillError { e.to_string().into() })?;
    let width = info.width;
    let height = info.height;
    let raw = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => raw.to_vec(),
        png::ColorType::Rgb => raw
            .chunks(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::Grayscale => raw.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => raw
            .chunks(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        _ => {
            let e: pill_core::PillError =
                format!("unsupported PNG color type: {:?}", info.color_type).into();
            return Err(e);
        }
    };
    Ok((Cow::Owned(rgba), width, height))
}

impl PillTypeMapKey for Texture {
    type Storage = ResourceStorage<Texture>;
}

impl Resource for Texture {
    type Handle = TextureHandle;

    fn get_name(&self) -> String {
        self.name.clone()
    }

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        let error_message = format!(
            "Initializing {} {} failed",
            "Resource".general_object_style(),
            get_type_name::<Self>().specific_object_style()
        );

        // Create new renderer texture resource
        let (rgba, width, height) = match &self.resource_loader {
            ResourceLoader::Path(path) => {
                let base = engine.game_resources_directory_path.join(path);
                let cooked_tex_path = base.with_extension("cooked_tex");
                // The mounted asset pack wins over the cooked file on disk
                if let Some(bytes) = engine.packed_asset(&path.with_extension("cooked_tex")) {
                    owned(decode_cooked_tex(&bytes)?)
                } else if cooked_tex_path.exists() {
                    let bytes =
                        std::fs::read(&cooked_tex_path).map_err(|e| -> pill_core::PillError {
                            format!("Failed to read texture {cooked_tex_path:?}: {e}").into()
                        })?;
                    owned(decode_cooked_tex(&bytes)?)
                } else {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        // Check if path to asset is correct
                        pill_core::validate_asset_path(&base, &["png", "cooked_tex"])?;
                        let bytes = std::fs::read(&base).map_err(|e| -> pill_core::PillError {
                            format!("Failed to read texture {base:?}: {e}").into()
                        })?;
                        if base.extension().map(|e| e == "cooked_tex").unwrap_or(false) {
                            owned(decode_cooked_tex(&bytes)?)
                        } else {
                            decode_png(&bytes)?
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        return Err(pill_core::PillError::from(format!(
                            "No preprocessed .cooked_tex found for {:?}; run `pill_launcher -a assets`",
                            base
                        )));
                    }
                }
            }
            ResourceLoader::Bytes(bytes) => {
                if bytes.starts_with(b"RTEX") {
                    decode_cooked_tex(bytes)?
                } else {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        decode_png(bytes)?
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        return Err(pill_core::PillError::from(
                            "Texture::from_bytes on wasm requires pre-converted COOKED_TEX format",
                        ));
                    }
                }
            }
        };

        // Create renderer texture resource
        #[cfg(not(feature = "headless"))]
        {
            let renderer_texture = RendererTexture::new_texture(
                engine.renderer.get_device(),
                engine.renderer.get_queue(),
                &self.name,
                &rgba,
                width,
                height,
                self.texture_type,
            )
            .context(error_message)?;
            engine.resource_manager.add_resource(renderer_texture)?;
        }

        // The pixels live on the GPU now; nothing reads the source bytes again
        if let ResourceLoader::Bytes(bytes) = &mut self.resource_loader {
            *bytes = Box::default();
        }

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {
        #[cfg(not(feature = "headless"))]
        engine
            .resource_manager
            .remove_resource_by_name::<RendererTexture>(&self.name)?;

        // Clear slots referencing this texture (CPU side), then rebuild their GPU bind groups
        let texture_handle = TextureHandle::from(self_handle.data());
        let affected_material_names: Vec<String> = engine
            .resource_manager
            .get_resource_storage_mut::<Material>()?
            .data
            .iter_mut()
            .filter_map(|(_, slot)| {
                let material = slot.as_mut().expect("Critical: Resource is None");
                material
                    .remove_texture(&texture_handle)
                    .then(|| material.name.clone())
            })
            .collect();

        #[cfg(not(feature = "headless"))]
        for material_name in &affected_material_names {
            let textures = engine
                .resource_manager
                .get_resource_by_name::<Material>(material_name)?
                .renderer_textures(&engine.resource_manager)?;
            set_renderer_textures(engine, material_name, &textures)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse_cooked_tex;

    fn rtex_v4(width: u32, height: u32, mip_count: u32, payload_bytes: usize) -> Vec<u8> {
        let mut bytes = b"RTEX".to_vec();
        for value in [4, width, height, mip_count] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.resize(bytes.len() + payload_bytes, 0);
        bytes
    }

    #[test]
    fn v4_mip_count_0_is_rejected() {
        assert!(parse_cooked_tex(&rtex_v4(4, 4, 0, 256)).is_err());
    }

    #[test]
    fn v4_mip_count_u32_max_is_rejected() {
        assert!(parse_cooked_tex(&rtex_v4(4, 4, u32::MAX, 256)).is_err());
    }

    #[test]
    fn v4_4x4_with_3_mips_parses_all_levels() {
        // 4x4 + 2x2 + 1x1 Rgba32Float texels
        let bytes = rtex_v4(4, 4, 3, (16 + 4 + 1) * 16);
        let cooked = parse_cooked_tex(&bytes).unwrap();
        assert_eq!(cooked.mips.len(), 3);
    }

    #[test]
    fn v4_truncated_payload_is_rejected() {
        assert!(parse_cooked_tex(&rtex_v4(4, 4, 3, 16 * 16)).is_err());
    }
}
