use pill_core::Result;
use pill_core::Timer;
use std::sync::Arc;
use winit::event::WindowEvent;

const BORDER_RADIUS: f32 = 2.0;

pub struct EguiDrawer {
    pub context: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    pub window_scale_factor: f32,
    pub window: Arc<winit::window::Window>,
}

pub struct EguiDrawOutput {
    pub command_buffers: Vec<wgpu::CommandBuffer>,
    pub textures_to_free: Vec<egui::TextureId>,
}

impl EguiDrawer {
    pub fn new(
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
            egui_wgpu::RendererOptions {
                depth_stencil_format: output_depth_format,
                msaa_samples,
                dithering: false,
                ..Default::default()
            },
        );

        EguiDrawer {
            context,
            state,
            renderer,
            window_scale_factor,
            window,
        }
    }

    pub fn handle_input(&mut self, event: &WindowEvent) {
        let _ = self.state.on_window_event(&self.window, event);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_draw_commands(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        window_surface_view: &wgpu::TextureView,
        screen_descriptor: egui_wgpu::ScreenDescriptor,
        // run_ui: impl FnOnce(&egui::Context),
        mut run_ui: Box<dyn FnMut(&egui::Context)>,
        timer: &mut Timer,
    ) -> Result<EguiDrawOutput> {
        timer.record("Prepare window and input");

        let window = &self.window;
        let raw_input = self.state.take_egui_input(window);

        let context = self.context.clone();
        let full_output = self.context.run_ui(raw_input, |_| {
            run_ui(&context);
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

        let command_buffers =
            self.renderer
                .update_buffers(device, queue, encoder, &tris, &screen_descriptor);

        let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: window_surface_view,
                depth_slice: None,
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
            multiview_mask: None,
        });

        timer.record("Render");

        self.renderer.render(
            &mut render_pass.forget_lifetime(),
            &tris,
            &screen_descriptor,
        );

        Ok(EguiDrawOutput {
            command_buffers,
            textures_to_free: full_output.textures_delta.free,
        })
    }

    pub fn free_textures(&mut self, texture_ids: &[egui::TextureId]) {
        for texture_id in texture_ids {
            self.renderer.free_texture(texture_id);
        }
    }
}
