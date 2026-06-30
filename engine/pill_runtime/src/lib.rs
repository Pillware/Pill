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

thread_local! {
    static LAST_ERR: RefCell<CString> = RefCell::new(CString::new("").unwrap());
}

fn set_err(msg: impl Into<String>) {
    let s = msg.into();
    LAST_ERR.with(|e| {
        *e.borrow_mut() = CString::new(s).unwrap_or_else(|_| CString::new("error").unwrap());
    });
}

unsafe fn cstr(p: *const c_char) -> Result<&'static str> {
    if p.is_null() {
        return Err("null cstr".into());
    }
    CStr::from_ptr(p)
        .to_str()
        .map_err(|e| -> PillError { e.to_string().into() })
}

fn load_project(project_library_path: &str) -> Result<(Library, Box<dyn PillProject>)> {
    // SAFETY:
    // As long as the caller stops ALL functions running in the pill project + engine
    // we are fine to unload + load a new Box<dyn PillProject>
    type CreateProjectFn = unsafe extern "C" fn() -> *mut c_void;
    let project_dynamic_library = unsafe {
        Library::new(project_library_path).map_err(|e| -> PillError {
            format!("Failed to load pill project dynamic library at {project_library_path}: {e}")
                .into()
        })?
    };
    let get_project_function: Symbol<CreateProjectFn> =
        unsafe { project_dynamic_library.get(b"get_project") }
            .map_err(|e| -> PillError { format!("Missing symbol get_project: {e}").into() })?;
    let project = unsafe { *Box::from_raw(get_project_function() as *mut Box<dyn PillProject>) };
    Ok((project_dynamic_library, project))
}

struct Runtime {
    // Keep the window alive for Renderer
    window: Arc<Window>,
    // Last known physical size
    window_size: PhysicalSize<u32>,

    resource_directory: PathBuf,

    config: EngineConfig,

    process: EngineProcessInfo,

    // Keep engine ptr for hot-reload
    engine: Option<Engine>,
    project_library: Option<Library>,
}

impl Runtime {
    fn build_engine(&self, project: Box<dyn PillProject>) -> Result<Engine> {
        let t = std::time::Instant::now();
        let renderer: Box<dyn PillRenderer> = Box::new(<Renderer as PillRenderer>::new(
            Arc::clone(&self.window),
            self.config.clone(),
        )?);
        println!("[TIMING]     Renderer::new (wgpu init): {:.3}s", t.elapsed().as_secs_f64());

        let mut engine = Engine::new(
            project,
            self.resource_directory.clone(),
            renderer,
            self.config.clone(),
            self.process.clone(),
        );
        let t = std::time::Instant::now();
        engine.initialize(Some(self.window_size))?;
        println!("[TIMING]     engine.initialize: {:.3}s", t.elapsed().as_secs_f64());
        Ok(engine)
    }

    fn shutdown_engine(&mut self) {
        if let Some(mut e) = self.engine.take() {
            e.shutdown();
            drop(e);
        }
    }
}

// --- ABI ---
extern "C" fn last_error_utf8() -> *const c_char {
    LAST_ERR.with(|e| e.borrow().as_ptr())
}

extern "C" fn create(args: *const PillEngineCreateArgsV1, out_engine: *mut EngineHandle) -> i32 {
    if args.is_null() || out_engine.is_null() {
        set_err("create: args or out_engine is null");
        return PILL_ERR;
    }

    let r = (|| -> Result<()> {
        let a = unsafe { &*args };

        if a.window_ptr.is_null() {
            return Err("create: window_ptr is null".into());
        }

        let project_library_path = unsafe { cstr(a.project_dylib_path) }?.to_string();
        let project_resource_dir = unsafe { cstr(a.project_resources_dir) }?.to_string();
        let config_path = unsafe { cstr(a.config_path) }?.to_string();

        // Take ownership of one reference to Window that standalone gave us via
        // Arc::into_raw(clone)
        let window = unsafe { Arc::from_raw(a.window_ptr as *const Window) };

        let config_ini = std::fs::read_to_string(&config_path).unwrap_or_default();
        let mut config = EngineConfig::from_ini(&config_ini);

        // Initialize the DLL-side logger so engine error/info logs are visible.
        // (pill_native sets up the app-side logger; the DLL has its own global state.)
        if let Ok(log_levels) = config.get_str("LOG_LEVELS") {
            set_log_levels(&log_levels, false);
        }

        if config.get_int("WINDOW_WIDTH").is_err() {
            config.set("WINDOW_WIDTH", a.initial_w as i64);
        }
        if config.get_int("WINDOW_HEIGHT").is_err() {
            config.set("WINDOW_HEIGHT", a.initial_h as i64);
        }
        let compile_mode = std::env::var("PILL_COMPILE_MODE").unwrap_or_else(|_| {
            // Default: if running in packaged layout it's "release", otherwise "debug".
            // The launcher always sets this, but standalone runs (e.g. double-clicking
            // the .exe) should just work without env vars.
            match std::env::var("PILL_STANDALONE_LAYOUT").ok().as_deref() {
                Some("packaged") => "release".to_string(),
                _ => "debug".to_string(),
            }
        });
        let process =
            EngineProcessInfo::new(&compile_mode, pill_engine::internal::BuildTarget::Native);

        let t = std::time::Instant::now();
        let (project_library, project) = load_project(&project_library_path)?;
        println!("[TIMING]   load_project (project.dll): {:.3}s", t.elapsed().as_secs_f64());

        let mut runtime = Box::new(Runtime {
            window,
            window_size: winit::dpi::PhysicalSize::new(a.initial_w, a.initial_h),
            resource_directory: project_resource_dir.into(),
            config,
            process,
            engine: None,
            project_library: Some(project_library),
        });

        let t = std::time::Instant::now();
        let engine = runtime.build_engine(project)?;
        println!("[TIMING]   build_engine total: {:.3}s", t.elapsed().as_secs_f64());
        runtime.engine = Some(engine);

        unsafe {
            *out_engine = Box::into_raw(runtime) as *mut c_void;
        }
        Ok(())
    })();

    match r {
        Ok(()) => PILL_OK,
        Err(e) => {
            set_err(format!("{e}"));
            PILL_ERR
        }
    }
}

