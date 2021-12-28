pub use crate::ecs::*;
use crate::{
    internal::Engine, 
    graphics::{RenderQueueKey, compose_render_queue_key}, 
    resources::{Material, MaterialHandle, Mesh, MeshHandle, ResourceManager}
};
use anyhow::{Result, Context, Error};

pub struct MeshRenderingComponent {
    pub mesh_handle: Option<MeshHandle>,
    pub material_handle: Option<MaterialHandle>,
    pub(crate) render_queue_key: Option<RenderQueueKey>, 
}

impl Component for MeshRenderingComponent {
    type Storage = ComponentStorage<MeshRenderingComponent>; 
}

impl MeshRenderingComponent {
    pub fn new(engine: &Engine, mesh_handle: &MeshHandle, material_handle: &MaterialHandle) -> Result<Self> {

        let mut mesh_rendering_component = Self { 
            mesh_handle: Some(mesh_handle.clone()),
            material_handle: Some(material_handle.clone()),
            render_queue_key: None,
        };

        // [TOOD] Check if handles are valid
        mesh_rendering_component.update_render_queue_key(&engine.resource_manager)?;
    
        Ok(mesh_rendering_component)
    }

    pub fn set_material(&mut self, engine: &Engine, material_handle: &MaterialHandle) -> Result<()> {
        self.material_handle = Some(material_handle.clone());
        self.update_render_queue_key(&engine.resource_manager)?;

        Ok(())
    }

    pub fn set_mesh(&mut self, engine: &Engine, mesh_handle: &MeshHandle) -> Result<()> {
        self.mesh_handle = Some(mesh_handle.clone());
        self.update_render_queue_key(&engine.resource_manager)?;

        Ok(())
    }

    pub fn remove_material(&mut self, engine: &Engine) -> Result<()> {
        self.material_handle = None;
        self.update_render_queue_key(&engine.resource_manager)?;

        Ok(())
    }

    pub fn remove_mesh(&mut self, engine: &Engine) -> Result<()> {
        self.mesh_handle = None;
        self.update_render_queue_key(&engine.resource_manager)?;

        Ok(())
    }

    pub(crate) fn update_render_queue_key(&mut self, resource_manager: &ResourceManager) -> Result<()> {
        if self.material_handle.is_some() && self.mesh_handle.is_some() {
            if let Ok(render_queue_key) = compose_render_queue_key(
                resource_manager, 
                &self.material_handle.unwrap(), 
                &self.mesh_handle.unwrap()) 
            {
                self.render_queue_key = Some(render_queue_key);
            }
            else
            {
                self.render_queue_key = None;
            }
        }
        else
        {
            self.render_queue_key = None;
        }

        Ok(())
    }
}

impl Default for MeshRenderingComponent {
    fn default() -> Self {
        Self { 
            mesh_handle: Option::None,
            material_handle: Option::None,
            render_queue_key: Option::None,
        }
    }
}
