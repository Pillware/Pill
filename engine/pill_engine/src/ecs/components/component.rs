use crate::{ 
    ecs::{ ComponentStorage, DeferredOperation, EntityHandle, GlobalComponentStorage, SceneHandle }, engine::Engine, resources::Resource
};

use pill_core::{ get_type_name, PillSlotMapKey, PillStyle, PillTypeMap, PillTypeMapKey };

use std::{path::PathBuf, marker::PhantomData};
use anyhow::{ Context, Result, Error };
use dyn_clone::DynClone;


// --- Component ---

// TypeMapKey trait gives handle to the ResourceStorage
// PillSlotMapKey trait gives handle to the actual object in ResourceStorage

pub trait Component : PillTypeMapKey + Send {  
    // Required to implement
    //fn is_initialized(&self) -> bool;
    fn set_handles(&mut self, self_scene_handle: SceneHandle, self_entity_handle: EntityHandle) { }

    // Optional to implement
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> { Ok(()) } // Called when component is added to the engine, before adding it to storage
    //fn deferred_operation(&mut self, engine: &mut Engine, request: usize) -> Result<()> { Ok(()) } // Called by DeferredOperationSystem when request related to the component is being processed
    fn destroy(&mut self, engine: &mut Engine, self_scene_handle: SceneHandle, self_entity_handle: EntityHandle) -> Result<()> { Ok(()) } // Called when component is being removed from the engine
}

// --- Global Component ---

pub trait GlobalComponent : PillTypeMapKey + Send {  
    //fn is_initialized(&self) -> bool;
    //fn set_handle(&mut self, self_scene_handle: SceneHandle);
    
    // Optional to implement
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> { Ok(()) } // Called when component is added to the engine, before adding it to storage
    //fn deferred_operation(&mut self, engine: &mut Engine, request: usize) -> Result<()> { Ok(()) } // Called by DeferredOperationSystem when request related to the component is being processed
    fn destroy(&mut self, engine: &mut Engine) -> Result<()> { Ok(()) } // Called when component is being removed from the engine
}

// --- Component Destroyers ---

// Approach that makes it possible to delete components by iterating over typemap of component storages and not knowing the types of the components
// Use DynClone to be able to clone Boxed component destroyers
pub trait ComponentDestroyer: DynClone  {
    fn destroy(&mut self, engine: &mut Engine, scene_handle: SceneHandle, entity_handle: EntityHandle) -> Result<()>;
}

dyn_clone::clone_trait_object!(ComponentDestroyer);

pub struct ConcreteComponentDestroyer<T> {
    component_type: PhantomData<T>,
}

impl<T> ConcreteComponentDestroyer<T> {
    pub fn new() -> Self {
        Self {
            component_type: PhantomData::<T>,
        }
    }
}

impl <T> Clone for ConcreteComponentDestroyer<T> {
    fn clone(&self) -> Self {
        Self { component_type: self.component_type.clone() }
    }
}

impl<T> ComponentDestroyer for ConcreteComponentDestroyer<T> 
    where T: Component<Storage = ComponentStorage::<T>>
{
    fn destroy(&mut self, engine: &mut Engine, scene_handle: SceneHandle, entity_handle: EntityHandle) -> Result<()> {
        // Take component out of storage
        let component: Option<T>;
        {
            // Get scene
            let target_scene = engine.scene_manager.get_scene_mut(scene_handle)?;

            // Take component out of slot
            let component_storage = target_scene.components.get_mut::<T>().unwrap();
            let component_slot = component_storage.data.get_mut(entity_handle.data().index as usize).expect("Critical: Vector not initialized");
            component = Some(component_slot.take().unwrap());
        }

        // Call destroy function on component
        component.unwrap().destroy(engine, scene_handle, entity_handle)?;

        Ok(())
    }
}

// --- Component Deferred Operation ---

