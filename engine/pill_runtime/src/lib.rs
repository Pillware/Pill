// This file is the runtime dynamic library loaded by pill_native at startup.
//
// Responsibilities:
//   - Exposes a C-ABI vtable (PillEngineApiV1) so the host can call into
//     the engine without linking against pill_runtime directly.
//   - Loads the project dylib, creates the Engine, initialises the Renderer
//     (wgpu), and wires up the ECS and resource manager.
//   - Forwards window/input events from the host to the engine.
//   - Supports hot-reload by swapping the project dylib in-place.

use std::{
    cell::RefCell,
    ffi::{c_char, c_void, CStr, CString},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use libloading::{Library, Symbol};
use pill_abi::*;
use pill_core::{set_log_levels, PillError, Result};
use pill_engine::internal::*;
use pill_renderer::Renderer;
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    window::Window,
};

// ---------------------------------------------------------------------------
// Error Handling Helpers
// ---------------------------------------------------------------------------

// Thread-local storage for the last error message produced by any FFI
// function.  The host reads this via last_error_utf8() after a call
// returns PILL_ERR.
thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::new("").unwrap());
}

// Stores a human-readable error message in the thread-local LAST_ERROR
// cell so the host can retrieve it across the FFI boundary.
fn set_last_error(message: impl Into<String>) {
    let message = message.into();
    LAST_ERROR.with(|error_cell| {
        *error_cell.borrow_mut() =
            CString::new(message).unwrap_or_else(|_| CString::new("error").unwrap());
    });
}

// Converts a null-terminated C-string pointer into a Rust &str.
// Returns an error if the pointer is null or contains invalid UTF-8.
//
// SAFETY: the caller must ensure `pointer` is valid and points to
// a null-terminated C string with a static or sufficiently long lifetime.
unsafe fn c_string_to_str(pointer: *const c_char) -> Result<&'static str> {
    if pointer.is_null() {
        return Err("null c_string_to_str".into());
    }
    CStr::from_ptr(pointer)
        .to_str()
        .map_err(|error| -> PillError { error.to_string().into() })
}

// ---------------------------------------------------------------------------
// Project Loading
// ---------------------------------------------------------------------------

// Opens the project dynamic library, looks up the `get_project` export,
// and returns both the loaded library handle and the Box<dyn PillProject>.
//
// The returned Library must be kept alive for as long as the project
// trait object is in use — dropping it unloads the dylib.
fn load_project(project_library_path: &str) -> Result<(Library, Box<dyn PillProject>)> {
    // SAFETY:
    // As long as the caller stops ALL functions running in the pill project + engine
    // we are fine to unload + load a new Box<dyn PillProject>
    type CreateProjectFn = unsafe extern "C" fn() -> *mut c_void;

    // 1. Load the project dynamic library (the compiled pill project crate).
    let project_dynamic_library = unsafe {
        Library::new(project_library_path).map_err(|error| -> PillError {
            format!(
                "Failed to load pill project dynamic library at {project_library_path}: {error}"
            )
            .into()
        })?
    };

    // 2. Look up the `get_project` symbol — every pill project must export
    //    this function to return its Box<dyn PillProject>.
    let get_project_function: Symbol<CreateProjectFn> = unsafe {
        project_dynamic_library.get(b"get_project")
    }
    .map_err(|error| -> PillError { format!("Missing symbol get_project: {error}").into() })?;

    // 3. Call get_project() and reconstruct the Box from the raw pointer.
    let project = unsafe { *Box::from_raw(get_project_function() as *mut Box<dyn PillProject>) };
    Ok((project_dynamic_library, project))
}

// ---------------------------------------------------------------------------
// Runtime State
// ---------------------------------------------------------------------------

// Holds all state for one engine instance.  Created once per process
// by the `create` FFI entry point and destroyed by `destroy`.
struct Runtime {
    // Keeps the winit Window alive for the wgpu Renderer.
    window: Arc<Window>,
    // Last known physical window size, updated on resize.
    window_size: PhysicalSize<u32>,

    resource_directory: PathBuf,

    config: EngineConfig,

    process: EngineProcessInfo,

    // The engine is stored as an Option so it can be taken out and
    // dropped before the project library is unloaded (drop order matters).
    engine: Option<Engine>,
    // Keeps the project dynamic library loaded; must outlive the engine.
    project_library: Option<Library>,
}

