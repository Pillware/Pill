pub use crate::ecs::*;
use crate::{
    internal::Engine, 
    graphics::{RenderQueueKey, compose_render_queue_key}, 
    resources::{Material, MaterialHandle, Mesh, MeshHandle}
};
use anyhow::{Result, Context, Error};

pub struct MeshRenderingComponent {
    pub mesh_handle: Option<MeshHandle>,
    pub material_handle: Option<MaterialHandle>,
    pub(crate) render_queue_key: RenderQueueKey,
}

impl Component for MeshRenderingComponent {
    type Storage = ComponentStorage<MeshRenderingComponent>; 
}

impl MeshRenderingComponent {
    pub fn new(engine: &Engine, mesh_handle: &MeshHandle, material_handle: &MaterialHandle) -> Result<Self> {
        // [TOOD] Check if handles are valid

        let render_queue_key = compose_render_queue_key(engine, material_handle, mesh_handle)?;

        let mesh_rendering_component = Self { 
            mesh_handle: Some(mesh_handle.clone()),
            material_handle: Some(material_handle.clone()),
            render_queue_key,
        };
    
        Ok(mesh_rendering_component)
    }

    pub fn assign_material(&mut self, engine: &Engine, material_handle: &MaterialHandle) -> Result<()> {
        self.material_handle = Some(material_handle.clone());

        if self.mesh_handle.is_some() {
            self.render_queue_key = compose_render_queue_key(engine, material_handle, &self.mesh_handle.unwrap()).unwrap();
        }

        Ok(())
    }

    pub fn assign_mesh(&mut self, engine: &Engine, mesh_handle: &MeshHandle) -> Result<()> {
        self.mesh_handle = Some(mesh_handle.clone());

        if self.material_handle.is_some() {
            self.render_queue_key = compose_render_queue_key(engine, &self.material_handle.unwrap(), mesh_handle).unwrap();
        }


        Ok(())
    }
}

impl Default for MeshRenderingComponent {
    fn default() -> Self {
        Self { 
            mesh_handle: Option::None,
            material_handle: Option::None,
            render_queue_key: RenderQueueKey::default(),
        }
    }
}







