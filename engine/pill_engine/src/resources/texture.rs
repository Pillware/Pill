use crate::{
    engine::Engine,
    graphics::RendererTextureHandle,
    resources::{Material, Resource, ResourceLoader, ResourceStorage},
};

use pill_core::{get_type_name, PillSlotMapKey, PillStyle, PillTypeMapKey};

use anyhow::{Context, Result};
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
        let image_data = match &self.resource_loader {
            ResourceLoader::Path(path) => {
                // Check if path to asset is correct
                let resource_file_path = engine.game_resources_directory_path.join(path);
                pill_core::validate_asset_path(&resource_file_path, &["png", "jpg", "gif", "tif"])?;

                // Load data
                image::open(&resource_file_path)?
            }
            ResourceLoader::Bytes(bytes) => {
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
            let mut material_updated = false;
            // Iterate all texture slots in material and remove one with handle to this texture
            // for (texture_slot_name, texture_slot) in material.textures.iter_mut() {
            //     if texture_slot.texture_handle.data() == self_handle.data() { // TODO: Add proper handle comparison method
            //         material.textures.remove(texture_slot_name);
            //         material_updated = true;
            //     }
            // }

            let mut texture_slots_to_remove = Vec::new();
            for (texture_slot_name, texture_slot) in material.textures.iter() {
                if texture_slot.texture_handle.data() == self_handle.data() {
                    texture_slots_to_remove.push(texture_slot_name.clone());
                }
            }
            for texture_slot in texture_slots_to_remove {
                material.textures.remove(&texture_slot);
                material_updated = true;
            }

            // for texture_slot in material.textures.iter_mut() {
            //     if let Some(texture_handle) = texture_slot.1.texture_handle {
            //         // If material texture has handle to this texture
            //         if texture_handle.data() == self_handle.data() {
            //             texture_slot.1.texture_handle = None;
            //             texture_slot.1.renderer_texture_handle = None;
            //             material_updated = true;
            //         }
            //     }
            // }

            if material_updated {
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
