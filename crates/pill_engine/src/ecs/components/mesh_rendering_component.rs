use crate::{
    engine::Engine,
    graphics::{ RenderQueueKey, compose_render_queue_key }, 
    resources::{ Material, MaterialHandle, Mesh, MeshHandle, ResourceManager },
    ecs::{ EntityHandle, ComponentStorage, Component, SceneHandle, DeferredUpdateComponentRequest, DeferredUpdateManagerPointer, DeferredUpdateComponent }, 
    config::DEFAULT_MATERIAL_HANDLE,
};

use cgmath::num_traits::Float;
use pill_core::{ PillTypeMap, PillTypeMapKey, PillStyle, get_type_name, PillSlotMapKey };

use anyhow::{ Result, Context, Error };


const DEFERRED_REQUEST_VARIANT_UPDATE_RENDER_QUEUE: usize = 0;
const DEFERRED_REQUEST_VARIANT_SET_MATERIAL: usize = 1;
const DEFERRED_REQUEST_VARIANT_SET_MESH: usize = 2;

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
    deferred_update_manager: Option<DeferredUpdateManagerPointer>,
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
            deferred_update_manager: None,
        }
    }

    pub fn set_material(&mut self, material_handle: &MaterialHandle) {
        self.material_handle = Some(material_handle.clone());
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_SET_MATERIAL);
    }

    pub fn set_mesh(&mut self, mesh_handle: &MeshHandle) {
        self.mesh_handle = Some(mesh_handle.clone());
        self.post_deferred_update_request(DEFERRED_REQUEST_VARIANT_SET_MESH);
    }

    pub fn remove_material(&mut self) {
        self.material_handle = None;
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

    fn post_deferred_update_request(&mut self, request_variant: usize) {
        if self.deferred_update_manager.is_some() {
            let entity_handle = self.entity_handle.expect("Critical: Cannot post deferred update request. No EntityHandle set in Component");
            let scene_handle = self.scene_handle.expect("Critical: Cannot post deferred update request. No SceneHandle set in Component");
            let request = DeferredUpdateComponentRequest::<MeshRenderingComponent>::new(entity_handle, scene_handle, request_variant);
            self.deferred_update_manager.as_mut().expect("Critical: No DeferredUpdateManager").post_update_request(request);
        }
    }
}

impl PillTypeMapKey for MeshRenderingComponent {
    type Storage = ComponentStorage<MeshRenderingComponent>; 
}

impl Component for MeshRenderingComponent {
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> {
        // This component is using DeferredUpdateSystem so keep DeferredUpdateManager
        let deferred_update_component = engine.get_global_component_mut::<DeferredUpdateComponent>().expect("Critical: No DeferredUpdateComponent");
        self.deferred_update_manager = Some(deferred_update_component.borrow_deferred_update_manager());

        // Check if material handle is valid
        if self.material_handle.is_some() {
            engine.get_resource::<Material>(&self.material_handle.unwrap())
                .context(format!("Creating {} {} failed", "Component".gobj_style(), get_type_name::<Self>().sobj_style()))?;
        }

        // Check if mesh handle is valid
        if self.mesh_handle.is_some() {
            engine.get_resource::<Mesh>(&self.mesh_handle.unwrap())
                .context(format!("Creating {} {} failed", "Component".gobj_style(), get_type_name::<Self>().sobj_style()))?;
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
            DEFERRED_REQUEST_VARIANT_SET_MATERIAL => 
            {
                // Check if material handle is valid
                engine.get_resource::<Material>(&self.material_handle.unwrap())
                    .context(format!("Setting {} {} failed", "Resource".gobj_style(), "Material".sobj_style()))?;
                
                self.update_render_queue_key(&engine.resource_manager)?;
            },
            DEFERRED_REQUEST_VARIANT_SET_MESH =>
            {
                // Check if mesh handle is valid
                engine.get_resource::<Mesh>(&self.mesh_handle.unwrap())
                    .context(format!("Setting {} {} failed", "Resource".gobj_style(), "Mesh".sobj_style()))?;

                self.update_render_queue_key(&engine.resource_manager)?;
            },
            DEFERRED_REQUEST_VARIANT_UPDATE_RENDER_QUEUE => 
            {
                // Update mesh rendering queue
                self.update_render_queue_key(&engine.resource_manager)?;
            },
            _ => 
            {
                panic!("Critical: Processing deferred update request with value {} in {} failed. Handling is not implemented", request, get_type_name::<Self>().sobj_style());
            }
        }

        Ok(()) 
    }
}