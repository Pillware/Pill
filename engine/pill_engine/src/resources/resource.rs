use crate::{ecs::DeferredOperation, engine::Engine, game::ResourceStorage};

use pill_core::{ PillTypeMap, PillTypeMapKey, PillSlotMapKey };

use std::path::PathBuf;
use anyhow::{Context, Result, Error};

// --- Resource ---

// PillTypeMapKey trait gives handle to the ResourceStorage
// PillSlotMapKey trait gives handle to the actual object in ResourceStorage

pub trait Resource : PillTypeMapKey {
    type Handle: PillSlotMapKey + Send; 

    // Required to implement
    fn get_name(&self) -> String;
    fn is_initialized(&self) -> bool;
    fn set_handle(&mut self, handle: Self::Handle);

    // Optional to implement
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> { Ok(()) } // Called when resource is added to the engine, before adding it to storage
    //fn pass_handle<H: PillSlotMapKey>(&mut self, self_handle: H) {} // Called right after resource is added to the engine, after adding it to storage
    //fn deferred_operation(&mut self, engine: &mut Engine, request: usize) -> Result<()> { Ok(()) } // Called by DeferredOperationSystem when request related to the resource is being processed
    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> { Ok(()) } // Called when resource is being removed from the engine
}

pub enum ResourceLoader {
    Path(PathBuf),
    Bytes(Box::<[u8]>),
}

pub struct ResourcesDir {
    pub path: PathBuf,
}

// --- Resource Request ---

pub struct ResourceDeferredOperation<T> 
    where T: Resource<Storage = ResourceStorage::<T>>
{
    resource_handle: T::Handle,
    operation: Box<dyn Fn(&mut T, &mut Engine) -> Result<()> + Send>,
}

impl<T> ResourceDeferredOperation<T> 
    where T: Resource<Storage = ResourceStorage::<T>>
{
    pub fn new(resource_handle: T::Handle, operation: Box<dyn Fn(&mut T, &mut Engine) -> Result<()> + Send>) -> Self {
        Self {
            resource_handle,
            operation,
        }
    } 
}

impl<T> DeferredOperation for ResourceDeferredOperation<T> 
    where T: Resource<Storage = ResourceStorage::<T>>
{
    fn process(&mut self, engine: &mut Engine) -> Result<()> {
        // Get resource slot (it may happen that this resource was deleted, if so then just continue)
        let resource_slot = match engine.resource_manager.get_resource_slot_mut::<T>(&self.resource_handle) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };
        
        // Take resource from slot
        let mut resource = resource_slot.take().expect("Critical: Resource is None");
       
        // Process
        (self.operation)(&mut resource, engine)?;

        // Get resource slot
        let resource_slot = match engine.resource_manager.get_resource_slot_mut::<T>(&self.resource_handle) {
            Ok(v) => v,
            Err(_) => return Ok(()),
        };

        // Put resource back to slot
        let _ = resource_slot.insert(resource);

        Ok(())
    }
}