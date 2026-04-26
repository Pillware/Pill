use std::ffi::{c_char, c_void};

pub const PILL_ENGINE_ABI_VERSION: u32 = 1;

// return codes
pub const PILL_OK: i32 = 0;
pub const PILL_ERR: i32 = 1;

#[repr(C)]
pub struct PillEngineCreateArgsV1 {
    pub struct_size: u32,
    /// Pointer produced by `Arc::into_raw(Arc::clone(&window))` in standalone.
    /// Interpreted by the runtime as `*const winit::window::Window`.
    pub window_ptr: *const c_void,

    /// UTF-8, null-terminated
    pub game_dylib_path: *const c_char,
    pub game_resources_dir: *const c_char,
    pub config_path: *const c_char,

    pub initial_w: u32,
    pub initial_h: u32,
}

pub type EngineHandle = *mut c_void;

#[repr(C)]
pub struct PillEngineApiV1 {
    pub struct_size: u32, // set to size of Self
    pub abi_version: u32,

    /// Later stable hash that engine/game expect of each other
    pub abi_hash: u64,

    pub last_error_utf8: extern "C" fn() -> *const c_char,

    pub create:
        extern "C" fn(args: *const PillEngineCreateArgsV1, out_engine: *mut EngineHandle) -> i32,
    pub destroy: extern "C" fn(engine: EngineHandle),

    pub update: extern "C" fn(engine: EngineHandle, dt_ns: u64),
    pub resize: extern "C" fn(engine: EngineHandle, w: u32, h: u32),

    // --- Input APIs ---
    /// Forward winit::event::WindowEvent pointer (opaque to ABI crate).
    /// Runtime will call engine.pass_input_to_egui(&WindowEvent).
    pub window_event: extern "C" fn(engine: EngineHandle, window_event_ptr: *const c_void),

    /// Forward winit::event::KeyEvent pointer (opaque). Runtime calls engine.pass_keyboard_key_input(&KeyEvent).
    pub key_event: extern "C" fn(engine: EngineHandle, key_event_ptr: *const c_void),

    /// Hard ABI mouse button: 0=L 1=R 2=M 3=Back 4=Forward 5+=Other(n-5)
    pub mouse_button: extern "C" fn(engine: EngineHandle, button: u32, pressed: bool),

    /// Mouse motion delta
    pub mouse_delta: extern "C" fn(engine: EngineHandle, dx: f64, dy: f64),

    /// Cursor moved (physical coords)
    pub cursor_position: extern "C" fn(engine: EngineHandle, x: f64, y: f64),

    /// Mouse wheel: treat as LineDelta for now (good enough); you can add pixel variant later.
    pub mouse_wheel_line: extern "C" fn(engine: EngineHandle, dx: f32, dy: f32),

    // --- Hot reload ---
    pub reload_game: extern "C" fn(engine: EngineHandle, game_dylib_path: *const c_char) -> i32,
}

pub const PILL_ENGINE_API_SYMBOL: &[u8] = b"get_pill_engine_api_v1\0";
