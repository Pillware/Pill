

pub use crate::ecs::*;
use crate::{
    game::Engine, 
    graphics::{RenderQueueKey, create_render_queue_key}, 
    resources::{Material, MaterialHandle, Mesh, MeshHandle}
};
use anyhow::{Result, Context, Error};

pub struct MeshRenderingComponent {
    mesh: Option<MeshHandle>,
    material: Option<MaterialHandle>,
    render_queue_key: RenderQueueKey,
}

impl Component for MeshRenderingComponent {
    type Storage = ComponentStorage<MeshRenderingComponent>; 
}

impl MeshRenderingComponent {
    pub fn new(engine: &Engine, mesh_handle: &MeshHandle, material_handle: &MaterialHandle) -> Result<Self> {
        let render_queue_key = create_render_queue_key(engine, material_handle, mesh_handle)?;

        let mesh_rendering_component = Self { 
            mesh: Some(mesh_handle.clone()),
            material: Some(material_handle.clone()),
            render_queue_key,
        };
    
        Ok(mesh_rendering_component)
    }

}



// impl Default for MeshRenderingComponent {
//     fn default() -> Self {
//         Self { 
//             resource_id: None,
//         }
//     }
// }







