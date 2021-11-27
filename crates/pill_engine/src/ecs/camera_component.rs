use std::ops::Range;

pub use crate::ecs::*;
use crate::{
    game::Engine, 
    graphics::{RenderQueueKey, compose_render_queue_key}, 
    resources::{Material, MaterialHandle, Mesh, MeshHandle}
};
use anyhow::{Result, Context, Error};

pub struct CameraComponent {
    pub aspect: f32,
    pub fov: f32,
    pub range: Range<u32>,
}

impl Component for CameraComponent {
    type Storage = ComponentStorage<CameraComponent>; 
}

impl CameraComponent {
    pub fn new(engine: &Engine) -> Result<Self> {
        let camera_component = Self { 
            aspect: 1.77, // width as f32 / height as f32
            fov: 45.0,
            range: 1..100,
        };
    
        Ok(camera_component)
    }
}
