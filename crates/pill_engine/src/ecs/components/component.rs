use crate::{
    engine::Engine,
    ecs::{ SceneHandle, EntityHandle, ComponentStorage, GlobalComponentStorage }
};

use pill_core::{ PillTypeMap, PillTypeMapKey, PillSlotMapKey };

use std::{path::PathBuf, marker::PhantomData};
use anyhow::{ Context, Result, Error };
use dyn_clone::DynClone;


// --- Component ---

// TypeMapKey trait gives handle to the ResourceStorage
// PillSlotMapKey trait gives handle to the actual object in ResourceStorage

pub trait Component : PillTypeMapKey + Send {
    // Optional to implement
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> { Ok(()) } // Called when component is added to the engine, before adding it to storage
    fn pass_handles(&mut self, self_scene_handle: SceneHandle, self_entity_handle: EntityHandle) {} // Called right after component is added to the engine
    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> { Ok(()) } // Called by DeferredUpdateSystem when request related to the component is being processed
    fn destroy(&mut self, engine: &mut Engine, self_scene_handle: SceneHandle, self_entity_handle: EntityHandle) -> Result<()> { Ok(()) } // Called when component is being removed from the engine
}

// --- Global Component ---

pub trait GlobalComponent : PillTypeMapKey + Send {
    // Optional to implement
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> { Ok(()) } // Called when component is added to the engine, before adding it to storage
    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> { Ok(()) } // Called by DeferredUpdateSystem when request related to the component is being processed
    fn destroy(&mut self, engine: &mut Engine) -> Result<()> { Ok(()) } // Called when component is being removed from the engine
}

// --- Component Destroyers ---

// Approach that makes it possible to delete components by iterating over typemap of component storages and not knowing the types of the components
// Use DynClone to be able to clone Boxed component destroyers
pub trait ComponentDestroyer: DynClone {
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
