use crate::{
    ecs::{CameraComponent, ComponentStorage, EntityHandle, TransformComponent},
    engine::Engine,
    graphics::RenderQueueItem,
    resources::{
        MaterialHandle, MaterialParameterMap, MaterialTextureMap, MeshData, MeshHandle,
        TextureHandle, TextureType,
    },
};

use pill_core::PillStyle;
use pill_core::{PillSlotMapKey, Timer};

use anyhow::{Context, Error, Result};
use std::{path::PathBuf, sync::Arc};
use thiserror::Error;

// --- Renderer resource handles ---

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererMaterialHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererMeshHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererPipelineHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererCameraHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererTextureHandle;
}

// --- Renderer trait definition ---

pub trait PillRenderer {
    fn new(window: Arc<winit::window::Window>, config: config::Config) -> Self
    where
        Self: Sized;

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>);

    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle>;
    fn create_texture(
        &mut self,
        name: &str,
        image_data: &image::DynamicImage,
        texture_type: TextureType,
    ) -> Result<RendererTextureHandle>;
    fn create_material(
        &mut self,
        name: &str,
        textures: &MaterialTextureMap,
        parameters: &MaterialParameterMap,
    ) -> Result<RendererMaterialHandle>;
    fn create_camera(&mut self) -> Result<RendererCameraHandle>;

    fn update_material_textures(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        textures: &MaterialTextureMap,
    ) -> Result<()>;
    fn update_material_parameters(
        &mut self,
        renderer_material_handle: RendererMaterialHandle,
        parameters: &MaterialParameterMap,
    ) -> Result<()>;

    fn destroy_mesh(&mut self, renderer_mesh_handle: RendererMeshHandle) -> Result<()>;
    fn destroy_texture(&mut self, renderer_texture_handle: RendererTextureHandle) -> Result<()>;
    fn destroy_material(&mut self, renderer_material_handle: RendererMaterialHandle) -> Result<()>;
    fn destroy_camera(&mut self, renderer_camera_handle: RendererCameraHandle) -> Result<()>;

    fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) -> Result<()>;

    fn render(
        &mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &Vec<RenderQueueItem>,
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        egui_ui: Box<dyn Fn(&egui::Context)>,
        timer: &mut Timer,
    ) -> Result<()>;
}

pub type Renderer = Box<dyn PillRenderer>;
