use std::path::Path;
use std::path::PathBuf;

use crate::ecs::*; 
use crate::internal::Engine;
use crate::graphics::*;
use crate::resources::*;

use crate::resources::resource_map::Resource;

use super::resource_manager::ResourceHandle;
use anyhow::{Result, Context, Error};
use pill_core::na::SliceRange;

#[derive(Clone, Copy)]
pub struct MaterialHandle {
    pub index: u32,
}

impl ResourceHandle for MaterialHandle
{
    fn get_index(&self) -> u32 {
        self.index
    }
}

pub struct Material {
    pub name: String,
    //shader: ShaderHandle,
    pub color_texture: TextureHandle,
    pub normal_texture: TextureHandle,
    pub order: u32,
    renderer_resource_index: u32,
}

impl Material {
    pub fn new(resource_manager: &mut ResourceManager, renderer: &mut Renderer, name: &str) -> Result<Self> {  // [TODO] What if renderer fails to create material?
        
        let color_texture = resource_manager.get_default_texture(TextureType::Color);
        let normal_texture = resource_manager.get_default_texture(TextureType::Normal);
        let renderer_resource_index = renderer.create_material(color_texture, normal_texture).unwrap();

        let material = Self { 
            name: name.to_string(),
            //shader: 0,
            color_texture,
            normal_texture,
            order: 0, 
            renderer_resource_index,
        };

        Ok(material)
    }

    pub fn assign_color_texture(&mut self, engine: &mut Engine, texture_handle: TextureHandle) {
        self.color_texture = texture_handle;
        engine.renderer.update_material(self.renderer_resource_index, self);
        // [TODO] Revert material if failed to update in renderer
    }

    pub fn assign_normal_texture(&mut self, engine: &mut Engine, texture_handle: TextureHandle) {
        self.normal_texture = texture_handle;
        engine.renderer.update_material(self.renderer_resource_index, self);
        // [TODO] Revert material if failed to update in renderer
    }

    pub fn set_order(&mut self, engine: &mut Engine, order: u32) {
        // if order >= 2^order_mask_range.end { } // [TODO] Implement failing in not in range
        self.order = order;

        //engine.renderer.update_material(self.renderer_resource_index, self);
    }

}



impl Resource for Material {
    type Storage = ResourceStorage<Material>; 
}


