use std::path::Path;

use crate::ecs::*; 
use crate::graphics::*;
use crate::resources::*;

pub struct Texture {
    buffer_index: usize,
}

impl Texture {
    pub fn new(renderer: &mut Renderer, path: Box<&Path>) -> Self {  // [TODO] What if renderer fails to create texture?
        let buffer_index = renderer.create_texture(path).unwrap();
        return Self { 
            buffer_index: buffer_index,
        };
    }
}


impl Component for Texture { // [TODO] Change component to Resource, type aliasing for traits are not possible
    type Storage = ResourceStorage<Texture>; 
}