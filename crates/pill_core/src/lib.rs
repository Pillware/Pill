#[allow(unused_imports, dead_code)]
#[macro_use]
mod error;
mod types;
mod utils;
mod pill_slotmap;
mod bitmask_utils;

pub use types::*;
pub use error::EngineError;

pub use pill_slotmap::{PillSlotMap, PillSlotMapKey, PillSlotMapKeyData};
pub use bitmask_utils::create_bitmask_from_range;

pub use utils::get_type_name;
pub use utils::get_value_type_name;
pub use utils::PillStyle;

pub extern crate approx;
pub extern crate nalgebra as na;