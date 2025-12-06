use std::sync::{Arc, Mutex};

pub struct EguiClient {
    events: Mutex<Vec<winit::event::WindowEvent>>,
    ui: Mutex<Option<Box<dyn Fn(&egui::Context) + Send>>>,

    // Vignette parameters (mutable from UI)
    pub vignette_intensity: Mutex<f32>,
    pub vignette_smoothness: Mutex<f32>,
    pub vignette_radius: Mutex<f32>,

    // Depth of Field parameters (mutable from UI)
    pub dof_focus_distance: Mutex<f32>,
    pub dof_focus_range: Mutex<f32>,
    pub dof_blur_strength: Mutex<f32>,
    pub dof_samples: Mutex<i32>,
    pub dof_enabled: Mutex<bool>,

    // Chromatic Aberration parameters (mutable from UI)
    pub chromatic_aberration_intensity: Mutex<f32>,
    pub chromatic_aberration_radial_falloff: Mutex<f32>,

    // Color Grading parameters (mutable from UI)
    pub color_grade_contrast: Mutex<f32>,
    pub color_grade_brightness: Mutex<f32>,
    pub color_grade_saturation: Mutex<f32>,
    pub color_grade_curve: Mutex<f32>,

    // Bloom parameters (mutable from UI)
    pub bloom_threshold: Mutex<f32>,
    pub bloom_intensity: Mutex<f32>,
    pub bloom_radius: Mutex<f32>,
}

impl EguiClient {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            ui: Mutex::new(None),
            vignette_intensity: Mutex::new(0.85),
            vignette_smoothness: Mutex::new(0.23),
            vignette_radius: Mutex::new(1.15),
            dof_focus_distance: Mutex::new(0.2),
            dof_focus_range: Mutex::new(1.2),
            dof_blur_strength: Mutex::new(0.5),
            dof_samples: Mutex::new(64),
            dof_enabled: Mutex::new(true),
            chromatic_aberration_intensity: Mutex::new(0.02),
            chromatic_aberration_radial_falloff: Mutex::new(3.9),
            color_grade_contrast: Mutex::new(1.0),
            color_grade_brightness: Mutex::new(0.0),
            color_grade_saturation: Mutex::new(1.0),
            color_grade_curve: Mutex::new(1.0),
            bloom_threshold: Mutex::new(0.37),
            bloom_intensity: Mutex::new(1.0),
            bloom_radius: Mutex::new(3.1),
        })
    }

    pub fn handle_input(&self, event: &winit::event::WindowEvent) {
        let mut q = self.events.lock().unwrap();
        q.push(event.clone());
    }

    pub fn take_events(&self) -> Vec<winit::event::WindowEvent> {
        let mut q = self.events.lock().unwrap();
        std::mem::take(&mut *q)
    }

    pub fn set_ui(&self, ui: Box<dyn Fn(&egui::Context) + Send>) {
        let mut u = self.ui.lock().unwrap();
        *u = Some(ui);
    }

    pub fn take_ui(&self) -> Option<Box<dyn Fn(&egui::Context) + Send>> {
        let mut u = self.ui.lock().unwrap();
        u.take()
    }
}
