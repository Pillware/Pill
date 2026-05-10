use crate::{
    engine::Engine,
    renderer::resources::{RendererMaterial, RendererTexture},
    resources::{Material, Resource, ResourceLoader, ResourceStorage},
};
use std::collections::HashMap;

use pill_core::{get_type_name, ErrorContext, PillSlotMapKey, PillStyle, PillTypeMapKey, Result};
pill_core::define_new_pill_slotmap_key! {
    pub struct TextureHandle;
}

#[derive(Clone, Copy, Debug)]
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
        Self::new(
            name,
            texture_type,
            ResourceLoader::Bytes(bytes.to_vec().into_boxed_slice()),
        )
    }
}

fn decode_cooked_tex(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    if bytes.len() < 16 || &bytes[0..4] != b"RTEX" {
        return Err(pill_core::PillError::from(
            "not a valid .cooked_tex file (bad magic or truncated header)",
        ));
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
                let cooked_tex_path = base.with_extension("cooked_tex");
                if cooked_tex_path.exists() {
                    let bytes =
                        std::fs::read(&cooked_tex_path).map_err(|e| -> pill_core::PillError {
                            format!("Failed to read texture {cooked_tex_path:?}: {e}").into()
                        })?;
                    decode_cooked_tex(&bytes)?
                } else {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        // Check if path to asset is correct
                        pill_core::validate_asset_path(&base, &["png", "cooked_tex"])?;
                        let bytes = std::fs::read(&base).map_err(|e| -> pill_core::PillError {
                            format!("Failed to read texture {base:?}: {e}").into()
                        })?;
                        if base.extension().map(|e| e == "cooked_tex").unwrap_or(false) {
                            decode_cooked_tex(&bytes)?
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

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {
        #[cfg(not(feature = "headless"))]
        engine
            .resource_manager
            .remove_resource_by_name::<RendererTexture>(&self.name)?;

        // Phase 1: find and update affected materials (CPU side only)
        let affected_material_names: Vec<String> = {
            let resource_storage = engine
                .resource_manager
                .get_resource_storage_mut::<Material>()?;
            let mut affected = Vec::new();
            for (_, slot) in resource_storage.data.iter_mut() {
                let material = slot.as_mut().expect("Critical: Resource is None");
                let before = material.textures.len();
                material
                    .textures
                    .retain(|(_, slot)| slot.texture_handle.data() != self_handle.data());
                if material.textures.len() < before {
                    affected.push(material.name.clone());
                }
            }
            affected
        };

        // Phase 2: rebuild RendererMaterial textures bind group for affected materials
        #[cfg(not(feature = "headless"))]
        for mat_name in &affected_material_names {
            let new_bind_group = {
                let renderer_mat = engine
                    .resource_manager
                    .get_resource_by_name::<RendererMaterial>(mat_name)?;
                let renderer_shader = engine
                    .resource_manager
                    .get_resource::<crate::renderer::resources::RendererShader>(
                    &renderer_mat.shader_handle,
                )?;
                let material = engine
                    .resource_manager
                    .get_resource_by_name::<Material>(mat_name)?;
                let mut resolved: HashMap<String, crate::graphics::RendererTextureHandle> =
                    HashMap::new();
                for (slot_name, mat_tex) in &material.textures {
                    let tex = engine
                        .resource_manager
                        .get_resource::<crate::resources::Texture>(&mat_tex.texture_handle)?;
                    let h = engine
                        .resource_manager
                        .get_resource_handle::<RendererTexture>(&tex.name)?;
                    resolved.insert(slot_name.clone(), h);
                }
                RendererMaterial::create_textures_bind_group(
                    engine.renderer.get_device(),
                    &engine.resource_manager,
                    renderer_shader.textures_bind_group_layout.as_ref().unwrap(),
                    &format!("{}_textures", mat_name),
                    &renderer_shader.texture_slots,
                    &resolved,
                )?
            };
            engine
                .resource_manager
                .get_resource_by_name_mut::<RendererMaterial>(mat_name)?
                .textures_bind_group = Some(new_bind_group);
        }

        Ok(())
    }
}
