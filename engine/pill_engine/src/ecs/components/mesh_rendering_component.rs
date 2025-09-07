use crate::{
    config::DEFAULT_MATERIAL_HANDLE, 
    ecs::{ components::component::ComponentDeferredOperation, Component, ComponentStorage, DeferredOperationComponent, DeferredOperationManagerPointer, EntityHandle, SceneHandle }, 
    engine::Engine, 
    graphics::{ compose_render_queue_key, RenderQueueKey }, 
    resources::{ Material, MaterialHandle, Mesh, MeshHandle, ResourceManager }
};
use cgmath::num_traits::Float;
use pill_core::{ PillTypeMap, PillTypeMapKey, PillStyle, get_type_name, PillSlotMapKey };
use anyhow::{ Result, Context, Error };

// --- Builder ---

pub struct MeshRenderingComponentBuilder {
    component: MeshRenderingComponent,
}

impl MeshRenderingComponentBuilder {
    pub fn default() -> Self {
        Self {
            component: MeshRenderingComponent::new(),
        }
    }
    
    pub fn mesh(mut self, mesh_handle: &MeshHandle) -> Self {
        self.component.mesh_handle = Some(mesh_handle.clone());
        self
    }

    pub fn material(mut self, material_handle: &MaterialHandle) -> Self {
        self.component.material_handle = Some(material_handle.clone());
        self
    }

    pub fn build(self) -> MeshRenderingComponent {
        self.component
    }
}

// --- Mesh Rendering Component ---

#[readonly::make]
pub struct MeshRenderingComponent {
    #[readonly]
    pub mesh_handle: Option<MeshHandle>,
    #[readonly]
    pub material_handle: Option<MaterialHandle>,
    pub(crate) render_queue_key: Option<RenderQueueKey>, 

    entity_handle: Option<EntityHandle>,
    scene_handle: Option<SceneHandle>,
    deferred_operation_manager: Option<DeferredOperationManagerPointer>,
}

impl MeshRenderingComponent {
    pub fn builder() -> MeshRenderingComponentBuilder {
        MeshRenderingComponentBuilder::default()
    }

    pub fn new() -> Self {
        Self { 
            mesh_handle: None,
            material_handle: None,
            render_queue_key: None,
            entity_handle: None,
            scene_handle: None,
            deferred_operation_manager: None,
        }
    }

    pub fn set_material(&mut self, material_handle: &MaterialHandle) {
        self.material_handle = Some(material_handle.clone());

        if self.is_initialized() {
            self.schedule_deferred_operation(Box::new(|self_mesh_rendering_component: &mut MeshRenderingComponent, engine: &mut Engine| {
                // Check if material handle is valid
                engine.get_resource::<Material>(&self_mesh_rendering_component.material_handle.unwrap())
                    .context(format!("Setting {} {} failed", "Resource".general_object_style(), "Material".specific_object_style()))?;
                
                self_mesh_rendering_component.update_render_queue_key(&engine.resource_manager)?;

                Ok(())
            }));
        }
    }

     pub fn reset_material(&mut self) {
        self.material_handle = None;

        if self.is_initialized() {
            self.schedule_deferred_operation(Box::new(|self_mesh_rendering_component: &mut MeshRenderingComponent, engine: &mut Engine| {
                self_mesh_rendering_component.update_render_queue_key(&engine.resource_manager)?;

                Ok(())
            }));
        }
    }

    pub fn set_mesh(&mut self, mesh_handle: &MeshHandle) {
        self.mesh_handle = Some(mesh_handle.clone());

        if self.is_initialized() {
            self.schedule_deferred_operation(Box::new(|self_mesh_rendering_component: &mut MeshRenderingComponent, engine: &mut Engine| {
                // Check if mesh handle is valid
                engine.get_resource::<Mesh>(&self_mesh_rendering_component.mesh_handle.unwrap())
                    .context(format!("Setting {} {} failed", "Resource".general_object_style(), "Mesh".specific_object_style()))?;

                self_mesh_rendering_component.update_render_queue_key(&engine.resource_manager)?;

                Ok(())
            }));
        }
    }

