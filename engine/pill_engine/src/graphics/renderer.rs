use crate::{
    ecs::{
        CameraComponent, ComponentStorage, EntityHandle, TransformComponent
    },  graphics::RenderQueueItem, internal::{MaterialParameter, MaterialTexture, MeshData}, resources::{
        ShaderParameterSlot,
        ShaderTextureSlot,
        TextureType,
    }
};

use indexmap::IndexMap;
use pill_core::Timer;

use std::{collections::HashMap, sync::Arc};
use anyhow::Result;


// --- Renderer resource handles ---

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererMaterialHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererMeshHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererCameraHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererTextureHandle;
}

pill_core::define_new_pill_slotmap_key! {
    pub struct RendererShaderHandle;
}

// --- Renderer trait definition ---

pub trait PillRenderer {
    fn new(window: Arc<winit::window::Window>, config: config::Config) -> Result<Self> where Self: Sized;

    // --- Create ---

    fn create_shader(
        &mut self,
        name: &str,
        vertex_shader_bytes: &[u8],
        fragment_shader_bytes: &[u8],
        texture_slots: &HashMap<String, ShaderTextureSlot>,
        parameter_slots: &HashMap<String, ShaderParameterSlot>,
        pass_engine_parameters: bool,
        pass_camera_parameters: bool,
   ) -> Result<RendererShaderHandle>;

    fn create_material(
        &mut self,
        name: &str,
        renderer_shader_handle: RendererShaderHandle,
        textures: &IndexMap<String, MaterialTexture>,
        parameters: &HashMap<String, MaterialParameter>
    ) -> Result<RendererMaterialHandle>;

    fn create_texture(&mut self, name: &str, image_data: &image::DynamicImage, texture_type: TextureType) -> Result<RendererTextureHandle>;

    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle>;

    fn create_camera(&mut self) -> Result<RendererCameraHandle>;

    // --- Update ---

    fn update_material_textures(&mut self, renderer_material_handle: RendererMaterialHandle, textures: &IndexMap<String, MaterialTexture>) -> Result<()>;

    fn update_material_parameters(&mut self, renderer_material_handle: RendererMaterialHandle, parameters: &HashMap<String, MaterialParameter>) -> Result<()>;

    // --- Destroy ---

    fn destroy_shader(&mut self, renderer_shader_handle: RendererShaderHandle) -> Result<()>;

    fn destroy_material(&mut self, renderer_material_handle: RendererMaterialHandle) -> Result<()>;

    fn destroy_texture(&mut self, renderer_texture_handle: RendererTextureHandle) -> Result<()>;

    fn destroy_mesh(&mut self, renderer_mesh_handle: RendererMeshHandle) -> Result<()>;

    fn destroy_camera(&mut self, renderer_camera_handle: RendererCameraHandle) -> Result<()>;

    // --- Other ---

    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>);

    fn pass_input_to_egui(&mut self, event: &winit::event::WindowEvent) -> Result<()>;

    fn render(&mut self,
        active_camera_entity_handle: EntityHandle,
        render_queue: &[RenderQueueItem],
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>,
        egui_ui:  Box<dyn FnMut(&egui::Context)>,
        delta_time: f32,
        timer: &mut Timer
    ) -> Result<()>;

}

pub type Renderer = Box<dyn PillRenderer>;





