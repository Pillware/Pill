#![allow(unused_assignments)]

use crate::{
    ecs::{
        Component, ComponentStorage, EntityHandle, GlobalComponent, GlobalComponentStorage,
        SceneHandle,
    },
    engine::Engine,
    resources::{Resource, ResourceStorage},
};

use pill_core::{get_type_name, PillSlotMapKey, PillStyle, PillTypeMapKey};

use anyhow::{Context, Result};
use std::{
    collections::VecDeque,
    marker::PhantomData,
    sync::{Arc, Mutex},
};

// --- Request Trait ---

pub trait DeferredUpdateRequest: Send {
    fn process(&mut self, engine: &mut Engine) -> Result<()>;
}

// --- Resource Request ---

pub struct DeferredUpdateResourceRequest<T>
where
    T: Resource<Storage = ResourceStorage<T>>,
{
    resource_handle: T::Handle,
    request_variant: usize,
}

impl<T> DeferredUpdateResourceRequest<T>
where
    T: Resource<Storage = ResourceStorage<T>>,
{
    pub fn new(resource_handle: T::Handle, request_variant: usize) -> Self {
        Self {
            resource_handle,
            request_variant,
        }
    }
}

impl<T> DeferredUpdateRequest for DeferredUpdateResourceRequest<T>
where
    T: Resource<Storage = ResourceStorage<T>>,
{
    fn process(&mut self, engine: &mut Engine) -> Result<()> {
        // Get resource slot (it may happen that this resource was deleted, if so then just continue)
        let resource_slot = match engine
            .resource_manager
            .get_resource_slot_mut::<T>(&self.resource_handle)
        {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };

        // Take resource from slot
        let mut resource = resource_slot.take().expect("Critical: Resource is None");

        // Process
        resource
            .deferred_update(engine, self.request_variant)
            .context(format!(
                "Deferred update of {} {} {} failed",
                "Resource".general_object_style(),
                get_type_name::<T>().specific_object_style(),
                resource.get_name().name_style()
            ))?;

        // Get resource slot
        let resource_slot = engine
            .resource_manager
            .get_resource_slot_mut::<T>(&self.resource_handle)
            .expect("Critical: Resource not registered");

        // Put resource back to slot
        let _ = resource_slot.insert(resource);

        Ok(())
    }
}

// --- Component Request ---

pub struct DeferredUpdateComponentRequest<T>
where
    T: Component<Storage = ComponentStorage<T>>,
{
    entity_handle: EntityHandle,
    scene_handle: SceneHandle,
    request_variant: usize,
    phantom: PhantomData<T>, // 👻
}

impl<T> DeferredUpdateComponentRequest<T>
where
    T: Component<Storage = ComponentStorage<T>>,
{
    pub fn new(
        entity_handle: EntityHandle,
        scene_handle: SceneHandle,
        request_variant: usize,
    ) -> Self {
        Self {
            entity_handle,
            scene_handle,
            request_variant,
            phantom: PhantomData,
        }
    }
}

impl<T> DeferredUpdateRequest for DeferredUpdateComponentRequest<T>
where
    T: Component<Storage = ComponentStorage<T>>,
{
    fn process(&mut self, engine: &mut Engine) -> Result<()> {
        let mut component = Option::<T>::None;

        {
            // Get scene
            let scene = engine
                .scene_manager
                .get_scene_mut(self.scene_handle)
                .unwrap();

            // Get component storage
            let component_storage = scene
                .get_component_storage_mut::<T>()
                .expect("Critical: Component not registered");

            // Get component slot
            let component_slot = component_storage
                .data
                .get_mut(self.entity_handle.data().index as usize)
                .unwrap();

            // Take component from slot
            component = Some(component_slot.take().expect("Critical: Component is None"));
        }

        // Process
        component
            .as_mut()
            .unwrap()
            .deferred_update(engine, self.request_variant)
            .context(format!(
                "Deferred update of {} {} failed",
                "Component".general_object_style(),
                get_type_name::<T>().specific_object_style()
            ))?;

        {
            // Get scene
            let scene = engine
                .scene_manager
                .get_scene_mut(self.scene_handle)
                .unwrap();

            // Get component storage
            let component_storage = scene
                .get_component_storage_mut::<T>()
                .expect("Critical: Component not registered");

            // Get component slot
            let component_slot = component_storage
                .data
                .get_mut(self.entity_handle.data().index as usize)
                .unwrap();

            // Put component back to slot
            let _ = component_slot.insert(component.take().unwrap());
        }

        Ok(())
    }
}

