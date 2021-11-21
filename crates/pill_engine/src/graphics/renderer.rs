use std::{ops::Range, path::{Path, PathBuf}};
use winit::{ 
    event::*, 
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
    dpi::PhysicalPosition,
};

use anyhow::{Result, Context, Error};
use crate::{game::Engine, resources::{Material, MaterialHandle, Mesh, MeshData, MeshHandle, TextureHandle}};
use crate::ecs::Scene;

static order_mask_range: Range<u32> =             0..4;
static material_instance_mask_range: Range<u32> = 5..10;
static mesh_mask_range: Range<u32> =              11..20;

static order_mask_shift: u32 = 32 - order_mask_range.end - 1; // 27
static material_instance_mask_shift: u32 = 32 - material_instance_mask_range.end - 1; // 21
static mesh_mask_range_shift: u32 = 32 - mesh_mask_range.end - 1; // 11

// [TODO] Generate these masks from ranges above
static order_mask: u32 =             0b11111_000000_0000000000_00000000000;
static material_instance_mask: u32 = 0b00000_111111_0000000000_00000000000;
static mesh_mask: u32 =              0b00000_000000_1111111111_00000000000;

pub fn create_render_queue_key(engine: &Engine, material_handle: &MaterialHandle, mesh_handle: &MeshHandle) -> Result<RenderQueueKey> { 
    let mesh = engine.resource_manager.get_resource::<Mesh, MeshHandle>(mesh_handle)?;
    let material = engine.resource_manager.get_resource::<Material, MaterialHandle>(material_handle)?;

    let render_queue_key = 
        (material.order << order_mask_shift) | 
        (material_handle.index << material_instance_mask_shift) | 
        (mesh_handle.index << mesh_mask_range_shift);

    Ok(render_queue_key)
}

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
    fn create_mesh(&mut self, mesh_data: &MeshData) -> Result<u32, RendererError>;
    fn create_texture(&mut self, path: &PathBuf) -> Result<u32, RendererError>;
    fn create_material(&mut self, color_texture: TextureHandle, normal_texture: TextureHandle) -> Result<u32, RendererError>;
    fn update_material(&mut self, index: u32, updated_material: &Material) -> Result<u32, RendererError>;
}

pub type Renderer = Box<dyn PillRenderer>;

pub type RenderQueueKey = u32;