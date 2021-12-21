use core::fmt;
use std::{cmp::Ordering, fmt::Display, ops::{Range}, path::{Path, PathBuf}};
use std::{fmt::Binary, ops::{Add, Not, Shl, Sub}};


use pill_core::PillSlotMapKey;
use thiserror::Error;
use winit::{ 
    event::*, 
    event_loop::{ControlFlow, EventLoop},
    window::{Window, WindowBuilder},
    dpi::PhysicalPosition,
};

use core::fmt::Debug;
use anyhow::{Result, Context, Error};
use crate::{ecs::{ComponentStorage, TransformComponent, CameraComponent, EntityHandle}, engine::Engine, resources::{Material, MaterialHandle, Mesh, MeshData, MeshHandle, TextureHandle, TextureType, ResourceManager, TextureMap, ParameterMap}};
use crate::ecs::Scene;
use lazy_static::lazy_static;
use crate::resources::{ RendererCameraHandle, RendererMaterialHandle, RendererMeshHandle, RendererPipelineHandle, RendererTextureHandle };

use super::RenderQueueItem;


// --- Renderer error

#[derive(Error, Debug)]
pub enum RendererError { 
    #[error("Renderer resource not found \n\nSource: ")]
    RendererResourceNotFound,
    #[error("Renderer surface lost \n\nSource: ")]
    SurfaceLost,
    #[error("Renderer surface out of memory \n\nSource: ")]
    SurfaceOutOfMemory,
    #[error("Undefined renderer surface error \n\nSource: ")]
    SurfaceOther,
}

// --- Renderer trait definition

pub trait PillRenderer { 
    fn new(window: &Window) -> Self where Self: Sized;

    fn render(&mut self, 
        active_camera_entity_handle: EntityHandle, // [TODO] Work only in ECS approach in which index of entity equals index of its components
        render_queue: &Vec::<RenderQueueItem>, 
        camera_component_storage: &ComponentStorage<CameraComponent>,
        transform_component_storage: &ComponentStorage<TransformComponent>
    ) -> Result<(), RendererError>;
    
    fn resize(&mut self, new_window_size: winit::dpi::PhysicalSize<u32>);
    fn set_master_pipeline(&mut self, vertex_shader_bytes: &[u8], fragment_shader_bytes: &[u8],) -> Result<()>; // [TODO] This can be later changed to create_pipeline, if shader parsing will be implemeneted
    
    fn create_mesh(&mut self, name: &str, mesh_data: &MeshData) -> Result<RendererMeshHandle>;
    fn create_texture(&mut self, path: &PathBuf, name: &str, texture_type: TextureType) -> Result<RendererTextureHandle>;
    fn create_texture_from_bytes(&mut self, bytes: &[u8], name: &str, texture_type: TextureType) -> Result<RendererTextureHandle>;
    fn create_material(&mut self, name: &str, textures: &TextureMap, parameters: &ParameterMap) -> Result<RendererMaterialHandle>;
    fn create_camera(&mut self) -> Result<RendererCameraHandle>;

    fn update_material_textures(&mut self, renderer_material_handle: RendererMaterialHandle, textures: &TextureMap) -> Result<()>;
    fn update_material_parameters(&mut self, renderer_material_handle: RendererMaterialHandle, parameters: &ParameterMap) -> Result<()>;

    fn destroy_texture(&mut self, renderer_texture_handle: RendererTextureHandle) -> Result<()>;
    fn destroy_material(&mut self, renderer_material_handle: RendererMaterialHandle) -> Result<()>;
    fn destroy_camera(&mut self, renderer_camera_handle: RendererCameraHandle) -> Result<()>;
    fn destroy_mesh(&mut self, renderer_mesh_handle: RendererMeshHandle) -> Result<()>;
}

pub type Renderer = Box<dyn PillRenderer>;





