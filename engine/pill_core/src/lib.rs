#![allow(unused_imports, dead_code)]
#![macro_use]

mod bitmask_utils;
mod color;
mod error;
mod handle;
mod log;
mod math;
#[cfg(not(target_arch = "wasm32"))]
mod networking;
mod pill_slotmap;
mod pill_twinmap;
mod pill_typemap;
mod style;
mod tags;
mod timer;
mod utils;

// --- Error types ---

pub type PillError = Box<dyn std::error::Error + Send + Sync + 'static>;
pub type Result<T> = std::result::Result<T, PillError>;

pub trait ErrorContext<T> {
    fn context<E: Into<PillError>>(self, err: E) -> Result<T>;
}

impl<T> ErrorContext<T> for Option<T> {
    #[inline]
    fn context<E: Into<PillError>>(self, err: E) -> Result<T> {
        self.ok_or_else(|| err.into())
    }
}

impl<T, S: Into<PillError>> ErrorContext<T> for std::result::Result<T, S> {
    #[inline]
    fn context<E: Into<PillError>>(self, err: E) -> Result<T> {
        self.map_err(|_| err.into())
    }
}

// --- Use ---

pub use math::{
    Color, Direction, Matrix3f, Matrix3fA, Matrix4f, Vector2f, Vector2i, Vector3f, Vector4f,
};

pub use error::{CoreError, EngineError, RendererError};

pub use pill_slotmap::{PillSlotMap, PillSlotMapKey, PillSlotMapKeyData};

pub use pill_twinmap::PillTwinMap;

pub use pill_typemap::{PillTypeMap, PillTypeMapKey};

pub use bitmask_utils::{
    create_bitmask_from_range, create_bitmask_with_one, get_indices_of_set_elements,
};

pub use utils::{
    enum_variant_eq, get_enum_variant_type_name, get_game_error_message, get_type_name,
    get_value_type_name, validate_asset_path,
};

pub use color::{generate_color_palette, hsl_to_rgb, DISTINCT_COLOR_PALETTE};

pub use handle::{Handle, ResourcePool};

pub use tags::{
    RendererBufferTag, RendererCameraTag, RendererMaterialTag, RendererMeshTag,
    RendererPipelineTag, RendererPipelineV2Tag, RendererTextureTag,
};

pub use timer::{Timer, TimerRecord};

#[cfg(not(target_arch = "wasm32"))]
pub use networking::{
    client_connect, client_disconnect, client_flush, client_get_events, client_send, client_update,
    is_not_ready, server_broadcast, server_broadcast_except, server_broadcast_exit,
    server_disconnect_client, server_dying_grasp, server_flush, server_get_events, server_send_one,
    server_start, server_update, ExitNotice, NetworkAction, NetworkClient, NetworkPacket,
    NetworkServer, RELIABLE_CHANNEL_ID, UNRELIABLE_CHANNEL_ID,
};

pub use style::PillStyle;

pub use log::{get_default_log_levels, set_log_levels, LogContext};
