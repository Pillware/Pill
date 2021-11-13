pub use crate::ecs::*;

pub struct MeshRenderingComponent {
    resource_id: Option<usize>,
}

impl Component for MeshRenderingComponent {
    type Storage = ComponentStorage<MeshRenderingComponent>; 
}

impl Default for MeshRenderingComponent {
    fn default() -> Self {
        Self { 
            resource_id: None,
        }
    }
}







