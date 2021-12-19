use std::path::Path;
use std::path::PathBuf;

use crate::ecs::*; 
use crate::engine::Engine;
use crate::graphics::*;
use crate::resources::*;

//use crate::resources::resource_mapxxx::Resource;

use anyhow::{Result, Context, Error};
use typemap_rev::TypeMapKey;


#[derive(Clone, Copy)]
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
    fn initialize(&mut self, engine: &mut Engine) {
        let renderer_resource_handle = match &self.load_type {
            ResourceLoadType::Path(path) => engine.renderer.create_texture(&path, &self.name, self.texture_type).unwrap(),
            ResourceLoadType::Bytes(bytes) => engine.renderer.create_texture_from_bytes(&bytes, &self.name, self.texture_type).unwrap(),
        };

        self.renderer_resource_handle = Some(renderer_resource_handle);
    }

    fn get_name(&self) -> String {
        self.name.clone()
    }

    fn destroy(&mut self, engine: &mut Engine) {
        //todo!()
    }
}

impl TypeMapKey for Texture {
    type Value = ResourceStorage<TextureHandle, Texture>; 
}
