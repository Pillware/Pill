#![allow(unused_imports, dead_code)]
#[macro_use]

mod error;
mod math;
mod utils;
mod pill_slotmap;
mod pill_twinmap;
mod pill_typemap;
mod bitmask_utils;

// --- Use ---

pub use math::*;

pub use error::EngineError;

pub use pill_slotmap::{ 
    PillSlotMap, 
    PillSlotMapKey, 
    PillSlotMapKeyData,
};

pub use pill_twinmap::{
    PillTwinMap,
};

pub use pill_typemap::{
    PillTypeMap,
    PillTypeMapKey,
};

pub use bitmask_utils::{
    create_bitmask_from_range,
};

pub use utils::{ 
    PillStyle,
    get_type_name, 
    get_value_type_name, 
    enum_variant_eq, 
    get_enum_variant_type_name, 
    validate_asset_path,

};
