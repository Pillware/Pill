use crate::{
    config::DEFAULT_MATERIAL_HANDLE,
    ecs::{
        Component, ComponentStorage, DeferredUpdateComponent, DeferredUpdateComponentRequest,
        DeferredUpdateManagerPointer, EntityHandle, SceneHandle,
    },
    engine::Engine,
    graphics::{compose_pbr_render_queue_key, compose_render_queue_key, RenderQueueKey},
    resources::{
        Material, MaterialHandle, Mesh, MeshHandle, PBRMaterial, PBRMaterialHandle, ResourceManager,
    },
};

use pill_core::{get_type_name, PillStyle, PillTypeMapKey};

use pill_core::{ErrorContext, Result};

const DEFERRED_REQUEST_VARIANT_UPDATE_RENDER_QUEUE: usize = 0;
const DEFERRED_REQUEST_VARIANT_SET_MATERIAL: usize = 1;
const DEFERRED_REQUEST_VARIANT_SET_MESH: usize = 2;
const DEFERRED_REQUEST_VARIANT_SET_PBR_MATERIAL: usize = 3;

// --- Builder ---

pub struct PbrRenderableComponentBuilder {
    component: PbrRenderableComponent,
}

impl PbrRenderableComponentBuilder {
    pub fn default() -> Self {
        Self {
            component: PbrRenderableComponent::new(),
        }
    }

    pub fn mesh(mut self, mesh_handle: &MeshHandle) -> Self {
        self.component.mesh_handle = Some(*mesh_handle);
        self
    }

    pub fn material(mut self, material_handle: &MaterialHandle) -> Self {
        self.component.material_handle = Some(*material_handle);
        self
    }

    pub fn pbr_material(mut self, material_handle: &PBRMaterialHandle) -> Self {
        self.component.pbr_material_handle = Some(*material_handle);
        self
    }

    pub fn build(self) -> PbrRenderableComponent {
        self.component
    }
}

// --- PBR Renderable Component ---

#[readonly::make]
pub struct PbrRenderableComponent {
    #[readonly]
    pub mesh_handle: Option<MeshHandle>,
    #[readonly]
    pub material_handle: Option<MaterialHandle>,
    #[readonly]
    pub pbr_material_handle: Option<PBRMaterialHandle>,
    pub(crate) render_queue_key: Option<RenderQueueKey>,

    entity_handle: Option<EntityHandle>,
    scene_handle: Option<SceneHandle>,
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
}

impl Default for PbrRenderableComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl PbrRenderableComponent {
    pub fn builder() -> PbrRenderableComponentBuilder {
        PbrRenderableComponentBuilder::default()
    }

    pub fn new() -> Self {
        Self {
            mesh_handle: None,
            material_handle: None,
            pbr_material_handle: None,
            render_queue_key: None,
            entity_handle: None,
            scene_handle: None,
            deferred_update_manager: None,
        }
    }

    pub fn set_material(&mut self, material_handle: &MaterialHandle) {
        self.material_handle = Some(*material_handle);
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_SET_MATERIAL);
    }

    pub fn set_pbr_material(&mut self, material_handle: &PBRMaterialHandle) {
        self.pbr_material_handle = Some(*material_handle);
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_SET_PBR_MATERIAL);
    }

    pub fn set_mesh(&mut self, mesh_handle: &MeshHandle) {
        self.mesh_handle = Some(*mesh_handle);
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_SET_MESH);
    }

    pub fn remove_material(&mut self) {
        self.material_handle = None;
        self.pbr_material_handle = None;
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_UPDATE_RENDER_QUEUE);
    }

    pub fn remove_mesh(&mut self) {
        self.mesh_handle = None;
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_UPDATE_RENDER_QUEUE);
    }

    pub(crate) fn set_material_handle(&mut self, material_handle: Option<MaterialHandle>) {
        self.material_handle = material_handle;
    }

    pub(crate) fn set_mesh_handle(&mut self, mesh_handle: Option<MeshHandle>) {
        self.mesh_handle = mesh_handle;
    }

    pub(crate) fn update_render_queue_key(
        &mut self,
        resource_manager: &ResourceManager,
    ) -> Result<()> {
        if let Some(mesh_handle) = &self.mesh_handle {
            // PBR path takes priority; legacy Material path is the fallback
            let result = if let Some(pbr_handle) = self.pbr_material_handle {
                compose_pbr_render_queue_key(resource_manager, pbr_handle, mesh_handle)
            } else {
                // Use default material if no material is set
                let material_handle = self.material_handle.unwrap_or(DEFAULT_MATERIAL_HANDLE);
                compose_render_queue_key(resource_manager, &material_handle, mesh_handle)
            };

            if let Ok(render_queue_key) = result {
                self.render_queue_key = Some(render_queue_key);
            } else {
                self.render_queue_key = None;
            }
        } else {
            self.render_queue_key = None;
        }

        Ok(())
    }

    fn post_deferred_update_request(&mut self, request_variant: usize) {
        if let Some(manager) = self.deferred_update_manager.as_mut() {
            let entity_handle = self.entity_handle.expect(
                "Critical: Cannot post deferred update request. No EntityHandle set in Component",
            );
            let scene_handle = self.scene_handle.expect(
                "Critical: Cannot post deferred update request. No SceneHandle set in Component",
            );
            let request = DeferredUpdateComponentRequest::<PbrRenderableComponent>::new(
                entity_handle,
                scene_handle,
                request_variant,
            );
            manager.post_update_request(request);
        }
    }
}

