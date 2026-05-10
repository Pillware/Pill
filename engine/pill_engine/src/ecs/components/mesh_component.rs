use crate::{
    ecs::{Component, ComponentStorage},
    engine::Engine,
    resources::{Mesh, MeshHandle},
};
use pill_core::{get_type_name, PillStyle, PillTypeMapKey};
use pill_core::{ErrorContext, Result};

// --- Builder ---

pub struct MeshComponentBuilder {
    component: MeshComponent,
}

impl MeshComponentBuilder {
    pub fn default() -> Self {
        Self {
            component: MeshComponent::new(),
        }
    }

    pub fn mesh(mut self, mesh_handle: &MeshHandle) -> Self {
        self.component.mesh_handle = Some(*mesh_handle);
        self
    }

    pub fn build(self) -> MeshComponent {
        self.component
    }
}

// --- Mesh Component ---

#[readonly::make]
pub struct MeshComponent {
    #[readonly]
    pub mesh_handle: Option<MeshHandle>,
}

impl Default for MeshComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl MeshComponent {
    pub fn builder() -> MeshComponentBuilder {
        MeshComponentBuilder::default()
    }

    pub fn new() -> Self {
        Self { mesh_handle: None }
    }

    pub fn set_mesh(&mut self, mesh_handle: &MeshHandle) {
        self.mesh_handle = Some(*mesh_handle);
    }

    pub fn remove_mesh(&mut self) {
        self.mesh_handle = None;
    }

    pub(crate) fn set_mesh_handle(&mut self, mesh_handle: Option<MeshHandle>) {
        self.mesh_handle = mesh_handle;
    }
}

impl PillTypeMapKey for MeshComponent {
    type Storage = ComponentStorage<MeshComponent>;
}

impl Component for MeshComponent {
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        if let Some(handle) = &self.mesh_handle {
            engine.get_resource::<Mesh>(handle).context(format!(
                "Creating {} {} failed",
                "Component".general_object_style(),
                get_type_name::<Self>().specific_object_style()
            ))?;
        }
        Ok(())
    }
}
