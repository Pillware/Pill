#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

pub mod egui;
pub mod resources;
pub mod wgpu;

pub use wgpu::Renderer;