impl Runtime {
    // Creates and initialises an Engine from the given project.
    // Builds the wgpu Renderer, wires up the ECS, and calls
    // engine.initialize() to run startup systems.
    fn build_engine(&self, project: Box<dyn PillProject>) -> Result<Engine> {
        // Create the wgpu-backed renderer, passing a clone of the window Arc.
        let renderer: Box<dyn PillRenderer> = Box::new(<Renderer as PillRenderer>::new(
            Arc::clone(&self.window),
            self.config.clone(),
        )?);

        let mut engine = Engine::new(
            project,
            self.resource_directory.clone(),
            renderer,
            self.config.clone(),
            self.process.clone(),
        );
        engine.initialize(Some(self.window_size))?;
        Ok(engine)
    }

    // Tears down the engine gracefully before unloading the project dylib.
    // Must be called before project_library is dropped so that any drop
    // glue in the project trait object runs while the dylib is still loaded.
    fn shutdown_engine(&mut self) {
        if let Some(mut engine) = self.engine.take() {
            engine.shutdown();
            drop(engine);
        }
    }
}

// ===========================================================================
// C ABI — FFI entry points exposed to pill_native via PillEngineApiV1
// ===========================================================================
//
// Every function below follows the same pattern:
//   1. Validate the engine handle (null check).
//   2. Cast the opaque handle back to &mut Runtime.
//   3. Delegate to the Engine (if present).
//
// Functions that can fail return PILL_OK / PILL_ERR and set the
// thread-local LAST_ERROR on failure so the host can retrieve details.

// ---------------------------------------------------------------------------
// ABI: Error & Lifecycle
// ---------------------------------------------------------------------------

// Returns a pointer to the thread-local error message.
// The host must read it immediately — subsequent FFI calls may overwrite it.
extern "C" fn last_error_utf8() -> *const c_char {
    LAST_ERROR.with(|error_cell| error_cell.borrow().as_ptr())
}

// Creates the engine inside the runtime.
//
// Takes ownership of:
//   - One Arc<Window> reference (transferred via Arc::into_raw by the host).
//   - The project dynamic library path (loaded via libloading).
//
// Writes the opaque engine handle to *out_engine on success.
extern "C" fn create(args: *const PillEngineCreateArgsV1, out_engine: *mut EngineHandle) -> i32 {
    // Validate incoming pointers.
    if args.is_null() || out_engine.is_null() {
        set_last_error("create: args or out_engine is null");
        return PILL_ERR;
    }

    // Wrap the entire initialisation in a closure so we can use `?`
    // and translate any error into PILL_ERR + LAST_ERROR.
    let result = (|| -> Result<()> {
        let create_args = unsafe { &*args };

        if create_args.window_ptr.is_null() {
            return Err("create: window_ptr is null".into());
        }

        // Convert C-string fields to Rust strings.
        let project_library_path =
            unsafe { c_string_to_str(create_args.project_dylib_path) }?.to_string();
        let project_resource_directory =
            unsafe { c_string_to_str(create_args.project_resources_dir) }?.to_string();
        let config_path = unsafe { c_string_to_str(create_args.config_path) }?.to_string();

        // Reconstruct the Arc<Window> from the raw pointer the host gave us.
        // This consumes exactly one reference count.
        let window = unsafe { Arc::from_raw(create_args.window_ptr as *const Window) };

        // Load and parse the project configuration file.
        let config_ini = std::fs::read_to_string(&config_path).unwrap_or_default();
        let mut config = EngineConfig::from_ini(&config_ini);

        // Initialise the DLL-side logger so engine error/info logs are visible.
        // (pill_native sets up the app-side logger; the DLL has its own global state.)
        if let Ok(log_levels) = config.get_str("LOG_LEVELS") {
            set_log_levels(&log_levels, false);
        }

        // Fill in default window dimensions from the create args if the
        // config file didn't specify them.
        if config.get_int("WINDOW_WIDTH").is_err() {
            config.set("WINDOW_WIDTH", create_args.initial_w as i64);
        }
        if config.get_int("WINDOW_HEIGHT").is_err() {
            config.set("WINDOW_HEIGHT", create_args.initial_h as i64);
        }

        // Determine the compile mode from environment variables.
        // Falls back to "debug" for standalone runs where the launcher
        // did not set PILL_COMPILE_MODE (e.g. double-clicking the .exe).
        let compile_mode = std::env::var("PILL_COMPILE_MODE").unwrap_or_else(|_| {
            match std::env::var("PILL_STANDALONE_LAYOUT").ok().as_deref() {
                Some("packaged") => "release".to_string(),
                _ => "debug".to_string(),
            }
        });
        let process =
            EngineProcessInfo::new(&compile_mode, pill_engine::internal::BuildTarget::Native);

        // Load the project dynamic library and construct the PillProject.
        let (project_library, project) = load_project(&project_library_path)?;

        // Assemble the Runtime struct — the engine is built immediately after.
        let mut runtime = Box::new(Runtime {
            window,
            window_size: winit::dpi::PhysicalSize::new(
                create_args.initial_w,
                create_args.initial_h,
            ),
            resource_directory: project_resource_directory.into(),
            config,
            process,
            engine: None,
            project_library: Some(project_library),
        });

        // Build the engine (creates wgpu device, initialises renderer & ECS).
        let engine = runtime.build_engine(project)?;
        runtime.engine = Some(engine);

        // Transfer ownership of the Runtime to the host as an opaque pointer.
        unsafe {
            *out_engine = Box::into_raw(runtime) as *mut c_void;
        }
        Ok(())
    })();

    match result {
        Ok(()) => PILL_OK,
        Err(error) => {
            set_last_error(format!("{error}"));
            PILL_ERR
        }
    }
}

