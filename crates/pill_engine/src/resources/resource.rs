use std::path::PathBuf;

use pill_core::PillSlotMapKey;
use typemap_rev::TypeMapKey;

use crate::game::Engine;

// --- Resource ---

pub trait Resource : TypeMapKey {
    type Handle: PillSlotMapKey;
    
    fn initialize(&mut self, _engine: &mut Engine) {}
    fn destroy<H: PillSlotMapKey>(&mut self, _engine: &mut Engine, _self_handlee: H) {}
    fn get_name(&self) -> String;
}

pub enum ResourceLoadType {
    Path(PathBuf),
    Bytes(Box::<[u8]>),
}
