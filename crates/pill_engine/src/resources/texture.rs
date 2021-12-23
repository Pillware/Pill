use std::borrow::BorrowMut;
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

        // Take resource storage from engine
        let mut resource_storage = engine.resource_manager.get_resource_storage_mut::<Material>().unwrap().take();
        let materials = &mut resource_storage.as_mut().unwrap().data;

        // Find materials that use this texture and update them
        for material_entry in materials.iter_mut() {
            
            // Find texture to update
            let mut texture_to_change_data: Option<(String, TextureType)> = Option::None;
            for material_texture in material_entry.1.textures.data.iter() {
                if let Some(texture_data) = material_texture.1.get_texture_data() {
                    if texture_data.0.data() == self_handle.data() {
                        texture_to_change_data = Some((material_texture.0.to_string(), material_texture.1.get_type()));
                        break;
                    }
                }
            }

            // Update found texture if any
            if let Some(data) = texture_to_change_data {
                let material = material_entry.1;
                let default_texture = engine.resource_manager.get_default_texture(data.1).unwrap();
                material.set_texture(engine, &data.0, default_texture.0).unwrap();
            }
        }
    
        // Take resource storage to engine
        *engine.resource_manager.get_resource_storage_mut::<Material>().unwrap() = resource_storage;
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }
}

impl TypeMapKey for Texture {
    type Value = Option<ResourceStorage<Texture>>; 
}
