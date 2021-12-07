pub use crate::ecs::*;
use crate::{
    internal::Engine, 
    graphics::{RenderQueueKey, compose_render_queue_key}, 
    resources::{Material, MaterialHandle, Mesh, MeshHandle}
};
use anyhow::{Result, Context, Error};

pub struct MeshRenderingComponent {
    mesh: Option<MeshHandle>,
    material: Option<MaterialHandle>,
    pub(crate) render_queue_key: RenderQueueKey,
}

impl Component for MeshRenderingComponent {
    type Storage = ComponentStorage<MeshRenderingComponent>; 
}

impl MeshRenderingComponent {
    pub fn new(engine: &Engine, mesh_handle: &MeshHandle, material_handle: &MaterialHandle) -> Result<Self> {
        let render_queue_key = compose_render_queue_key(engine, material_handle, mesh_handle)?;

        let mesh_rendering_component = Self { 
            mesh: Some(mesh_handle.clone()),
            material: Some(material_handle.clone()),
            render_queue_key,
        };
    
        Ok(mesh_rendering_component)
    }
}

impl Default for MeshRenderingComponent {
    fn default() -> Self {
        Self { 
            mesh: Option::None,
            material: Option::None,
            render_queue_key: RenderQueueKey::default(),
        }
    }
}







