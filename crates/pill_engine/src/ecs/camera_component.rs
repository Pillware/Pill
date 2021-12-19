use std::ops::Range;

pub use crate::ecs::*;
use crate::{
    game::Engine, 
    graphics::{RenderQueueKey, compose_render_queue_key}, 
    resources::{Material, MaterialHandle, Mesh, MeshHandle, RendererCameraHandle}
};
use anyhow::{Result, Context, Error};

pub struct CameraComponent {
    pub aspect: f32,
    pub fov: f32,
    pub range: Range<u32>,
    pub(crate) renderer_resource_handle: RendererCameraHandle,
    pub enabled: bool,
}

impl Component for CameraComponent {
    type Storage = ComponentStorage<CameraComponent>; 
}

impl CameraComponent {
    pub fn new(engine: &mut Engine) -> Result<Self> {
        let renderer_resource_handle = engine.renderer.create_camera().unwrap();//?;

        let camera_component = Self { 
            aspect: 1.0, // width as f32 / height as f32
            fov: 60.0,
            range: 1..100,
            renderer_resource_handle,
            enabled: false,
        };
    
        Ok(camera_component)
    }

    pub fn get_renderer_resource_handle(&self) -> RendererCameraHandle {
        self.renderer_resource_handle
    }

    // pub fn set_active(engine: &mut Engine) -> Result<()> {
    //     let active_scene_handle = engine.get_active_scene().unwrap();
    //     let active_scene = engine.scene_manager.get_scene(active_scene_handle)?;
    //     engine.get_active_scene
    // }
}