impl PillTypeMapKey for PbrRenderableComponent {
    type Storage = ComponentStorage<PbrRenderableComponent>;
}

impl Component for PbrRenderableComponent {
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // This component is using DeferredUpdateSystem so keep DeferredUpdateManager
        let deferred_update_component = engine
            .get_global_component_mut::<DeferredUpdateComponent>()
            .expect("Critical: No DeferredUpdateComponent");
        self.deferred_update_manager =
            Some(deferred_update_component.borrow_deferred_update_manager());

        // Check if material handle is valid
        if let Some(handle) = &self.material_handle {
            engine.get_resource::<Material>(handle).context(format!(
                "Creating {} {} failed",
                "Component".general_object_style(),
                get_type_name::<Self>().specific_object_style()
            ))?;
        }

        // Check if PBR material handle is valid
        if let Some(handle) = &self.pbr_material_handle {
            engine.get_resource::<PBRMaterial>(handle).context(format!(
                "Creating {} {} failed",
                "Component".general_object_style(),
                get_type_name::<Self>().specific_object_style()
            ))?;
        }

        // Check if mesh handle is valid
        if let Some(handle) = &self.mesh_handle {
            engine.get_resource::<Mesh>(handle).context(format!(
                "Creating {} {} failed",
                "Component".general_object_style(),
                get_type_name::<Self>().specific_object_style()
            ))?;
        }

        // Update mesh rendering queue
        self.update_render_queue_key(&engine.resource_manager)?;

        Ok(())
    }

    fn pass_handles(&mut self, self_scene_handle: SceneHandle, self_entity_handle: EntityHandle) {
        self.scene_handle = Some(self_scene_handle);
        self.entity_handle = Some(self_entity_handle);
    }

    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> {
        match request {
            DEFERRED_REQUEST_VARIANT_SET_MATERIAL => {
                engine
                    .get_resource::<Material>(&self.material_handle.unwrap())
                    .context(format!(
                        "Setting {} {} failed",
                        "Resource".general_object_style(),
                        "Material".specific_object_style()
                    ))?;

                self.update_render_queue_key(&engine.resource_manager)?;
            }
            DEFERRED_REQUEST_VARIANT_SET_PBR_MATERIAL => {
                engine
                    .get_resource::<PBRMaterial>(&self.pbr_material_handle.unwrap())
                    .context(format!(
                        "Setting {} {} failed",
                        "Resource".general_object_style(),
                        "PBRMaterial".specific_object_style()
                    ))?;

                self.update_render_queue_key(&engine.resource_manager)?;
            }
            DEFERRED_REQUEST_VARIANT_SET_MESH => {
                engine
                    .get_resource::<Mesh>(&self.mesh_handle.unwrap())
                    .context(format!(
                        "Setting {} {} failed",
                        "Resource".general_object_style(),
                        "Mesh".specific_object_style()
                    ))?;

                self.update_render_queue_key(&engine.resource_manager)?;
            }
            DEFERRED_REQUEST_VARIANT_UPDATE_RENDER_QUEUE => {
                self.update_render_queue_key(&engine.resource_manager)?;
            }
            _ => {
                panic!("Critical: Processing deferred update request with value {} in {} failed. Handling is not implemented", request, get_type_name::<Self>().specific_object_style());
            }
        }

        Ok(())
    }
}
