use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use crate::ecs::*; 
use crate::engine::Engine;
use crate::graphics::*;
use crate::resources::*;

//use crate::resources::resource_mapxxx::Resource;

use anyhow::{Result, Context, Error};
use pill_core::PillSlotMapKey;
use typemap_rev::TypeMapKey;

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
    pub load_type: ResourceLoadType,
    #[readonly]
    pub texture_type: TextureType,
    pub(crate) renderer_resource_handle: Option<RendererTextureHandle>,
}

impl Texture {

    pub fn new(name: &str, texture_type: TextureType, resource_load_type: ResourceLoadType) -> Self {   
        Self {
            name: name.to_string(),
            load_type: resource_load_type,
            texture_type,
            renderer_resource_handle: None,
        }
    }
}

impl Resource for Texture { // [TODO] What if renderer fails to create texture?
    type Handle = TextureHandle;

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        let renderer_resource_handle = match &self.load_type {
            ResourceLoadType::Path(path) => {
                // Check if path to asset is correct
                pill_core::validate_asset_path(path, "png")?;

                engine.renderer.create_texture(&path, &self.name, self.texture_type).unwrap()
            },
            ResourceLoadType::Bytes(bytes) => engine.renderer.create_texture_from_bytes(&bytes, &self.name, self.texture_type).unwrap(),
        };

        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) {
        
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_texture(v).unwrap();
        }

        // Get default texture of same type
        let default_texture = engine.resource_manager.get_default_texture(self.texture_type).expect("Critical: No default Resource").clone();

        // Take resource storage from engine
        let resource_storage = engine.resource_manager.get_resource_storage_mut::<Material>().expect("Critical: Resource not registered");
        let materials = &mut resource_storage.data;

        // Find materials that use this texture and update them
        for material_entry in materials.iter_mut() {
            
            let material = material_entry.1.as_mut().expect("Critical: Resource is None");
            // Find texture to update
            for texture_slot in material.get_textures().data.iter_mut() {
                if let Some(texture_slot_data) = texture_slot.1.get_texture_data_mut() {
                    if texture_slot_data.0.data() == self_handle.data() {
                        texture_slot_data.0 = default_texture.0.clone();
                        texture_slot_data.1 = default_texture.1.clone();
                        engine.renderer.update_material_textures(material.renderer_resource_handle.unwrap(), &material.textures).unwrap();
                        break;
                    }
                }
            }
        }
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }
}

impl TypeMapKey for Texture {
    type Value = ResourceStorage<Texture>; 
}
