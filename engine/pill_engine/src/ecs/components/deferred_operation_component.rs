use crate::{
    engine::Engine,
    resources::{ MeshHandle, Material, Resource, ResourceStorage }, 
    ecs::{ Component, EntityHandle, ComponentStorage, SceneHandle, GlobalComponentStorage, GlobalComponent }
};
use pill_core::{ PillSlotMapKey, PillStyle, get_type_name, PillTypeMapKey };
use std::{
    sync::{Arc, Mutex}, 
    collections::VecDeque, 
    marker::PhantomData
};
use anyhow::{Result, Context, Error};


// --- Trait ---

pub trait DeferredOperation: Send {
    fn process(&mut self, engine: &mut Engine) -> Result<()>;
}

// --- Manager ---

pub struct DeferredOperationManager {
    pub operation_queue: Option<VecDeque<Box<dyn DeferredOperation>>>,
}

impl DeferredOperationManager {
    pub fn new() -> Self {
        Self {
            operation_queue: Some(VecDeque::<Box<dyn DeferredOperation>>::new()),
        }
    }
}

// --- Manager pointer ---

pub struct DeferredOperationManagerPointer(pub(crate) Arc<Mutex<DeferredOperationManager>>);

impl DeferredOperationManagerPointer {
    pub fn new() -> Self {
        Self {
            0: Arc::new(Mutex::new(DeferredOperationManager::new())),
        }
    }

    pub(crate) fn clone(&mut self) -> Self {
        Self { 
            0: self.0.clone(),
        }
    }

    pub fn schedule_deferred_operation(&mut self, deferred_operation: impl DeferredOperation + 'static) {
        let mut deferred_operation_manager = self.0.lock().expect("Critical: Mutex is blocked");
        let operation_queue = deferred_operation_manager.operation_queue.as_mut().expect("Critical: Queue in None");
        operation_queue.push_back(Box::new(deferred_operation));
    }
}

// --- Component ---

pub struct DeferredOperationComponent {
    pub(crate) manager: DeferredOperationManagerPointer,

    self_scene_handle: Option<SceneHandle>,
}

impl DeferredOperationComponent {
    pub fn new() -> Self {  
        Self { 
            manager: DeferredOperationManagerPointer::new(),
            self_scene_handle: None,
        }
    }

    pub(crate) fn borrow_deferred_operation_manager(&mut self) -> DeferredOperationManagerPointer
    {
        self.manager.clone()
    }
}


impl PillTypeMapKey for DeferredOperationComponent {
    type Storage = GlobalComponentStorage<DeferredOperationComponent>; 
}

impl GlobalComponent for DeferredOperationComponent {

}