use crate::{
    ecs::{Component, ComponentStorage, EntityHandle, SceneHandle},
    engine::Engine,
    graphics::RendererCameraHandle,
};

use pill_core::{get_type_name, PillStyle, Vector3f};

use anyhow::{Context, Result};
use pill_core::PillTypeMapKey;
use std::ops::Range;

pub enum CameraAspectRatio {
    Automatic(f32),
    Manual(f32),
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

    pub fn clear_color(mut self, clear_color: Vector3f) -> Self {
        self.component.clear_color = clear_color;
        self
    }

    pub fn fog_density(mut self, density: f32) -> Self {
        self.component.fog_density = density;
        self
    }

    pub fn fog_color(mut self, color: Vector3f) -> Self {
        self.component.fog_color = color;
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
    pub clear_color: Vector3f,
    // Exponential-squared fog: final_color is mixed toward fog_color by
    // 1 - exp(-density² · distance²). density = 0.0 disables fog (default).
    pub fog_density: f32,
    pub fog_color: Vector3f,
    pub enabled: bool,
    pub(crate) renderer_resource_handle: Option<RendererCameraHandle>,
}

impl Default for CameraComponent {
    fn default() -> Self {
        Self::new()
    }
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
            clear_color: Vector3f::new(0.15, 0.15, 0.15),
            fog_density: 0.0,
            fog_color: Vector3f::new(0.0, 0.0, 0.0),
            renderer_resource_handle: None,
            enabled: false,
        }
    }
}

// This needed so that renderer can get renderer camera handle from camera component while it is still hidden in game API
pub fn get_renderer_resource_handle_from_camera_component(
    camera_component: &CameraComponent,
) -> RendererCameraHandle {
    camera_component
        .renderer_resource_handle
        .expect("Critical: No renderer resource handle")
}

impl PillTypeMapKey for CameraComponent {
    type Storage = ComponentStorage<CameraComponent>;
}

impl Component for CameraComponent {
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        let error_message = format!(
            "Initializing {} {} failed",
            "Component".general_object_style(),
            get_type_name::<Self>().specific_object_style()
        );

        // Create new renderer camera resource
        let renderer_resource_handle = engine.renderer.create_camera().context(error_message)?;
        self.renderer_resource_handle = Some(renderer_resource_handle);

        Ok(())
    }

    fn destroy(
        &mut self,
        engine: &mut Engine,
        _self_scene_handle: SceneHandle,
        _self_entity_handle: EntityHandle,
    ) -> Result<()> {
        // Destroy renderer resource
        if let Some(v) = self.renderer_resource_handle {
            engine.renderer.destroy_camera(v).unwrap();
        }

        Ok(())
    }
}
