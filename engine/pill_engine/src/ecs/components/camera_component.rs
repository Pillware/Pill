#![cfg(feature = "rendering")]

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

#[repr(C)]
#[derive(Debug, Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PostprocessParams {
    pub vignette_strength: f32,
    pub vignette_extent: f32,
    pub screen_resolution: [f32; 2],
    pub tilt_shift_focus_area: f32,
    pub tilt_shift_focus_pos: f32,
    pub tilt_shift_blur_amount: f32,
    pub abberration_strength: f32, // Strength of the aberration effect
 //      pub _padding: f32,               // 4 bytes
}

impl Default for PostprocessParams {
    fn default() -> Self {
        Self {
            vignette_strength: 0.8,
            vignette_extent: 0.5,
            screen_resolution: [1920.0, 1080.0],
            tilt_shift_focus_area: 0.3,
            tilt_shift_focus_pos: 0.5,
            tilt_shift_blur_amount: 0.2,
            abberration_strength: 0.0,
          //  _padding: 0.0, // Padding to ensure 32-byte alignment
        }
    }
}

pub struct CameraComponent {
    pub aspect: CameraAspectRatio,
    pub fov: f32,
    pub range: Range<f32>,
    pub clear_color: Color,
    pub enabled: bool,
    pub(crate) renderer_resource_handle: Option<RendererCameraHandle>,
    pub postprocess_params: PostprocessParams,
    pub target_aberrration: f32, // Target aberration value for post-processing effects
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
            postprocess_params: PostprocessParams::default(),
            target_aberrration: 0.0, // Default value for target aberration
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
        let renderer_resource_handle = engine.renderer.as_mut().unwrap().create_camera().context(error_message)?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn destroy(&mut self, engine: &mut Engine, self_scene_handle: SceneHandle, self_entity_handle: EntityHandle) -> Result<()> {
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.as_mut().unwrap().destroy_camera(v).unwrap();
        }

        Ok(())
    }
}