extern "C" fn destroy(engine: EngineHandle) {
    if engine.is_null() {
        return;
    }
    unsafe {
        let mut runtime = Box::from_raw(engine as *mut Runtime);

        // Drop engine and pill project first, then unload
        runtime.shutdown_engine();
        runtime.project_library.take();
        // rt drops here
    }
}

extern "C" fn update(engine: EngineHandle, dt_ns: u64) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(e) = runtime.engine.as_mut() {
        e.update(Duration::from_nanos(dt_ns));
    }
}

extern "C" fn resize(engine: EngineHandle, w: u32, h: u32) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    runtime.window_size = winit::dpi::PhysicalSize::new(w, h);
    if let Some(e) = runtime.engine.as_mut() {
        e.resize(runtime.window_size);
    }
}

extern "C" fn window_event(engine: EngineHandle, window_event_ptr: *const c_void) {
    if engine.is_null() || window_event_ptr.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(e) = runtime.engine.as_mut() {
        // Soft ABI: standalone passes &WindowEvent as *const c_void
        let we = unsafe { &*(window_event_ptr as *const winit::event::WindowEvent) };
        e.pass_input_to_egui(we);
    }
}

extern "C" fn key_event(engine: EngineHandle, key_event_ptr: *const c_void) {
    if engine.is_null() || key_event_ptr.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(e) = runtime.engine.as_mut() {
        // Soft ABI: standalone passes &KeyEvent as *const c_void
        let ke = unsafe { &*(key_event_ptr as *const winit::event::KeyEvent) };
        e.pass_keyboard_key_input(ke);
    }
}

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

extern "C" fn mouse_button(engine: EngineHandle, button: u32, pressed: bool) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(e) = runtime.engine.as_mut() {
        let decoded_button = decode_mouse_button(button);
        let state = if pressed {
            winit::event::ElementState::Pressed
        } else {
            winit::event::ElementState::Released
        };
        e.pass_mouse_key_input(&decoded_button, &state);
    }
}

extern "C" fn mouse_delta(engine: EngineHandle, dx: f64, dy: f64) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(e) = runtime.engine.as_mut() {
        e.pass_mouse_delta_input(&(dx, dy));
    }
}

extern "C" fn cursor_position(engine: EngineHandle, x: f64, y: f64) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(e) = runtime.engine.as_mut() {
        let position = PhysicalPosition::new(x, y);
        e.pass_mouse_position_input(&position);
    }
}

extern "C" fn mouse_wheel_line(engine: EngineHandle, dx: f32, dy: f32) {
    if engine.is_null() {
        return;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    if let Some(e) = runtime.engine.as_mut() {
        let delta = (dx as f64, dy as f64);
        e.pass_mouse_delta_input(&delta);
    }
}

extern "C" fn reload_project(engine: EngineHandle, project_dylib_path: *const c_char) -> i32 {
    if engine.is_null() {
        set_err("reload_project: engine is null");
        return PILL_ERR;
    }

    let r = (|| -> Result<()> {
        let runtime = unsafe { &mut *(engine as *mut Runtime) };
        let project_path = unsafe { cstr(project_dylib_path) }?.to_string();

        // Drop engine/project first then unload the lib
        runtime.shutdown_engine();
        runtime.project_library.take();

        let (project_library, project) = load_project(&project_path)?;
        runtime.project_library = Some(project_library);

        let new_engine = runtime.build_engine(project)?;
        runtime.engine = Some(new_engine);

        Ok(())
    })();

    match r {
        Ok(()) => PILL_OK,
        Err(e) => {
            set_err(format!("{e}"));
            PILL_ERR
        }
    }
}

extern "C" fn is_exit_requested(engine: EngineHandle) -> i32 {
    if engine.is_null() {
        return 0;
    }
    let runtime = unsafe { &mut *(engine as *mut Runtime) };
    match runtime.engine.as_ref() {
        Some(e) if e.is_exit_requested() => 1,
        _ => 0,
    }
}

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

#[no_mangle]
pub extern "C" fn get_pill_engine_api_v1() -> *const PillEngineApiV1 {
    &API
}