// Tears down the engine and releases all runtime resources.
//
// Drop order is critical:
//   1. Shut down and drop the Engine (which drops the Renderer / wgpu stack).
//   2. Drop the project Library (unloads the project dylib).
//   3. Drop the Runtime itself (releases window Arc, config, paths).
extern "C" fn destroy(engine: EngineHandle) {
    if engine.is_null() {
        return;
    }
    unsafe {
        // Reconstruct the Box<Runtime> so it will be dropped at the end
        // of this scope, running all destructors in the correct order.
        let mut runtime = Box::from_raw(engine as *mut Runtime);

        // Drop the engine before unloading the project dylib so that
        // the PillProject trait object's drop glue runs while the
        // dylib is still mapped in memory.
        runtime.shutdown_engine();
        runtime.project_library.take();
        // Runtime fields (window, config, paths) are dropped here.
    }
}

// ---------------------------------------------------------------------------
// ABI: Frame Update & Window Resize
// ---------------------------------------------------------------------------

// Advances the engine by one frame.
// `delta_time_nanoseconds` is the wall-clock duration since the last update,
// measured by the host and passed as nanoseconds to avoid floating-point
// across the FFI boundary.
extern "C" fn update(engine: EngineHandle, delta_time_nanoseconds: u64) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(engine) = runtime.engine.as_mut() {
        engine.update(Duration::from_nanos(delta_time_nanoseconds));
    }
}

// Notifies the engine that the window has been resized.
// Updates the stored size and forwards to the renderer so it can
// recreate the swapchain at the new dimensions.
extern "C" fn resize(engine: EngineHandle, width: u32, height: u32) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    runtime.window_size = winit::dpi::PhysicalSize::new(width, height);
    if let Some(engine) = runtime.engine.as_mut() {
        engine.resize(runtime.window_size);
    }
}

// ---------------------------------------------------------------------------
// ABI: Input Forwarding
// ---------------------------------------------------------------------------
//
// Each input function receives a raw *const c_void cast from the
// corresponding winit event type.  The runtime casts it back to the
// concrete reference and forwards it to the engine.

// Forwards a winit WindowEvent for egui input processing.
// No-op when the debug_ui feature is disabled (pass_input_to_egui
// ignores the event in that configuration).
extern "C" fn window_event(engine: EngineHandle, window_event_ptr: *const c_void) {
    if engine.is_null() || window_event_ptr.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(engine) = runtime.engine.as_mut() {
        let window_event_ref = unsafe { &*(window_event_ptr as *const winit::event::WindowEvent) };
        engine.pass_input_to_egui(window_event_ref);
    }
}

// Forwards a winit KeyEvent to the engine's input queue.
extern "C" fn key_event(engine: EngineHandle, key_event_ptr: *const c_void) {
    if engine.is_null() || key_event_ptr.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(engine) = runtime.engine.as_mut() {
        let key_event_ref = unsafe { &*(key_event_ptr as *const winit::event::KeyEvent) };
        engine.pass_keyboard_key_input(key_event_ref);
    }
}

// Decodes the stable u32 mouse-button encoding used across the FFI
// boundary back into winit's MouseButton enum.
// Encoding (mirrors encode_mouse_button in pill_native):
//   0 = Left, 1 = Right, 2 = Middle, 3 = Back, 4 = Forward, 5+ = Other(n-5)
fn decode_mouse_button(button: u32) -> winit::event::MouseButton {
    use winit::event::MouseButton::*;
    match button {
        0 => Left,
        1 => Right,
        2 => Middle,
        3 => Back,
        4 => Forward,
        n => Other(n.saturating_sub(5) as u16),
    }
}

