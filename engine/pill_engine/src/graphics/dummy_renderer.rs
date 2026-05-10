use crate::{
    app_config::EngineConfig,
    ecs::{EntityHandle, Scene},
    graphics::{
        BufferDesc, Pass, PillRenderer, PipelineV2, PipelineV2Desc, RendererCameraHandle,
        RendererTargetDesc, RendererTextureHandle, WorldQuery,
    },
    resources::{ResourceManager, ShaderParameterSlot, ShaderTextureSlot},
};

use pill_core::Result;
use pill_core::Timer;
use std::collections::HashMap;
use std::sync::Arc;
use winit::{dpi::PhysicalSize, window::Window};

pub struct DummyRenderer;

impl PillRenderer for DummyRenderer {
    fn new(_window: Arc<Window>, _config: EngineConfig) -> Result<Self> {
        Ok(DummyRenderer)
    }

    // --- Create ---

    fn create_shader_struct(
        &mut self,
        _name: &str,
        _vertex_wgsl: &str,
        _fragment_wgsl: &str,
        _texture_slots: &HashMap<String, ShaderTextureSlot>,
        _parameter_slots: &[(String, ShaderParameterSlot)],
        _pass_engine_parameters: bool,
        _pass_camera_parameters: bool,
    ) -> Result<crate::renderer::resources::RendererShader> {
        unimplemented!("DummyRenderer has no GPU shader creation")
    }

    fn create_camera(&mut self) -> Result<RendererCameraHandle> {
        Ok(RendererCameraHandle::default())
    }

    // --- Destroy ---

    fn destroy_camera(&mut self, _renderer_camera_handle: RendererCameraHandle) -> Result<()> {
        Ok(())
    }

    // --- Other ---

    fn resize(&mut self, _new_window_size: PhysicalSize<u32>) {}

    fn get_window(&self) -> std::sync::Arc<winit::window::Window> {
        unimplemented!("DummyRenderer has no window")
    }

    fn render(
        &mut self,
        _active_camera_entity_handle: EntityHandle,
        _scene: &Scene,
        _globals: &pill_core::PillTypeMap,
        _delta_time: f32,
        _timer: &mut Timer,
        _resource_manager: &ResourceManager,
    ) -> Result<()> {
        Ok(())
    }

    // --- Pass API ---

    fn set_passes(&mut self, _passes: Vec<Box<dyn Pass>>) -> Result<()> {
        Ok(())
    }

    fn get_surface_size(&self) -> (u32, u32) {
        (0, 0)
    }

    fn get_device(&self) -> &wgpu::Device {
        unimplemented!("DummyRenderer has no wgpu Device")
    }

    fn get_queue(&self) -> &wgpu::Queue {
        unimplemented!("DummyRenderer has no wgpu Queue")
    }

    fn get_surface_format(&self) -> wgpu::TextureFormat {
        wgpu::TextureFormat::Rgba8UnormSrgb
    }

    fn get_engine_parameters(&self) -> &crate::renderer::resources::EngineParameters {
        unimplemented!("DummyRenderer has no EngineParameters")
    }

    fn get_camera_bind_group_layout(&self) -> wgpu::BindGroupLayout {
        unimplemented!("DummyRenderer has no wgpu Device")
    }

    fn create_buffer(&mut self, _desc: BufferDesc) -> Result<wgpu::Buffer> {
        unimplemented!("DummyRenderer has no wgpu Device")
    }

    fn create_pipeline_v2(&mut self, _desc: PipelineV2Desc) -> Result<PipelineV2> {
        unimplemented!("DummyRenderer has no wgpu Device")
    }

    fn create_render_target(&mut self, _desc: RendererTargetDesc) -> Result<RendererTextureHandle> {
        Ok(RendererTextureHandle::default())
    }

    fn create_depth_texture(&mut self, _label: &str) -> Result<RendererTextureHandle> {
        Ok(RendererTextureHandle::default())
    }

    fn get_render_target_view(&self, _handle: RendererTextureHandle) -> Option<&wgpu::TextureView> {
        None
    }

    fn create_texture_from_pixels(
        &mut self,
        _name: &str,
        _mip_pixels: &[&[u8]],
        _base_width: u32,
        _base_height: u32,
        _format: wgpu::TextureFormat,
    ) -> RendererTextureHandle {
        RendererTextureHandle::default()
    }

    fn get_texture_view(&self, _handle: RendererTextureHandle) -> Option<wgpu::TextureView> {
        None
    }
}
