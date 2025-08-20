use std::sync::Arc;

use egui::epaint::Shadow;
use egui::{Context, Visuals};
use egui_wgpu::ScreenDescriptor;
use egui_wgpu::Renderer;

use egui_winit::State;
use pill_core::Timer;
use wgpu::{CommandEncoder, Device, Queue, TextureFormat, TextureView};
use winit::event::WindowEvent;
use winit::window::Window;
use anyhow::{Error, Result};

const BORDER_RADIUS: f32 = 2.0;

pub struct EguiDrawer {
    pub context: Context,
    state: State,
    renderer: Renderer,
    pub window_scale_factor: f32,
    pub window: Arc<winit::window::Window>,
}

impl EguiDrawer {
    pub fn new(
        device: &Device,
        output_color_format: TextureFormat,
        output_depth_format: Option<TextureFormat>,
        msaa_samples: u32,
        window: Arc<winit::window::Window>,
    ) -> EguiDrawer {
        let window_scale_factor = window.scale_factor() as f32;
        let context = egui::Context::default();
        let id = context.viewport_id();

        let visuals = egui::Visuals {
            window_rounding: egui::Rounding::same(BORDER_RADIUS),
            window_shadow: egui::Shadow::NONE,
            ..Default::default()
        };
        context.set_visuals(visuals);

        let state = egui_winit::State::new(
            context.clone(), 
            id, 
            &window, 
            None, 
            None
        );

        let renderer = egui_wgpu::Renderer::new(
            device,
            output_color_format,
            output_depth_format,
            msaa_samples,
        );

        EguiDrawer {
            context,
            state,
            renderer,
            window_scale_factor,
            window
        }
    }

    pub fn handle_input(&mut self, event: &WindowEvent) {
        let _ = self.state.on_window_event(&self.window, event);
    }

    pub fn record_draw_commands(
        &mut self,
        device: &Device,
        queue: &Queue,
        encoder: &mut CommandEncoder,
        window_surface_view: &TextureView,
        screen_descriptor: ScreenDescriptor,
        run_ui: impl FnOnce(&Context),
        timer: &mut Timer,
    ) -> Result<()> {
        timer.record("Prepare window and input");

        let window = &self.window;
        let raw_input = self.state.take_egui_input(&window);
        let full_output = self.context.run(raw_input, |_| {
            run_ui(&self.context);
        });

        timer.record("Handle platform output");

        self.state.handle_platform_output(&window, full_output.platform_output);

        timer.record("Tesselate and update textures");

        let tris = self.context.tessellate(full_output.shapes, full_output.pixels_per_point);
        for (id, image_delta) in &full_output.textures_delta.set {
            self.renderer.update_texture(&device, &queue, *id, &image_delta);
        }

        timer.record("Update buffers and record render pass");

        self.renderer.update_buffers(&device, &queue, encoder, &tris, &screen_descriptor);

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
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

        timer.record("Render");

        self.renderer.render(&mut render_pass, &tris, &screen_descriptor);

        drop(render_pass);

        for texture_id in &full_output.textures_delta.free {
            self.renderer.free_texture(texture_id)
        }

        Ok(())
    }
}

