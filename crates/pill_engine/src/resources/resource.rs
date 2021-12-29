#![allow(unused_imports, dead_code, unused_variables)]

use crate::{internal::Engine, };
// use crate::ecs::DeferredUpdateRequest;
use pill_core::PillSlotMapKey;

use std::path::PathBuf;
use anyhow::{Context, Result, Error};
use typemap_rev::TypeMapKey;

// --- Resource ---

pub trait Resource : TypeMapKey {
    type Handle: PillSlotMapKey;
    
    // Optional to implement
    fn initialize(&mut self, engine: &mut Engine) -> Result<()> { Ok(()) } // Called when resource is added to the Engine
    fn deferred_update(&mut self, engine: &mut Engine, request: u32) { } // Called by DeferredUpdateSystem when request related to the resource is being processed
    fn destroy<H: PillSlotMapKey>(&mut self, engine: &mut Engine, self_handle: H) {} // Called when resource is being removed from the Engine
    
    // Required to implement
    fn get_name(&self) -> String;
}

pub enum ResourceLoadType {
    Path(PathBuf),
    Bytes(Box::<[u8]>),
}
