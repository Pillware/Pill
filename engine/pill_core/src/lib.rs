#![allow(unused_imports, dead_code)]
#[macro_use]

mod error;
mod math;
mod utils;
mod pill_slotmap;
mod pill_twinmap;
mod pill_typemap;
mod bitmask_utils;
mod timer;
mod color;
mod networking;

// --- Use ---

pub use math::{
    Vector3i,
    Vector2i,
    Vector3f,
    Vector2f,
    Color,
    Matrix3f,
    Direction,
    Vector3fExt,
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
    PillStyle,
    get_type_name,
    get_value_type_name,
    enum_variant_eq,
    get_enum_variant_type_name,
    validate_asset_path,
    get_game_error_message,
};

pub use color::{
	generate_color_palette,
    hsl_to_rgb,
    DISTINCT_COLOR_PALETTE
};

pub use timer::{
    Timer,
    TimerRecord
};

pub use networking::{
    NetServer,
    NetClient,
    NetworkPacket,
    NetworkAction,
    ExitNotice,
    RELIABLE_CHANNEL_ID,
    UNRELIABLE_CHANNEL_ID,
    server_start,
    server_send_one,
    server_broadcast,
    server_broadcast_except,
    server_broacast_exit,
    server_dying_grasp,
    server_flush,
    server_update,
    server_get_events,
    client_connect,
    client_send,
    client_update,
    client_get_events,
    client_flush,
    is_not_ready,
};
