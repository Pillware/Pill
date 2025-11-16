use std::sync::Arc;

use anyhow::Result;
use wgpu::CommandEncoder;

use crate::graphics::renderer::{Pass, PillRenderer as EnginePillRenderer, WorldQuery};

pub struct PassEgui {
    label: String,
    window: Arc<winit::window::Window>,
    client: Arc<crate::ecs::EguiClient>,
    state: Option<PassEguiState>,
}

struct PassEguiState {
    egui: crate::renderer::egui::EguiRenderer,
}

impl PassEgui {
    pub fn new(
        label: &str,
        window: Arc<winit::window::Window>,
        client: Arc<crate::ecs::EguiClient>,
    ) -> Self {
        Self {
            label: label.to_string(),
            window,
            client,
            state: None,
        }
    }
}

impl Pass for PassEgui {
    fn get_label(&self) -> &str {
        &self.label
    }

    fn init(
        &mut self,
        renderer: &mut dyn EnginePillRenderer,
        _resources: &mut crate::resources::ResourceManager,
    ) -> Result<()> {
        let device = renderer.get_device();
        let format = renderer.get_surface_format();
        let egui = crate::renderer::egui::EguiRenderer::new(
            device,
            format,
            None,
            1,
            self.window.clone(),
        );
        self.state = Some(PassEguiState { egui });
        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut CommandEncoder,
        renderer: &mut dyn EnginePillRenderer,
        _resources: &mut crate::resources::ResourceManager,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        _world: &WorldQuery,
    ) -> Result<()> {
        let state = self.state.as_mut().expect("PassEgui not initialized");
        // Feed queued input events
        for ev in self.client.take_events().into_iter() {
            state.egui.handle_input(&ev);
        }
        // Build screen descriptor and UI callback
        let size = self.window.inner_size();
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point: state.egui.window_scale_factor,
        };
        let run_ui = self
            .client
            .take_ui()
            .unwrap_or_else(|| Box::new(|_ctx: &egui::Context| {}));
        state.egui.draw(
            renderer.get_device(),
            renderer.get_queue(),
            encoder,
            view,
            screen_descriptor,
            run_ui,
        )?;
        Ok(())
    }
}


