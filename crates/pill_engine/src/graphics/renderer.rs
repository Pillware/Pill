use std::path::Path;
use winit::{ 
    event::*, 
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
    dpi::PhysicalPosition,
};

use crate::ecs::Scene;

#[derive(Debug)]
pub enum RendererError { 
    SurfaceLost,
    SurfaceOutOfMemory,
    SurfaceOther,
}

pub trait PillRenderer { 
    fn new(window: &Window) -> Self where Self: Sized;
    fn initialize(&self);
    fn render(&mut self, scene: &Scene, dt: std::time::Duration) -> Result<(), RendererError>;
    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>);
    fn create_model(&mut self, path: Box<&Path>) -> Result<usize, RendererError>;
    fn create_texture(&mut self, path: Box<&Path>) -> Result<usize, RendererError>;
}

pub type Renderer = Box<dyn PillRenderer>;