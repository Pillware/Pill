use crate::internal::Engine;
use pill_core::PillSlotMapKey;

use std::path::PathBuf;
use anyhow::{Context, Result, Error};
use typemap_rev::TypeMapKey;

// --- Resource ---

pub trait Resource : TypeMapKey {
    type Handle: PillSlotMapKey;
    
    fn initialize(&mut self, _engine: &mut Engine) -> Result<()> { Ok(()) }
    fn destroy<H: PillSlotMapKey>(&mut self, _engine: &mut Engine, _self_handlee: H) {}
    fn get_name(&self) -> String;
}

pub enum ResourceLoadType {
    Path(PathBuf),
    Bytes(Box::<[u8]>),
}
