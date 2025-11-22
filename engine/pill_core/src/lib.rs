#![allow(unused_imports, dead_code)]
#![macro_use]

mod error;
mod math;
mod utils;
mod pill_slotmap;
mod pill_twinmap;
mod pill_typemap;
mod bitmask_utils;
mod timer;
mod log;
mod style;

// --- Use ---

pub use math::{
    Vector2i,
    Vector2f,
    Vector3f,
    Vector4f,
    Color,
    Matrix3f,
    Matrix3fA,
    Matrix4f,
    Direction,
};

pub use error::{
    EngineError,
    RendererError
};

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
    create_bitmask_with_one,
    get_indices_of_set_elements,
};

pub use utils::{
    get_type_name,
    get_value_type_name,
    enum_variant_eq,
    get_enum_variant_type_name,
    validate_asset_path,
    get_game_error_message,
};

pub use timer::{
    Timer,
    TimerRecord
};

pub use style::{
    PillStyle,
};

pub use log::{
    set_log_levels,
    get_default_log_levels,
    LogContext
};