    pub fn reset_mesh(&mut self) {
        self.mesh_handle = None;

        if self.is_initialized() {
            self.schedule_deferred_operation(Box::new(|self_mesh_rendering_component: &mut MeshRenderingComponent, engine: &mut Engine| {
                self_mesh_rendering_component.update_render_queue_key(&engine.resource_manager)?;

                Ok(())
            }));
        }
    }

    // pub(crate) fn set_material_handle(&mut self, material_handle: Option<MaterialHandle>) {
    //     self.material_handle = material_handle;
    // }

    // pub(crate) fn set_mesh_handle(&mut self, mesh_handle: Option<MeshHandle>) {
    //     self.mesh_handle = mesh_handle;
    // }

    pub(crate) fn update_render_queue_key(&mut self, resource_manager: &ResourceManager) -> Result<()> {
        if self.mesh_handle.is_some() {
            // Use default material if no material is set
            let material_handle = match self.material_handle {
                Some(v) => v,
                None => DEFAULT_MATERIAL_HANDLE,
            };

            // Compose render queue key and set it
            if let Ok(render_queue_key) = compose_render_queue_key(resource_manager, &material_handle, &self.mesh_handle.unwrap()) 
            {
                self.render_queue_key = Some(render_queue_key);
            }
            else {
                self.render_queue_key = None;
            }
        }
        else
        {
            self.render_queue_key = None;
        }

        Ok(())
    }

    fn schedule_deferred_operation(&mut self, operation: Box<dyn Fn(&mut MeshRenderingComponent, &mut Engine) -> Result<()> + Send>) {
        let entity_handle = self.entity_handle.expect("Critical: Cannot post deferred update request. No EntityHandle set in Component");
        let scene_handle = self.scene_handle.expect("Critical: Cannot post deferred update request. No SceneHandle set in Component");
        let operation_to_schedule = ComponentDeferredOperation::<MeshRenderingComponent>::new(entity_handle, scene_handle, operation);
        self.deferred_operation_manager.as_mut().expect("Critical: No DeferredOperationManager").schedule_deferred_operation(operation_to_schedule);
    }

    fn is_initialized(&self) -> bool {
        self.entity_handle.is_some() && self.scene_handle.is_some() && self.deferred_operation_manager.is_some()
    }
}

impl PillTypeMapKey for MeshRenderingComponent {
    type Storage = ComponentStorage<MeshRenderingComponent>; 
}

impl Component for MeshRenderingComponent {
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // This component is using DeferredOperationSystem so keep DeferredOperationComponent
        let deferred_operation_component = engine.get_global_component_mut::<DeferredOperationComponent>().expect("Critical: No DeferredOperationComponent");
        self.deferred_operation_manager = Some(deferred_operation_component.borrow_deferred_operation_manager());

        // Check if material handle is valid
        if self.material_handle.is_some() {
            engine.get_resource::<Material>(&self.material_handle.unwrap())
                .context(format!("Creating {} {} failed", "Component".general_object_style(), get_type_name::<Self>().specific_object_style()))?;
        }

        // Check if mesh handle is valid
        if self.mesh_handle.is_some() {
            engine.get_resource::<Mesh>(&self.mesh_handle.unwrap())
                .context(format!("Creating {} {} failed", "Component".general_object_style(), get_type_name::<Self>().specific_object_style()))?;
        }

        // Update mesh rendering queue
        self.update_render_queue_key(&engine.resource_manager)?;

        Ok(())
    }

    fn set_handles(&mut self, self_scene_handle: SceneHandle, self_entity_handle: EntityHandle) {
        self.scene_handle = Some(self_scene_handle);
        self.entity_handle = Some(self_entity_handle);
    }
}