// --- Global Component Request ---

pub struct DeferredUpdateGlobalComponentRequest<T>
where
    T: GlobalComponent<Storage = GlobalComponentStorage<T>>,
{
    request_variant: usize,
    phantom: PhantomData<T>, // 👻
}

impl<T> DeferredUpdateGlobalComponentRequest<T>
where
    T: GlobalComponent<Storage = GlobalComponentStorage<T>>,
{
    pub fn new(request_variant: usize) -> Self {
        Self {
            request_variant,
            phantom: PhantomData,
        }
    }
}

impl<T> DeferredUpdateRequest for DeferredUpdateGlobalComponentRequest<T>
where
    T: GlobalComponent<Storage = GlobalComponentStorage<T>>,
{
    fn process(&mut self, engine: &mut Engine) -> Result<()> {
        let mut component = Option::<T>::None;

        {
            // Get component storage
            let component_storage = engine
                .global_components
                .get_mut::<T>()
                .expect("Critical: Component not registered");

            // Get component slot
            let component_slot = &mut component_storage.data;

            // Take component from slot
            component = Some(component_slot.take().expect("Critical: Component is None"));
        }

        // Process
        component
            .as_mut()
            .unwrap()
            .deferred_update(engine, self.request_variant)
            .context(format!(
                "Deferred update of {} {} failed",
                "GlobalComponent".general_object_style(),
                get_type_name::<T>().specific_object_style()
            ))?;

        {
            // Get component storage
            let component_storage = engine
                .global_components
                .get_mut::<T>()
                .expect("Critical: Component not registered");

            // Get component slot
            let component_slot = &mut component_storage.data;

            // Put component back to slot
            let _ = component_slot.insert(component.take().unwrap());
        }

        Ok(())
    }
}

// --- Manager ---

pub struct DeferredUpdateManager {
    pub request_queue: Option<VecDeque<Box<dyn DeferredUpdateRequest>>>,
}

impl DeferredUpdateManager {
    pub fn new() -> Self {
        Self {
            request_queue: Some(VecDeque::<Box<dyn DeferredUpdateRequest>>::new()),
        }
    }
}

// --- Manager pointer ---

pub struct DeferredUpdateManagerPointer(pub(crate) Arc<Mutex<DeferredUpdateManager>>);

impl DeferredUpdateManagerPointer {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(DeferredUpdateManager::new())))
    }

    pub(crate) fn clone(&mut self) -> Self {
        Self(self.0.clone())
    }

    pub fn post_update_request(&mut self, request: impl DeferredUpdateRequest + 'static) {
        let mut deferred_update_manager = self.0.lock().expect("Critical: Mutex is blocked");
        let request_queue = deferred_update_manager
            .request_queue
            .as_mut()
            .expect("Critical: Queue in None");
        request_queue.push_back(Box::new(request));
    }
}

// --- Component ---

pub struct DeferredUpdateComponent {
    pub(crate) manager: DeferredUpdateManagerPointer,
}

impl DeferredUpdateComponent {
    pub fn new() -> Self {
        Self {
            manager: DeferredUpdateManagerPointer::new(),
        }
    }

    pub(crate) fn borrow_deferred_update_manager(&mut self) -> DeferredUpdateManagerPointer {
        self.manager.clone()
    }
}

impl PillTypeMapKey for DeferredUpdateComponent {
    type Storage = GlobalComponentStorage<DeferredUpdateComponent>;
}

impl GlobalComponent for DeferredUpdateComponent {}
