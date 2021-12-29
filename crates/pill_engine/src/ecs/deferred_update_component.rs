use std::{sync::{Arc, Mutex}, collections::VecDeque, marker::PhantomData};

use pill_core::PillSlotMapKey;

use crate::{resources::{Resource, ResourceStorage}, game::{MeshHandle, Engine, Material}};

use super::{Component, EntityHandle, ComponentStorage};

// --- Resource Request

pub struct DeferredUpdateResourceRequest<T> 
    where T: Resource<Value = ResourceStorage::<T>>
{
    resource_handle: T::Handle,
    request_variant: u32,
}

impl<T> DeferredUpdateResourceRequest<T> 
    where T: Resource<Value = ResourceStorage::<T>>
{
    pub fn new(resource_handle: T::Handle, request_variant: u32) -> Self {
        Self {
            resource_handle,
            request_variant,
        }
    } 

    fn process(&mut self, engine: &mut Engine) {
        // Get resource slot
        let resource_slot = engine.resource_manager.get_resource_slot_mut::<T>(&self.resource_handle).expect("Critical: Resource not registered");
        // Take resource from slot
        let mut resource = resource_slot.take().expect("Critical: Resource is None");
        // Process
        resource.deferred_update(engine, self.request_variant);
        // Get resource slot
        let resource_slot = engine.resource_manager.get_resource_slot_mut::<T>(&self.resource_handle).expect("Critical: Resource not registered");
        // Put resource back to slot
        resource_slot.insert(resource);
    }
}


// --- Component Request

pub struct DeferredUpdateComponentRequest<T> 
    where T: Component<Storage = ComponentStorage<T>>
{
    entity_handle: EntityHandle,
    request_variant: u32,
    phantom: PhantomData<T>, // 👻
}

impl<T> DeferredUpdateComponentRequest<T> 
    where T: Component<Storage = ComponentStorage<T>>
{
    pub fn new(entity_handle: EntityHandle, request_variant: u32) -> Self {
        Self {
            entity_handle,
            request_variant,
            phantom: PhantomData,
        }
    } 
    
    fn process(&mut self, engine: &mut Engine) {
        // // Get component slot
        // let component_slot = engine.resource_manager.get_component_slot_mut::<T>(&self.entity_handle).except("Critical: Component not registered");
        // // Take component from slot
        // let mut component = component_slot.take().expect("Critical: Component is None");
        // // Process
        // component.deferred_update(engine, self.request_variant);
        // // Get component slot
        // let component_slot = engine.resource_manager.get_component_slot_mut::<T>(&self.entity_handle).except("Critical: Component not registered");
        // // Put resource back to slot
        // component_slot.insert(component);
    }
}


pub struct DeferredUpdateManager {
    //pub requests_requests: 
    //pub resource_requests: VecDeque<DeferredUpdateResourceRequest<dyn DeferredUpdateUser + ?Sized>>,
   // pub resource_requests: VecDeque<DeferredUpdateResourceRequest<T>>,
    //pub component_requests: VecDeque<DeferredUpdateComponentRequest<T>>,
}

impl DeferredUpdateManager {
    pub fn new() -> Self {
        Self {

        }
    }
}

pub struct DeferredUpdateManagerPointer(Arc<Mutex<DeferredUpdateManager>>);

impl DeferredUpdateManagerPointer {
    pub fn new() -> Self {  
        Self { 
            0: Arc::new(Mutex::new(DeferredUpdateManager::new())),
        }
    }

    pub(crate) fn clone(&mut self) -> Self {
        Self { 
            0: self.0.clone(),
        }
    }

    pub fn post_resource_update_request(&mut self, handle: impl PillSlotMapKey, request: u32) {
        //let deferred_update_manager = self.0.lock().unwrap();

        let x = MeshHandle::new(1, std::num::NonZeroU32::new(1).unwrap());
        //let request = Request::<crate::game::Mesh> { caller_handle: x, request_type: DeferredUpdateRequestType::MaterialOrder };


       //deferred_update_manager.
    }
}


pub struct DeferredUpdateGlobalComponent {
    manager: DeferredUpdateManagerPointer,
}





// impl GlobalComponent for TransformComponent {
//     type Storage = DefferedUpdateGlobalComponent; 
// }

impl DeferredUpdateGlobalComponent {
    pub fn new() -> Self {  
        Self { 
            manager: DeferredUpdateManagerPointer::new(),
        }
    }

    pub(crate) fn borrow_deferred_update_manager(&mut self) -> DeferredUpdateManagerPointer
    {
        self.manager.clone()
    }
}