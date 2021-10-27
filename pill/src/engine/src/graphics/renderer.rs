use std::path::Path;

use winit::{ // Import dependencies
    event::*, // Bring all public items into scope
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
    dpi::PhysicalPosition,
};

use crate::scene::Scene;

#[derive(Debug)]
pub enum RendererError { 
    SurfaceLost,
    SurfaceOutOfMemory,
    SurfaceOther,
}

pub trait Pill_Renderer { 
    fn new(window: &Window) -> Self where Self: Sized;
    fn initialize(&self);
    fn render(&mut self, scene: &Scene, dt: std::time::Duration) -> Result<(), RendererError>;
    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>);
    fn create_model(&mut self, path: Box<&Path>) -> usize;
}

pub type Renderer = Box<dyn Pill_Renderer>;