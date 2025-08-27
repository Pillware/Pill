use crate::{
    engine::Engine,
    ecs::{EntityHandle, ComponentStorage, TransformComponent, CameraComponent},
    resources::{MaterialTextureMap, MaterialParameterMap, MeshData, TextureType},
    graphics::RenderQueueItem,
    graphics::{
        PillRenderer,
        RendererCameraHandle,
        RendererMaterialHandle,
        RendererMeshHandle,
        RendererPipelineHandle,
        RendererTextureHandle,
    },
};

use pill_core::Timer;

use std::{path::PathBuf, sync::Arc};
use anyhow::{Result, Ok};
use winit::{dpi::PhysicalSize, window::Window, event::WindowEvent};
use image::DynamicImage;
use config::Config;

pub struct DummyRenderer;

impl PillRenderer for DummyRenderer {
    fn new(_window: Arc<Window>, _config: Config) -> Self {
        DummyRenderer
    }

    fn resize(&mut self, _new_window_size: PhysicalSize<u32>) {}

    fn set_master_pipeline(&mut self, _vertex_shader_bytes: &[u8], _fragment_shader_bytes: &[u8]) -> Result<()> {
        Ok(())
    }

    fn create_mesh(&mut self, _name: &str, _mesh_data: &MeshData) -> Result<RendererMeshHandle> {
        Ok(RendererMeshHandle::default())
    }

    fn create_texture(&mut self, _name: &str, _image_data: &DynamicImage, _texture_type: TextureType) -> Result<RendererTextureHandle> {
        Ok(RendererTextureHandle::default())
    }

    fn create_material(&mut self, _name: &str, _textures: &MaterialTextureMap, _parameters: &MaterialParameterMap) -> Result<RendererMaterialHandle> {
        Ok(RendererMaterialHandle::default())
    }

    fn create_camera(&mut self) -> Result<RendererCameraHandle> {
        Ok(RendererCameraHandle::default())
    }

    fn update_material_textures(&mut self, _renderer_material_handle: RendererMaterialHandle, _textures: &MaterialTextureMap) -> Result<()> {
        Ok(())
    }

    fn update_material_parameters(&mut self, _renderer_material_handle: RendererMaterialHandle, _parameters: &MaterialParameterMap) -> Result<()> {
        Ok(())
    }

    fn destroy_mesh(&mut self, _renderer_mesh_handle: RendererMeshHandle) -> Result<()> {
        Ok(())
    }

    fn destroy_texture(&mut self, _renderer_texture_handle: RendererTextureHandle) -> Result<()> {
        Ok(())
    }

    fn destroy_material(&mut self, _renderer_material_handle: RendererMaterialHandle) -> Result<()> {
        Ok(())
    }

    fn destroy_camera(&mut self, _renderer_camera_handle: RendererCameraHandle) -> Result<()> {
        Ok(())
    }

    fn pass_input_to_egui(&mut self, _event: &WindowEvent) -> Result<()> {
        Ok(())
    }

    fn render(
        &mut self,
        _active_camera_entity_handle: EntityHandle,
        _render_queue: &Vec<RenderQueueItem>,
        _camera_component_storage: &ComponentStorage<CameraComponent>,
        _transform_component_storage: &ComponentStorage<TransformComponent>,
        _egui_ui: Box<dyn Fn(&egui::Context)>,
        _timer: &mut Timer,
    ) -> Result<()> {
        Ok(())
    }
}

