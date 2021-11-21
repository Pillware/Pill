use std::path::Path;
use std::path::PathBuf;

use crate::ecs::*; 
use crate::internal::Engine;
use crate::graphics::*;
use crate::resources::*;

use crate::resources::resource_map::Resource;

use super::resource_manager::ResourceHandle;
use anyhow::{Result, Context, Error};

pub enum TextureType{
    Color,
    Normal,
}

#[derive(Clone, Copy)]
pub struct TextureHandle {
    pub index: u32,
}

impl ResourceHandle for TextureHandle
{
    fn get_index(&self) -> u32 {
        self.index
    }
}

pub struct Texture {
    name: String,
    path: PathBuf,
    renderer_resource_index: u32,
}

impl Texture {
    pub fn new(renderer: &mut Renderer, name: &str, path: PathBuf) -> Result<Self> {  // [TODO] What if renderer fails to create texture?
        let renderer_resource_index = renderer.create_texture(&path).unwrap();
        let texture = Self { 
            name: name.to_string(),
            path: path,
            renderer_resource_index,
        };
        Ok(texture)
    }
}

impl Resource for Texture {
    type Storage = ResourceStorage<Texture>; 
}


