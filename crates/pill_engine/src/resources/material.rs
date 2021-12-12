use std::path::Path;
use std::path::PathBuf;

use crate::ecs::*; 
use crate::internal::Engine;
use crate::graphics::*;
use crate::resources::*;

//use crate::resources::resource_mapxxx::Resource;

//use super::resource_manager::ResourceHandle;
use anyhow::{Result, Context, Error};
use pill_core::na::SliceRange;

#[readonly::make]
pub struct Material {
    #[readonly]
    pub name: String,
    #[readonly]
    pub color_texture_handle: Option<TextureHandle>,
    #[readonly]
    pub normal_texture_handle: Option<TextureHandle>,
    #[readonly]
    pub order: u32,
    pub(crate) renderer_resource_handle: Option<RendererMaterialHandle>,
}

impl Material {

    pub fn new(name: &str) -> Self {  // [TODO] What if renderer fails to create material?
        Self {
            name: name.to_string(),
            color_texture_handle: None,
            normal_texture_handle: None,
            order: 0,
            renderer_resource_handle: None,
        }
    }

    pub fn assign_texture(&mut self, engine: &mut Engine, texture_handle: TextureHandle, texture_type: TextureType) {
        let texture = engine.resource_manager.get_resource::<TextureHandle, Texture>(&texture_handle).unwrap();
        let renderer_texture_handle = texture.renderer_resource_handle.unwrap();

        engine.renderer.update_material_texture(self.renderer_resource_handle.unwrap(), renderer_texture_handle, texture_type).unwrap();

        match texture_type {
            TextureType::Color => {
                self.color_texture_handle = Some(texture_handle);
            },
            TextureType::Normal => {
                self.normal_texture_handle = Some(texture_handle);
            }
        }
        // [TODO] Revert material if failed to update in renderer ??
    }

    pub fn set_order(&mut self, _engine: &mut Engine, order: u32) {
        // if order >= 2^order_mask_range.end { } // [TODO] Implement failing in not in range
        self.order = order;

        //engine.renderer.update_material(self.renderer_resource_index, self);
    }

}

impl Resource for Material {
    fn initialize(&mut self, engine: &mut Engine) {

        // Assign default color texture if non texture is assigned
        if self.color_texture_handle.is_none() {
            self.color_texture_handle = Some(engine.resource_manager.get_default_texture_handle(TextureType::Color));
        }

        // Assign default normal texture if non texture is assigned
        if self.normal_texture_handle.is_none() {
            self.normal_texture_handle = Some(engine.resource_manager.get_default_texture_handle(TextureType::Normal));
        }

        // Get renderer texture resource handles
        let color_texture = engine.resource_manager.get_resource::<TextureHandle, Texture>(&self.color_texture_handle.as_ref().unwrap()).unwrap();
        let normal_texture = engine.resource_manager.get_resource::<TextureHandle, Texture>(&self.normal_texture_handle.as_ref().unwrap()).unwrap();

        // Create new renderer material resource
        let renderer_resource_handle = engine.renderer.create_material(
            &self.name,
            color_texture.renderer_resource_handle.unwrap(), 
            normal_texture.renderer_resource_handle.unwrap()
        ).unwrap();

        self.renderer_resource_handle = Some(renderer_resource_handle);
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }

    fn destroy(&mut self, engine: &mut Engine) {
        //todo!()
    }
}

impl typemap_rev::TypeMapKey for Material {
    type Value = ResourceStorage<MaterialHandle, Material>; 
}


// impl Resource for Material {
//     type Storage = ResourceStorage<MaterialHandle, Material>; 
// }


