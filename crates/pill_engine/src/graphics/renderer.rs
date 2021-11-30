use core::fmt;
use std::{cmp::Ordering, fmt::Display, ops::{Range}, path::{Path, PathBuf}};
use std::{fmt::Binary, ops::{Add, Not, Shl, Sub}};

use pill_core::PillSlotMapKey;
use winit::{ 
    event::*, 
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
    dpi::PhysicalPosition,
};

use core::fmt::Debug;
use anyhow::{Result, Context, Error};
use crate::{ecs::{ComponentStorage, TransformComponent}, engine::Engine, resources::{Material, MaterialHandle, Mesh, MeshData, MeshHandle, TextureHandle, TextureType}};
use crate::ecs::Scene;
use lazy_static::lazy_static;
use crate::resources::{ RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle, RendererPipelineHandle, RendererTextureHandle };

use super::RenderQueueItem;


// --- Renderer error

#[derive(Debug)]
pub enum RendererError { 
    SurfaceLost,
    SurfaceOutOfMemory,
    SurfaceOther,
}

// --- Renderer trait definition

pub trait PillRenderer { 
    fn new(window: &Window) -> Self where Self: Sized;
    fn initialize(&self);

    fn render(&mut self, 
        render_queue: &Vec::<RenderQueueItem>, 
        transform_component_storage: &ComponentStorage<TransformComponent>
    ) -> Result<(), RendererError>;
    
    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>);
    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle, RendererError>;
    fn create_texture(&mut self, path: &PathBuf, name: &str, texture_type: TextureType) -> Result<RendererTextureHandle, RendererError>;
    fn create_texture_from_bytes(&mut self, bytes: &[u8], name: &str, texture_type: TextureType) -> Result<RendererTextureHandle, RendererError>;
    fn create_material(&mut self, name: &str, renderer_color_texture_handle: RendererTextureHandle, renderer_normal_texture_handle: RendererTextureHandle) -> Result<RendererMaterialHandle, RendererError>;
    fn update_material_texture(&mut self, material_renderer_handle: RendererMaterialHandle, renderer_texture_handle: RendererTextureHandle, texture_type: TextureType) -> Result<(), RendererError>;
    fn create_camera(&mut self) -> Result<RendererCameraHandle, RendererError>;
}

pub type Renderer = Box<dyn PillRenderer>;





