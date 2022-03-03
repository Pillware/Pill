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
    pub app: egui_demo_lib::WrapApp,
    pub platform: egui_winit_platform::Platform,
    pub paint_jobs: Vec<egui::ClippedMesh>,
    pub start_time: Instant,
    pub previous_frame_time: Option<f32>, 
    pub window_size: winit::dpi::PhysicalSize<u32>, 
    pub window_scale_factor: f64
}

impl EguiState {
    pub fn new(window_size: winit::dpi::PhysicalSize<u32>, window_scale_factor: f64) -> Self {
        let app = egui_demo_lib::WrapApp::default();

        let platform_descriptor = egui_winit_platform::PlatformDescriptor {
            physical_width: window_size.width,
            physical_height: window_size.height,
            scale_factor: window_scale_factor,
            font_definitions: egui::FontDefinitions::default(),
            style: Default::default(),
        };
        let platform = egui_winit_platform::Platform::new(platform_descriptor);

        let paint_jobs = Vec::<egui::ClippedMesh>::new();
        let start_time = Instant::now(); // [TODO] Move to engine
        let previous_frame_time = None;

        EguiState {
            app,
            platform,
            paint_jobs,
            start_time,
            previous_frame_time,
            window_size,
            window_scale_factor
        }
    }

    pub fn pass_event(&mut self, event: &winit::event::Event<()>) {
        self.platform.handle_event(&event); 
    }

    pub fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>) {
        self.window_size = new_window_size;
    }

    pub fn update_time(&mut self){
         self.platform.update_time(self.start_time.elapsed().as_secs_f64());
    }

    pub fn update(&mut self) {
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
                native_pixels_per_point: Some(self.window_scale_factor as _),
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
