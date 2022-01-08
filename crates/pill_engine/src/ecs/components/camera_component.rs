use crate::{
    engine::Engine, 
    graphics::{ RenderQueueKey, compose_render_queue_key, RendererCameraHandle }, 
    resources::{ Material, MaterialHandle, Mesh, MeshHandle },
    ecs::{ Component, ComponentStorage, EntityHandle, SceneHandle, DeferredUpdateManagerPointer, DeferredUpdateComponentRequest },
};

use pill_core::{ PillSlotMapKey, Color, PillStyle, get_type_name };

use anyhow::{Result, Context, Error};
use pill_core::{ PillTypeMap, PillTypeMapKey };
use std::ops::Range;


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

// --- Builder ---

pub struct CameraComponentBuilder {
    component: CameraComponent,
}

impl CameraComponentBuilder {
    pub fn default() -> Self {
        Self {
            component: CameraComponent::new(),
        }
    }
    
    pub fn aspect(mut self, aspect: CameraAspectRatio) -> Self {
        self.component.aspect = aspect;
        self
    }

    pub fn fov(mut self, fov: f32) -> Self {
        self.component.fov = fov;
        self
    }

    pub fn range(mut self, range: Range<f32>) -> Self {
        self.component.range = range;
        self
    }

    pub fn clear_color(mut self, clear_color: Color) -> Self {
        self.component.clear_color = clear_color;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.component.enabled = enabled;
        self
    }

    pub fn build(self) -> CameraComponent {
        self.component
    }
}

// --- Camera Component ---

pub struct CameraComponent {
    pub aspect: CameraAspectRatio,
    pub fov: f32,
    pub range: Range<f32>,
    pub clear_color: Color,
    pub enabled: bool,
    pub(crate) renderer_resource_handle: Option<RendererCameraHandle>,
}

impl CameraComponent {
    pub fn builder() -> CameraComponentBuilder {
        CameraComponentBuilder::default()
    }

    pub fn new() -> Self {
        Self { 
            aspect: CameraAspectRatio::Automatic(1.0),
            fov: 60.0,
            range: 0.1..100.0,
            clear_color: Color::new(0.15, 0.15, 0.15),
            renderer_resource_handle: None,
            enabled: false,
        }
    }
}

// This needed so that renderer can get renderer camera handle from camera component while it is still hidden in game API
pub fn get_renderer_resource_handle_from_camera_component(camera_component: &CameraComponent) -> RendererCameraHandle {
    camera_component.renderer_resource_handle.expect("Critical: No renderer resource handle")
}

impl PillTypeMapKey for CameraComponent {
    type Storage = ComponentStorage<CameraComponent>; 
}

impl Component for CameraComponent {
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        let error_message = format!("Initializing {} {} failed", "Component".gobj_style(), get_type_name::<Self>().sobj_style());

        // Create new renderer camera resource
        let renderer_resource_handle = engine.renderer.create_camera().context(error_message)?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn destroy(&mut self, engine: &mut Engine, self_entity_handle: EntityHandle, self_scene_handle: SceneHandle) -> Result<()> {
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_camera(v).unwrap();
        }

        Ok(())
    }
}

