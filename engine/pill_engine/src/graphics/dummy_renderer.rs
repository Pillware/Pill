use crate::{
    app_config::EngineConfig,
    ecs::{CameraComponent, ComponentStorage, EntityHandle, TransformComponent},
    graphics::{
        PillRenderer, RenderQueueItem, RendererCameraHandle, RendererMaterialHandle,
        RendererMeshHandle, RendererShaderHandle, RendererTextureHandle,
    },
    internal::{MaterialParameter, MaterialTexture},
    resources::{MeshData, ShaderParameterSlot, ShaderTextureSlot, TextureType},
};

use pill_core::Result;
use pill_core::Timer;
use std::{collections::HashMap, sync::Arc};
use winit::{dpi::PhysicalSize, event::WindowEvent, window::Window};

pub struct DummyRenderer;

impl PillRenderer for DummyRenderer {
    fn new(_window: Arc<Window>, _config: EngineConfig) -> Result<Self> {
        Ok(DummyRenderer)
    }

    // --- Create ---

    fn create_shader(
        &mut self,
        _name: &str,
        _vertex_wgsl: &str,
        _fragment_wgsl: &str,
        _texture_slots: &HashMap<String, ShaderTextureSlot>,
        _parameter_slots: &Vec<(String, ShaderParameterSlot)>,
        _pass_engine_parameters: bool,
        _pass_camera_parameters: bool,
    ) -> Result<RendererShaderHandle> {
        Ok(RendererShaderHandle::default())
    }

    fn create_material(
        &mut self,
        _name: &str,
        _renderer_shader_handle: RendererShaderHandle,
        _textures: &Vec<(String, MaterialTexture)>,
        _parameters: &HashMap<String, MaterialParameter>,
    ) -> Result<RendererMaterialHandle> {
        Ok(RendererMaterialHandle::default())
    }

    fn create_texture(
        &mut self,
        _name: &str,
        _rgba: &[u8],
        _width: u32,
        _height: u32,
        _texture_type: TextureType,
    ) -> Result<RendererTextureHandle> {
        Ok(RendererTextureHandle::default())
    }

    fn create_mesh(&mut self, _name: &str, _mesh_data: &MeshData) -> Result<RendererMeshHandle> {
        Ok(RendererMeshHandle::default())
    }

    fn create_camera(&mut self) -> Result<RendererCameraHandle> {
        Ok(RendererCameraHandle::default())
    }

    // --- Update ---

    fn update_material_textures(
        &mut self,
        _renderer_material_handle: RendererMaterialHandle,
        _textures: &Vec<(String, MaterialTexture)>,
    ) -> Result<()> {
        Ok(())
    }

    fn update_material_parameters(
        &mut self,
        _renderer_material_handle: RendererMaterialHandle,
        _parameters: &HashMap<String, MaterialParameter>,
    ) -> Result<()> {
        Ok(())
    }

    // --- Destroy ---

    fn destroy_shader(&mut self, _renderer_shader_handle: RendererShaderHandle) -> Result<()> {
        Ok(())
    }

    fn destroy_material(
        &mut self,
        _renderer_material_handle: RendererMaterialHandle,
    ) -> Result<()> {
        Ok(())
    }

    fn destroy_texture(&mut self, _renderer_texture_handle: RendererTextureHandle) -> Result<()> {
        Ok(())
    }

    fn destroy_mesh(&mut self, _renderer_mesh_handle: RendererMeshHandle) -> Result<()> {
        Ok(())
    }

    fn destroy_camera(&mut self, _renderer_camera_handle: RendererCameraHandle) -> Result<()> {
        Ok(())
    }

    // --- Other ---

    fn resize(&mut self, _new_window_size: PhysicalSize<u32>) {
        // no-op for dummy
    }

    #[cfg(feature = "debug_ui")]
    fn pass_input_to_egui(&mut self, _event: &WindowEvent) -> Result<()> {
        Ok(())
    }

    #[cfg(feature = "debug_ui")]
    fn render(
        &mut self,
        _active_camera_entity_handle: EntityHandle,
        _render_queue: &[RenderQueueItem],
        _camera_component_storage: &ComponentStorage<CameraComponent>,
        _transform_component_storage: &ComponentStorage<TransformComponent>,
        _egui_ui: Box<dyn FnMut(&egui::Context)>,
        _delta_time: f32,
        _timer: &mut Timer,
    ) -> Result<()> {
        Ok(())
    }

    #[cfg(not(feature = "debug_ui"))]
    fn render(
        &mut self,
        _active_camera_entity_handle: EntityHandle,
        _render_queue: &[RenderQueueItem],
        _camera_component_storage: &ComponentStorage<CameraComponent>,
        _transform_component_storage: &ComponentStorage<TransformComponent>,
        _timer: &mut Timer,
    ) -> Result<()> {
        Ok(())
    }
}
