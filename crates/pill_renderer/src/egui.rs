use std::sync::Arc;

use egui::epaint::Shadow;
use egui::{Context, Visuals};
use egui_wgpu::ScreenDescriptor;
use egui_wgpu::Renderer;

use egui_winit::State;
use wgpu::{CommandEncoder, Device, Queue, TextureFormat, TextureView};
use winit::event::WindowEvent;
use winit::window::Window;

pub struct EguiRenderer {
    pub context: Context,
    state: State,
    renderer: Renderer,
    pub window_scale_factor: f32,
    pub window: Arc<winit::window::Window>,
}

impl EguiRenderer {
    pub fn new(
        device: &Device,
        output_color_format: TextureFormat,
        output_depth_format: Option<TextureFormat>,
        msaa_samples: u32,
        window: Arc<winit::window::Window>,
    ) -> EguiRenderer {
        let window_scale_factor = window.scale_factor() as f32;
        let egui_context = egui::Context::default();
        let id = egui_context.viewport_id();
        const BORDER_RADIUS: f32 = 2.0;
        
        let visuals = egui::Visuals {
            window_rounding: egui::Rounding::same(BORDER_RADIUS),
            window_shadow: egui::Shadow::NONE,
            ..Default::default()
        };
        egui_context.set_visuals(visuals);

        let egui_state = egui_winit::State::new(egui_context.clone(), id, &window, None, None);

        let egui_renderer = egui_wgpu::Renderer::new(
            device,
            output_color_format,
            output_depth_format,
            msaa_samples,
        );

        EguiRenderer {
            context: egui_context,
            state: egui_state,
            renderer: egui_renderer,
            window_scale_factor,
            window
        }
    }

    pub fn handle_input(&mut self, event: &WindowEvent) {
        let _ = self.state.on_window_event(&self.window, event);
    }

    pub fn draw(
        &mut self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        window_surface_view: &TextureView,
        screen_descriptor: ScreenDescriptor,
        run_ui: impl FnOnce(&Context),
    ) {
        let window = &self.window;
        let raw_input = self.state.take_egui_input(&window);
        let full_output = self.context.run(raw_input, |ui| {
            run_ui(&self.context);
        });

        self.state
            .handle_platform_output(&window, full_output.platform_output);

        let tris = self
            .context
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        for (id, image_delta) in &full_output.textures_delta.set {
            self.renderer
                .update_texture(&device, &queue, *id, &image_delta);
        }
        self.renderer
            .update_buffers(&device, &queue, encoder, &tris, &screen_descriptor);
        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &window_surface_view,
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
        self.renderer.render(&mut rpass, &tris, &screen_descriptor);
        drop(rpass);
        for x in &full_output.textures_delta.free {
            self.renderer.free_texture(x)
        }
    }
}

pub fn test_window(ui: &Context) {
    egui::Window::new("PillEngine")
        // .vscroll(true)
        .default_open(true)
        // .max_width(1000.0)
        // .max_height(800.0)
        // .default_width(800.0)
        .resizable(true)
        .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
        .show(&ui, |mut ui| {
            if ui.add(egui::Button::new("Click me")).clicked() {
                println!("PRESSED")
            }
        });
}