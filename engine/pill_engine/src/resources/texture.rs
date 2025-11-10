use crate::{
    config::*,
    ecs::DeferredUpdateManagerPointer,
    engine::Engine,
    graphics::RendererTextureHandle,
    resources::{PBRMaterial, Resource, ResourceLoadType, ResourceStorage},
};

use pill_core::{get_type_name, PillSlotMapKey, PillStyle, PillTypeMapKey};

use anyhow::{Context, Error, Result};
use bytemuck::{Pod, Zeroable}; // For derive macros to work
use readonly::make;
use std::collections::HashSet;
use std::path::{Path, PathBuf}; // For #[readonly::make] to resolve
pill_core::define_new_pill_slotmap_key! {
    pub struct TextureHandle;
}

#[derive(Clone, Copy, Debug)]
pub enum TextureType {
    Gamma, // sRGB
    Linear,
}

#[readonly::make]
pub struct Texture {
    #[readonly]
    pub name: String,
    #[readonly]
    pub load_type: ResourceLoadType,
    #[readonly]
    pub texture_type: TextureType,
    pub(crate) renderer_resource_handle: Option<RendererTextureHandle>,
}

impl Texture {
    pub fn new(
        name: &str,
        texture_type: TextureType,
        resource_load_type: ResourceLoadType,
    ) -> Self {
        Self {
            name: name.to_string(),
            load_type: resource_load_type,
            texture_type,
            renderer_resource_handle: None,
        }
    }
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
            "Resource".gobj_style(),
            get_type_name::<Self>().sobj_style()
        );

        // Create new renderer texture resource
        let image_data = match &self.load_type {
            ResourceLoadType::Path(path) => {
                // Check if path to asset is correct
                let resource_file_path = engine.game_resources_directory_path.join(path);
                pill_core::validate_asset_path(&resource_file_path, &["png", "jpg", "gif", "tif"])?;

                // Load data
                image::open(&resource_file_path)?
            }
            ResourceLoadType::Bytes(bytes) => {
                // Load data
                image::load_from_memory(bytes)?
            }
        };

        // Create renderer texture resource
        let renderer_resource_handle = engine
            .renderer
            .create_texture(&self.name, &image_data, self.texture_type)
            .context(error_message.clone())?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_texture(v).unwrap();
        }

        Ok(())
    }
}
