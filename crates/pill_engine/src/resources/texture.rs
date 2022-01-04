use crate::{
    engine::Engine,
    graphics::{ RendererTextureHandle }, 
    resources::{ ResourceStorage, Resource, ResourceLoadType, Material },
    ecs::{ DeferredUpdateManagerPointer },
    config::*,
};

use pill_core::{ PillSlotMapKey, PillTypeMapKey, PillStyle, get_type_name };

use std::collections::HashSet;
use std::path::{ Path, PathBuf };
use anyhow::{ Result, Context, Error };

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

impl PillTypeMapKey for Texture {
    type Storage = ResourceStorage<Texture>; 
}

impl Resource for Texture { // [TODO] What if renderer fails to create texture?
    type Handle = TextureHandle;

    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        let error_message = format!("Initializing {} {} failed", "Resource".gobj_style(), get_type_name::<Self>().sobj_style());    

        // Create new renderer texture resource
        let renderer_resource_handle = match &self.load_type {
            ResourceLoadType::Path(path) => {
                // Check if path to asset is correct
                pill_core::validate_asset_path(path, "png")?;
                // Create renderer texture resource
                engine.renderer.create_texture(&path, &self.name, self.texture_type).context(error_message.clone())?
            },
            ResourceLoadType::Bytes(bytes) => {
                // Create renderer texture resource
                engine.renderer.create_texture_from_bytes(&bytes, &self.name, self.texture_type).context(error_message.clone())?
            },
        };
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> {
        
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_texture(v).unwrap();
        }

        // Take resource storage from engine
        let resource_storage = engine.resource_manager.get_resource_storage_mut::<Material>().expect("Critical: Resource not registered");
        let materials = &mut resource_storage.data;

        // Find materials that use this texture and update them
        for material_slot in materials.iter_mut() {
            let material = material_slot.1.as_mut().expect("Critical: Resource is None");

            // Update texture slots
            let mut material_updated = false;
            for texture_slot in material.get_textures().data.iter_mut() {
                if let Some(texture_handle) = texture_slot.1.texture_handle {
                    // If material texture has handle to this texture  
                    if texture_handle.data() == self_handle.data() {
                        texture_slot.1.texture_handle = None;
                        texture_slot.1.renderer_texture_handle = None;
                        material_updated = true;
                    }
                }
            }

            if material_updated {
                engine.renderer.update_material_textures(material.renderer_resource_handle.unwrap(), &material.textures).unwrap();
            }
        }

        Ok(())
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }
}