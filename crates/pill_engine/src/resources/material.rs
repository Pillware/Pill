use std::path::Path;
use std::path::PathBuf;

use crate::ecs::*; 
use crate::internal::Engine;
use crate::graphics::*;
use crate::resources::*;

use crate::resources::resource_map::Resource;

//use super::resource_manager::ResourceHandle;
use anyhow::{Result, Context, Error};
use pill_core::na::SliceRange;

pub struct Material {
    pub name: String,
    //shader: ShaderHandle,
    pub color_texture_handle: TextureHandle,
    pub normal_texture_handle: TextureHandle,
    pub order: u32,
    pub(crate) renderer_resource_handle: RendererMaterialHandle,
}

impl Material {
    pub fn new(resource_manager: &mut ResourceManager, renderer: &mut Renderer, name: &str) -> Result<Self> {  // [TODO] What if renderer fails to create material?
        
        let color_texture_handle = resource_manager.get_default_texture(TextureType::Color);
        let normal_texture_handle = resource_manager.get_default_texture(TextureType::Normal);

        let renderer_color_texture_handle = resource_manager.get_resource::<TextureHandle, Texture>(&color_texture_handle).unwrap().renderer_resource_handle;
        let renderer_normal_texture_handle = resource_manager.get_resource::<TextureHandle, Texture>(&normal_texture_handle).unwrap().renderer_resource_handle;

        let renderer_resource_handle = renderer.create_material(
            name,
            renderer_color_texture_handle, 
            renderer_normal_texture_handle
        ).unwrap();

        let material = Self { 
            name: name.to_string(),
            //shader: 0,
            color_texture_handle,
            normal_texture_handle,
            order: 0, 
            renderer_resource_handle,
        };

        Ok(material)
    }

    pub fn assign_color_texture(&mut self, engine: &mut Engine, texture_handle: TextureHandle, texture_type: TextureType) {
        let texture = engine.resource_manager.get_resource::<TextureHandle, Texture>(&texture_handle).unwrap();
        let renderer_texture_handle = texture.renderer_resource_handle;

        engine.renderer.update_material_texture(self.renderer_resource_handle, renderer_texture_handle, TextureType::Color);

        match texture_type {
            TextureType::Color => {
                self.color_texture_handle = texture_handle;
            },
            TextureType::Normal => {
                self.normal_texture_handle = texture_handle;
            }
        }
        // [TODO] Revert material if failed to update in renderer ??
    }

    pub fn set_order(&mut self, engine: &mut Engine, order: u32) {
        // if order >= 2^order_mask_range.end { } // [TODO] Implement failing in not in range
        self.order = order;

        //engine.renderer.update_material(self.renderer_resource_index, self);
    }

}



impl Resource for Material {
    type Storage = ResourceStorage<MaterialHandle, Material>; 
}


