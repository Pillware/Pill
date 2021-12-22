use std::ops::Range;

pub use crate::ecs::*;
use crate::{
    game::Engine, 
    graphics::{RenderQueueKey, compose_render_queue_key, RendererCameraHandle }, 
    resources::{Material, MaterialHandle, Mesh, MeshHandle }
};
use anyhow::{Result, Context, Error};

pub enum CameraAspectRatio {
    Automatic(f32),
    Manual(f32)
}

impl CameraAspectRatio {
    pub fn get_value(&self) -> f32 {
        match self {
            CameraAspectRatio::Automatic(v) => *v,
            CameraAspectRatio::Manual(v) => *v,
        }
    }
}

pub struct CameraComponent {
    pub aspect: CameraAspectRatio,
    pub fov: f32,
    pub range: Range<f32>,
    pub(crate) renderer_resource_handle: RendererCameraHandle,
    pub enabled: bool,
}

impl Component for CameraComponent {
    type Storage = ComponentStorage<CameraComponent>; 
}

impl CameraComponent {
    pub fn new(engine: &mut Engine) -> Result<Self> {
        let renderer_resource_handle = engine.renderer.create_camera().unwrap();

        let camera_component = Self { 
            aspect: CameraAspectRatio::Automatic(1.0),
            fov: 60.0,
            range: 0.1..100.0,
            renderer_resource_handle,
            enabled: false,
        };
    
        Ok(camera_component)
    }

    pub fn get_renderer_resource_handle(&self) -> RendererCameraHandle {
        self.renderer_resource_handle
    }
}

