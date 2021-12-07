use std::path::Path;
use std::path::PathBuf;

use crate::ecs::*; 
use crate::engine::Engine;
use crate::graphics::*;
use crate::resources::*;

use crate::resources::resource_map::Resource;

use anyhow::{Result, Context, Error};

pub enum TextureType{
    Color,
    Normal,
}

pub struct Texture {
    name: String,
    path: PathBuf,
    pub(crate) renderer_resource_handle: RendererTextureHandle,
}

impl Texture {
    pub fn new(renderer: &mut Renderer, name: &str, path: PathBuf, texture_type: TextureType) -> Result<Self> {  // [TODO] What if renderer fails to create texture?
        let renderer_resource_handle = renderer.create_texture(&path, name, texture_type).unwrap();
        let texture = Self { 
            name: name.to_string(),
            path: path,
            renderer_resource_handle,
        };
        
        Ok(texture)
    }

    pub(crate) fn new_from_bytes(renderer: &mut Renderer, bytes: &[u8], name: &str, path: PathBuf, texture_type: TextureType) -> Result<Self> {
        let renderer_resource_handle = renderer.create_texture_from_bytes(bytes, name, texture_type).unwrap();
        let texture = Self { 
            name: name.to_string(),
            path: path,
            renderer_resource_handle,
        };
        
        Ok(texture)
    }
}

impl Resource for Texture {
    type Storage = ResourceStorage<TextureHandle, Texture>; 
}


