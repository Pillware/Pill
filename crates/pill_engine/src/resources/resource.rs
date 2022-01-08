use crate::engine::Engine;

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

    // Optional to implement
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> { Ok(()) } // Called when resource is added to the engine, before adding it to storage
    fn pass_handle<H: PillSlotMapKey>(&mut self, self_handle: H) {} // Called right after resource is added to the engine, after adding it to storage
    fn deferred_update(&mut self, engine: &mut Engine, request: usize) -> Result<()> { Ok(()) } // Called by DeferredUpdateSystem when request related to the resource is being processed
    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) -> Result<()> { Ok(()) } // Called when resource is being removed from the engine
}

pub enum ResourceLoadType {
    Path(PathBuf),
    Bytes(Box::<[u8]>),
}
