use crate::{ 
    engine::Engine,
    ecs::{ SceneHandle, EntityHandle },
};

use pill_core::{ PillTypeMap, PillTypeMapKey, PillSlotMapKey };

use std::path::PathBuf;
use anyhow::{ Context, Result, Error };


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