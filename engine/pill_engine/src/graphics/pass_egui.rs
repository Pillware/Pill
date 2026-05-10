use crate::{
    ecs::EguiClient,
    graphics::{Pass, PillRenderer, WorldQuery},
};
use pill_core::{RendererError, Result, Timer};
use std::sync::Arc;
use winit::event::WindowEvent;

const BORDER_RADIUS: f32 = 2.0;

struct EguiDrawer {
    context: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    window_scale_factor: f32,
    window: Arc<winit::window::Window>,
}

impl EguiDrawer {
    fn new(
        device: &wgpu::Device,
        output_color_format: wgpu::TextureFormat,
        output_depth_format: Option<wgpu::TextureFormat>,
        msaa_samples: u32,
        window: Arc<winit::window::Window>,
    ) -> EguiDrawer {
        let window_scale_factor = window.scale_factor() as f32;
        let context = egui::Context::default();
        let id = context.viewport_id();

        let visuals = egui::Visuals {
            window_corner_radius: egui::CornerRadius::from(BORDER_RADIUS),
            window_shadow: egui::Shadow::NONE,
            ..Default::default()
        };
        context.set_visuals(visuals);

        let state = egui_winit::State::new(context.clone(), id, &window, None, None, None);

        let renderer = egui_wgpu::Renderer::new(
            device,
            output_color_format,
            output_depth_format,
            msaa_samples,
            false,
        );

        EguiDrawer {
            context,
            state,
            renderer,
            window_scale_factor,
            window,
        }
    }

    fn handle_input(&mut self, event: &WindowEvent) {
        let _ = self.state.on_window_event(&self.window, event);
    }

    #[allow(clippy::too_many_arguments)]
    fn record_draw_commands(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        window_surface_view: &wgpu::TextureView,
        screen_descriptor: egui_wgpu::ScreenDescriptor,
        mut run_ui: Box<dyn FnMut(&egui::Context)>,
        timer: &mut Timer,
    ) -> Result<()> {
        timer.record("Prepare window and input");

        let window = &self.window;
        let raw_input = self.state.take_egui_input(window);

        let full_output = self.context.run(raw_input, |_| {
            run_ui(&self.context);
        });

        timer.record("Handle platform output");

        self.state
            .handle_platform_output(window, full_output.platform_output);

        timer.record("Tesselate and update textures");

        let tris = self
            .context
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        for (id, image_delta) in &full_output.textures_delta.set {
            self.renderer
                .update_texture(device, queue, *id, image_delta);
        }

        timer.record("Update buffers and record render pass");

        self.renderer
            .update_buffers(device, queue, encoder, &tris, &screen_descriptor);

        let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: window_surface_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            label: Some("egui main render pass"),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        timer.record("Render");

        let mut render_pass = render_pass.forget_lifetime();

        self.renderer
            .render(&mut render_pass, &tris, &screen_descriptor);

        for texture_id in &full_output.textures_delta.free {
            self.renderer.free_texture(texture_id)
        }

        Ok(())
    }
}

pub struct PassEgui {
    window: Arc<winit::window::Window>,
    client: Arc<EguiClient>,
    drawer: Option<EguiDrawer>,
}

impl PassEgui {
    /// Creates the egui pass; `EguiDrawer` is initialised lazily in `Pass::init`.
    pub fn new(window: Arc<winit::window::Window>, client: Arc<EguiClient>) -> Self {
        Self {
            window,
            client,
            drawer: None,
        }
    }
}

impl Pass for PassEgui {
    fn get_label(&self) -> &str {
        "pass_egui"
    }

    fn init(&mut self, renderer: &mut dyn PillRenderer) -> Result<()> {
        self.drawer = Some(EguiDrawer::new(
            renderer.get_device(),
            renderer.get_surface_format(),
            None,
            1,
            self.window.clone(),
        ));
        Ok(())
    }

    fn draw(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        renderer: &mut dyn PillRenderer,
        _frame: &wgpu::SurfaceTexture,
        view: &wgpu::TextureView,
        _world: &WorldQuery<'_>,
    ) -> Result<()> {
        let drawer = self
            .drawer
            .as_mut()
            .ok_or_else(|| -> pill_core::PillError { RendererError::Other.into() })?;

        for ev in self.client.take_events() {
            drawer.handle_input(&ev);
        }

        let size = self.window.inner_size();
        let screen_desc = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point: drawer.window_scale_factor,
        };

        let ui_fn = self.client.take_ui();
        let run_ui: Box<dyn FnMut(&egui::Context)> = match ui_fn {
            Some(ui_function) => {
                Box::new(move |egui_context: &egui::Context| ui_function(egui_context))
            }
            None => Box::new(|_egui_context: &egui::Context| {}),
        };

        drawer.record_draw_commands(
            renderer.get_device(),
            renderer.get_queue(),
            encoder,
            view,
            screen_desc,
            run_ui,
            &mut Timer::new(),
        )
    }
}
