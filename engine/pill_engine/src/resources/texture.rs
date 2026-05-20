use crate::{
    engine::Engine,
    graphics::RendererTextureHandle,
    resources::{Material, Resource, ResourceLoader, ResourceStorage},
};

use pill_core::{get_type_name, ErrorContext, PillSlotMapKey, PillStyle, PillTypeMapKey, Result};
pill_core::define_new_pill_slotmap_key! {
    pub struct TextureHandle;
}

#[derive(Clone, Copy, Debug)]
pub enum TextureType {
    Color,
    Normal,
}

#[readonly::make]
pub struct Texture {
    #[readonly]
    pub name: String,
    #[readonly]
    pub resource_loader: ResourceLoader,
    #[readonly]
    pub texture_type: TextureType,
    pub(crate) renderer_resource_handle: Option<RendererTextureHandle>,
}

impl Texture {
    pub fn new(name: &str, texture_type: TextureType, resource_loader: ResourceLoader) -> Self {
        Self {
            name: name.to_string(),
            resource_loader,
            texture_type,
            renderer_resource_handle: None,
        }
    }

    /// Decode image bytes (format auto-detected) into a texture.
    pub fn from_bytes(name: &str, texture_type: TextureType, bytes: &[u8]) -> Self {
        Self::new(
            name,
            texture_type,
            ResourceLoader::Bytes(bytes.to_vec().into_boxed_slice()),
        )
    }
}

fn decode_rtex(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    if bytes.len() < 16 || &bytes[0..4] != b"RTEX" {
        return Err(pill_core::PillError::from("not a valid .rtex file (bad magic or truncated header)"));
    }
    let width = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    let height = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    Ok((bytes[16..].to_vec(), width, height))
}

#[cfg(not(target_arch = "wasm32"))]
fn decode_png(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
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
    Ok((rgba, width, height))
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
                let rtex_path = base.with_extension("rtex");
                if rtex_path.exists() {
                    let bytes = std::fs::read(&rtex_path).map_err(|e| -> pill_core::PillError {
                        format!("Failed to read texture {rtex_path:?}: {e}").into()
                    })?;
                    decode_rtex(&bytes)?
                } else {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        // Check if path to asset is correct
                        pill_core::validate_asset_path(&base, &["png", "rtex"])?;
                        let bytes = std::fs::read(&base).map_err(|e| -> pill_core::PillError {
                            format!("Failed to read texture {base:?}: {e}").into()
                        })?;
                        if base.extension().map(|e| e == "rtex").unwrap_or(false) {
                            decode_rtex(&bytes)?
                        } else {
                            decode_png(&bytes)?
                        }
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        return Err(pill_core::PillError::from(format!(
                            "No preprocessed .rtex found for {:?}; run `pill_launcher -a assets`",
                            base
                        )));
                    }
                }
            }
            ResourceLoader::Bytes(bytes) => {
                if bytes.starts_with(b"RTEX") {
                    decode_rtex(bytes)?
                } else {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        decode_png(bytes)?
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        return Err(pill_core::PillError::from("Texture::from_bytes on wasm requires pre-converted RTEX format"));
                    }
                }
            }
        };

        // Create renderer texture resource
        let renderer_resource_handle = engine
            .renderer
            .create_texture(&self.name, &rgba, width, height, self.texture_type)
            .context(error_message)?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_texture(v).unwrap();
        }

        // Take resource storage from engine
        let resource_storage = engine
            .resource_manager
            .get_resource_storage_mut::<Material>()?;
        let materials = &mut resource_storage.data;

        // Find materials that use this texture and update them
        for material_slot in materials.iter_mut() {
            let material: &mut Material = material_slot
                .1
                .as_mut()
                .expect("Critical: Resource is None");

            // Update texture slots
            let before = material.textures.len();
            material
                .textures
                .retain(|(_, slot)| slot.texture_handle.data() != self_handle.data());

            if material.textures.len() < before {
                engine
                    .renderer
                    .update_material_textures(
                        material.renderer_resource_handle.unwrap(),
                        &material.textures,
                    )
                    .unwrap();
            }
        }

        Ok(())
    }
}
