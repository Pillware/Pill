use std::time::Instant;

// Repaint signal that egui needs for requesting a repaint from another thread
// It sends the custom RequestRedraw event to the winit event loop
// This one is just a mock, does nothing
// Check implemententation of working version here: https://github1s.com/hasenbanck/egui_example/blob/HEAD/src/main.rs 
struct RepaintSignal();
impl epi::backend::RepaintSignal for RepaintSignal {
    fn request_repaint(&self) { }
}

pub struct EguiState {
    render_pass: egui_wgpu_backend::RenderPass,
    app: egui_demo_lib::WrapApp,
    platform: egui_winit_platform::Platform,
    paint_jobs: Vec<egui::ClippedMesh>,
    start_time: Instant,
    previous_frame_time: Option<f32>, 
}

impl EguiState {
    pub fn new(device: &wgpu::Device, surface_configuration: &wgpu::SurfaceConfiguration, window_scale_factor: f64) -> Self {
        let render_pass = egui_wgpu_backend::RenderPass::new(device, surface_configuration.format, 1);
        let app = egui_demo_lib::WrapApp::default();

        let platform_descriptor = egui_winit_platform::PlatformDescriptor {
            physical_width: surface_configuration.width as u32,
            physical_height: surface_configuration.height as u32,
            scale_factor: window_scale_factor,
            font_definitions: egui::FontDefinitions::default(),
            style: Default::default(),
        };
        let platform = egui_winit_platform::Platform::new(platform_descriptor);

        let paint_jobs = Vec::<egui::ClippedMesh>::new();
        let start_time= Instant::now(); // [TODO] Move to engine
        let previous_frame_time = None;

        EguiState {
            render_pass,
            app,
            platform,
            paint_jobs,
            start_time,
            previous_frame_time,
        }
    }

    pub fn update(&mut self, window_scale_factor: f64) {
      
        // Pass the winit events
        platform.handle_event(&event); 

        // Set elapsed time since the start of the program
        self.platform.update_time(self.start_time.elapsed().as_secs_f64());

        // Begin to draw the UI frame.
        let update_start = Instant::now(); // [TODO] Should be time at start of the frame
        self.platform.begin_frame();
        let app_output = epi::backend::AppOutput::default();

        let repaint_signal = std::sync::Arc::new(RepaintSignal());

        let mut frame =  epi::Frame::new(epi::backend::FrameData {
            info: epi::IntegrationInfo {
                name: "pill_egui",
                web_info: None,
                cpu_usage: self.previous_frame_time,
                native_pixels_per_point: Some(window_scale_factor as _),
                prefer_dark_mode: None,
            },
            output: app_output,
            repaint_signal: repaint_signal.clone(),
        });

        // Draw the demo application.
        epi::App::update(&mut self.app, &self.platform.context(), &mut frame);

        // End the UI frame. We could now handle the output and draw the UI with the backend.
        let (_output, paint_commands) = self.platform.end_frame(None);
        self.paint_jobs = self.platform.context().tessellate(paint_commands);

        let frame_time = (Instant::now() - update_start).as_secs_f64() as f32;
        self.previous_frame_time = Some(frame_time);
    }
}

pub struct EguiDrawer {
    
}

impl EguiDrawer {
    pub fn new() -> Self {
        EguiDrawer { }
    }

    pub fn record_draw_commands(
        &self, 
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder, 
        frame_view: &wgpu::TextureView,
        surface_configuration: &wgpu::SurfaceConfiguration,
        window_scale_factor: f64,
        egui_state: &mut EguiState
    ){
        let screen_descriptor = egui_wgpu_backend::ScreenDescriptor {
            physical_width: surface_configuration.width,
            physical_height: surface_configuration.height,
            scale_factor: window_scale_factor as f32,
        };
        
        egui_state.render_pass.update_texture(device, queue, &egui_state.platform.context().font_image());
        egui_state.render_pass.update_user_textures(device, queue);
        egui_state.render_pass.update_buffers(device, queue, &egui_state.paint_jobs, &screen_descriptor);

        // Record all render passes.
        egui_state.render_pass.execute(encoder, &frame_view, &egui_state.paint_jobs, &screen_descriptor, Some(wgpu::Color::BLACK)).unwrap();
    }
}


