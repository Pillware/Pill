use std::sync::{Arc, Mutex};

pub struct EguiClient {
    events: Mutex<Vec<winit::event::WindowEvent>>,
    ui: Mutex<Option<Box<dyn Fn(&egui::Context) + Send>>>,
}

impl EguiClient {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            ui: Mutex::new(None),
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