// Forwards a mouse button press or release event.
extern "C" fn mouse_button(engine: EngineHandle, button: u32, pressed: bool) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(engine) = runtime.engine.as_mut() {
        let decoded_button = decode_mouse_button(button);
        let state = if pressed {
            winit::event::ElementState::Pressed
        } else {
            winit::event::ElementState::Released
        };
        engine.pass_mouse_key_input(&decoded_button, &state);
    }
}

// Forwards raw mouse motion delta (unscaled, in physical pixels).
extern "C" fn mouse_delta(engine: EngineHandle, delta_x: f64, delta_y: f64) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(engine) = runtime.engine.as_mut() {
        engine.pass_mouse_delta_input(&(delta_x, delta_y));
    }
}

// Forwards the absolute cursor position in physical pixels.
extern "C" fn cursor_position(engine: EngineHandle, x: f64, y: f64) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(engine) = runtime.engine.as_mut() {
        let position = PhysicalPosition::new(x, y);
        engine.pass_mouse_position_input(&position);
    }
}

// Forwards mouse-wheel line-scroll deltas.
extern "C" fn mouse_wheel_line(engine: EngineHandle, delta_x: f32, delta_y: f32) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(engine) = runtime.engine.as_mut() {
        let delta = (delta_x as f64, delta_y as f64);
        engine.pass_mouse_delta_input(&delta);
    }
}

// ---------------------------------------------------------------------------
// ABI: Hot-Reload
// ---------------------------------------------------------------------------

// Swaps the currently loaded project for a newly compiled one.
//
// Sequence (order is critical for safety):
//   1. Shut down the current engine and drop it.
//   2. Unload the old project dylib.
//   3. Load the new project dylib and construct a fresh PillProject.
//   4. Build a brand-new Engine around the new project.
//
// Returns PILL_OK on success, PILL_ERR with LAST_ERROR set on failure.
// On failure the runtime is left without an engine — the host should
// treat this as a fatal state.
extern "C" fn reload_project(engine: EngineHandle, project_dylib_path: *const c_char) -> i32 {
    if engine.is_null() {
        set_last_error("reload_project: engine is null");
        return PILL_ERR;
    }

    let result = (|| -> Result<()> {
        let runtime = unsafe { &mut *(engine as *mut Runtime) };
        let project_path = unsafe { c_string_to_str(project_dylib_path) }?.to_string();

        // 1-2. Tear down old engine and unload old project dylib.
        runtime.shutdown_engine();
        runtime.project_library.take();

        // 3. Load the new project dylib.
        let (project_library, project) = load_project(&project_path)?;
        runtime.project_library = Some(project_library);

        // 4. Build a new engine — this calls PillProject::start() internally.
        let new_engine = runtime.build_engine(project)?;
        runtime.engine = Some(new_engine);

        Ok(())
    })();

    match result {
        Ok(()) => PILL_OK,
        Err(error) => {
            set_last_error(format!("{error}"));
            PILL_ERR
        }
    }
}

// ---------------------------------------------------------------------------
// ABI: Status Queries
// ---------------------------------------------------------------------------

// Returns 1 if the engine has requested graceful shutdown (e.g. a
// benchmark that finishes after N frames), 0 otherwise.
extern "C" fn is_exit_requested(engine: EngineHandle) -> i32 {
    if engine.is_null() {
        return 0;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    match runtime.engine.as_ref() {
        Some(engine) if engine.is_exit_requested() => 1,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// ABI VTable Export
// ---------------------------------------------------------------------------

// Static vtable that pill_native reads via get_pill_engine_api_v1().
// Every function pointer in this struct is an extern "C" function
// defined above, forming the stable ABI contract between host and runtime.
static API: PillEngineApiV1 = PillEngineApiV1 {
    struct_size: std::mem::size_of::<PillEngineApiV1>() as u32,
    abi_version: PILL_ENGINE_ABI_VERSION,
    abi_hash: 0, // TODO: implement this check
    last_error_utf8,
    create,
    destroy,
    update,
    resize,
    window_event,
    key_event,
    mouse_button,
    mouse_delta,
    cursor_position,
    mouse_wheel_line,
    reload_project,
    is_exit_requested,
};

// Entry point called by pill_native (via libloading) to obtain the
// static API vtable.  The host never links against pill_runtime
// directly — all interaction goes through this table.
#[no_mangle]
pub extern "C" fn get_pill_engine_api_v1() -> *const PillEngineApiV1 {
    &API
}