// pub struct EguiApp {
//     demo_windows: egui_demo_lib::DemoWindows,// super::DemoWindows,
// }

// impl epi::App for EguiApp {
//     fn name(&self) -> &str {
//         "pill-engine"
//     }

//     fn setup(
//         &mut self,
//         _ctx: &egui::CtxRef,
//         _frame: &epi::Frame,
//         _storage: Option<&dyn epi::Storage>,
//     ) {
//         #[cfg(feature = "persistence")]
//         if let Some(storage) = _storage {
//             *self = epi::get_value(storage, epi::APP_KEY).unwrap_or_default();
//         }
//     }

//     #[cfg(feature = "persistence")]
//     fn save(&mut self, storage: &mut dyn epi::Storage) {
//         epi::set_value(storage, epi::APP_KEY, self);
//     }

//     fn update(&mut self, ctx: &egui::CtxRef, _frame: &epi::Frame) {
//         self.demo_windows.ui(ctx);
//     }
// }







// /// A menu bar in which you can select different demo windows to show.
// #[derive(Default)]
// #[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
// #[cfg_attr(feature = "serde", serde(default))]
// pub struct DemoWindows {
//     //demos: Demos,
//     //tests: Tests,
// }

// impl DemoWindows {
//     /// Show the app ui (menu bar and windows).
//     /// `sidebar_ui` can be used to optionally show some things in the sidebar
//     pub fn ui(&mut self, ctx: &CtxRef) {
//         //let Self { demos, tests } = self;

//         egui::SidePanel::right("egui_demo_panel")
//             .min_width(150.0)
//             .default_width(180.0)
//             .show(ctx, |ui| {
//                 egui::trace!(ui);
//                 ui.vertical_centered(|ui| {
//                     ui.heading("✒ egui demos");
//                 });

//                 ui.separator();

//                 ScrollArea::vertical().show(ui, |ui| {
//                     use egui::special_emojis::{GITHUB, OS_APPLE, OS_LINUX, OS_WINDOWS};

//                     ui.vertical_centered(|ui| {
//                         ui.label("egui is an immediate mode GUI library written in Rust.");

//                         ui.label(format!(
//                             "egui runs on the web, or natively on {}{}{}",
//                             OS_APPLE, OS_LINUX, OS_WINDOWS,
//                         ));

//                         ui.hyperlink_to(
//                             format!("{} egui home page", GITHUB),
//                             "https://github.com/emilk/egui",
//                         );
//                     });

//                     // ui.separator();
//                     // demos.checkboxes(ui);
//                     // ui.separator();
//                     // tests.checkboxes(ui);
//                     // ui.separator();

//                     ui.vertical_centered(|ui| {
//                         if ui.button("Organize windows").clicked() {
//                             ui.ctx().memory().reset_areas();
//                         }
//                     });
//                 });
//             });

//         // egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
//         //     show_menu_bar(ui);
//         // });

//         // {
//         //     let mut fill = ctx.style().visuals.extreme_bg_color;
//         //     if !cfg!(target_arch = "wasm32") {
//         //         // Native: WrapApp uses a transparent window, so let's show that off:
//         //         // NOTE: the OS compositor assumes "normal" blending, so we need to hack it:
//         //         let [r, g, b, _] = fill.to_array();
//         //         fill = egui::Color32::from_rgba_premultiplied(r, g, b, 180);
//         //     }
//         //     let frame = egui::Frame::none().fill(fill);
//         //     egui::CentralPanel::default().frame(frame).show(ctx, |_| {});
//         // }

//         self.windows(ctx);
//     }

//     /// Show the open windows.
//     fn windows(&mut self, ctx: &CtxRef) {
//         let Self { demos, tests } = self;

//         demos.windows(ctx);
//         tests.windows(ctx);
//     }
// }
