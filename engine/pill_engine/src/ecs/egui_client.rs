#![cfg(feature = "ui")]

use std::sync::{Arc, Mutex};

type UiFunction = Box<dyn Fn(&egui::Context) + Send>;

pub struct EguiClient {
    events: Mutex<Vec<winit::event::WindowEvent>>,
    ui: Mutex<Option<UiFunction>>,
}

impl EguiClient {
    /// Creates a new shared egui client with empty event and UI queues.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            events: Mutex::new(Vec::new()),
            ui: Mutex::new(None),
        })
    }

    /// Enqueues a window event for delivery to the egui input handler on the next frame.
    pub fn handle_input(&self, event: winit::event::WindowEvent) {
        self.events.lock().unwrap().push(event);
    }

    /// Drains and returns all queued window events, leaving the queue empty.
    pub fn take_events(&self) -> Vec<winit::event::WindowEvent> {
        std::mem::take(&mut self.events.lock().unwrap())
    }

    /// Replaces the current UI function with `ui_fn`; called once per frame before rendering.
    pub fn set_ui(&self, ui_fn: impl Fn(&egui::Context) + Send + 'static) {
        *self.ui.lock().unwrap() = Some(Box::new(ui_fn));
    }

    /// Takes ownership of the pending UI function, leaving `None` in its place.
    pub fn take_ui(&self) -> Option<UiFunction> {
        self.ui.lock().unwrap().take()
    }
}