pub struct ComponentDeferredOperation<T> 
    where T: Component<Storage = ComponentStorage<T>>
{
    entity_handle: EntityHandle,
    scene_handle: SceneHandle,
    operation: Box<dyn Fn(&mut T, &mut Engine) -> Result<()> + Send>,
    phantom: PhantomData<T>, // 👻
}

impl<T> ComponentDeferredOperation<T> 
    where T: Component<Storage = ComponentStorage<T>>
{
    pub fn new(entity_handle: EntityHandle, scene_handle: SceneHandle, operation: Box<dyn Fn(&mut T, &mut Engine) -> Result<()> + Send>) -> Self {
        Self {
            entity_handle,
            scene_handle,
            operation,
            phantom: PhantomData,
        }
    } 
}

impl<T> DeferredOperation for ComponentDeferredOperation<T> 
    where T: Component<Storage = ComponentStorage<T>>
{
    fn process(&mut self, engine: &mut Engine) -> Result<()> {
        let mut component = Option::<T>::None;
        
        {
            // Get scene 
            let scene = engine.scene_manager.get_scene_mut(self.scene_handle).unwrap();

            // Get component storage
            let component_storage = scene.get_component_storage_mut::<T>().expect("Critical: Component not registered");

            // Get component slot
            let component_slot = component_storage.data.get_mut(self.entity_handle.data().index as usize).unwrap();
        
            // Take component from slot
            component = Some(component_slot.take().expect("Critical: Component is None"));
        }
        
        // Process
        (self.operation)(&mut component.as_mut().unwrap(), engine).context(format!("Deferred operation of {} {} failed", "Component".general_object_style(), get_type_name::<T>().specific_object_style()))?;

        {
            // Get scene 
            let scene = engine.scene_manager.get_scene_mut(self.scene_handle).unwrap();

            // Get component storage
            let component_storage = scene.get_component_storage_mut::<T>().expect("Critical: Component not registered");

            // Get component slot
            let component_slot = component_storage.data.get_mut(self.entity_handle.data().index as usize).unwrap();
        
            // Put component back to slot
            let _ = component_slot.insert(component.take().unwrap());
        }

        Ok(())
    }
}

// --- Global Component Deferred Operation ---

pub struct GlobalComponentDeferredOperation<T> 
    where T: GlobalComponent<Storage = GlobalComponentStorage<T>>
{
    operation: Box<dyn Fn(&mut T, &mut Engine) -> Result<()> + Send>,
    phantom: PhantomData<T>, // 👻
}

impl<T> GlobalComponentDeferredOperation<T> 
    where T: GlobalComponent<Storage = GlobalComponentStorage<T>>
{
    pub fn new(operation: Box<dyn Fn(&mut T, &mut Engine) -> Result<()> + Send>) -> Self {
        Self {
            operation,
            phantom: PhantomData,
        }
    } 
}

impl<T> DeferredOperation for GlobalComponentDeferredOperation<T> 
    where T: GlobalComponent<Storage = GlobalComponentStorage<T>>
{
    fn process(&mut self, engine: &mut Engine) -> Result<()> {
        let mut component = Option::<T>::None;
        
        {
            // Get component storage
            let component_storage = engine.global_components.get_mut::<T>().expect("Critical: Component not registered");

            // Get component slot
            let component_slot = &mut component_storage.data;
        
            // Take component from slot
            component = Some(component_slot.take().expect("Critical: Component is None"));
        }
        
        // Process
        (self.operation)(&mut component.as_mut().unwrap(), engine).context(format!("Deferred operation of {} {} failed", "GlobalComponent".general_object_style(), get_type_name::<T>().specific_object_style()))?;

        {
            // Get component storage
            let component_storage = engine.global_components.get_mut::<T>().expect("Critical: Component not registered");

            // Get component slot
            let component_slot = &mut component_storage.data;
        
            // Put component back to slot
            let _ = component_slot.insert(component.take().unwrap());
        }

        Ok(())
    }
}
