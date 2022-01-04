use crate::{ 
    engine::Engine,
    ecs::{ SceneHandle, EntityHandle },
};

use pill_core::PillSlotMapKey;

use std::path::PathBuf;
use anyhow::{Context, Result, Error};
use pill_core::{ PillTypeMap, PillTypeMapKey };

// --- Component ---

// TypeMapKey trait gives handle to the ResourceStorage
// PillSlotMapKey trait gives handle to the actual object in ResourceStorage

pub trait Component : PillTypeMapKey + Send {  
    // Optional to implement
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> { Ok(()) } // Called when component is added to the engine, before adding it to storage
    fn pass_handles(&mut self, self_entity_handle: EntityHandle, self_scene_handle: SceneHandle) {} // Called right after component is added to the engine
    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> { Ok(()) } // Called by DeferredUpdateSystem when request related to the component is being processed
    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_entity_handle: H) -> Result<()> { Ok(()) } // Called when component is being removed from the engine

    // Required to implement
    //fn get_name(&self) -> String;
}