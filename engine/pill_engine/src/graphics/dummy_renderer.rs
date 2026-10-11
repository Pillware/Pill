use crate::{
    app_config::EngineConfig,
    ecs::{EntityHandle, Scene},
    graphics::{
        BufferDesc, Pass, PassTextureHandle, PillRenderer, Pipeline, PipelineDesc,
        RendererTargetDesc, TextureData, WorldQuery,
    },
    resources::ResourceManager,
};

use pill_core::{PillTypeMap, Result, Timer};
use std::sync::Arc;
use winit::{dpi::PhysicalSize, window::Window};

pub struct DummyRenderer;

impl PillRenderer for DummyRenderer {
    fn new(_window: Arc<Window>, _config: EngineConfig) -> Result<Self> {
        Ok(DummyRenderer)
    }

    // --- Other ---

    fn resize(&mut self, _new_window_size: PhysicalSize<u32>) {}

    fn get_window(&self) -> Arc<winit::window::Window> {
        unimplemented!("DummyRenderer has no window")
    }

    fn render(
        &mut self,
        _active_camera_entity_handle: EntityHandle,
        _scene: &Scene,
        _globals: &PillTypeMap,
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

    fn create_buffer(&mut self, _desc: BufferDesc) -> Result<wgpu::Buffer> {
        unimplemented!("DummyRenderer has no wgpu Device")
    }

    fn create_pipeline(&mut self, _desc: PipelineDesc) -> Result<Pipeline> {
        unimplemented!("DummyRenderer has no wgpu Device")
    }

    fn create_render_target(&mut self, _desc: RendererTargetDesc) -> Result<PassTextureHandle> {
        Ok(PassTextureHandle::default())
    }

    fn create_depth_texture(&mut self, _label: &str) -> Result<PassTextureHandle> {
        Ok(PassTextureHandle::default())
    }

    fn get_render_target_view(&self, _handle: PassTextureHandle) -> Option<&wgpu::TextureView> {
        None
    }

    fn create_texture_from_pixels(
        &mut self,
        _name: &str,
        _data: TextureData,
    ) -> Result<PassTextureHandle> {
        Ok(PassTextureHandle::default())
    }

    fn get_texture_view(&self, _handle: PassTextureHandle) -> Option<wgpu::TextureView> {
        None
    }
}
