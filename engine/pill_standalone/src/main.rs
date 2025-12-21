mod file_watcher;
use crate::file_watcher::FileWatcher;
use anyhow::{bail, Context, Ok, Result};
use config::Config;
use pill_abi::*;
use pill_core::{info, set_log_levels, warn, EngineError, LogContext, PillStyle};
use winit::{
    event::{DeviceEvent, Event, WindowEvent},
    window::Icon,
};

use libloading::{Library, Symbol};
use std::ffi::{c_void, CString};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
    time::{Duration, Instant},
};
#[cfg(target_os = "windows")]
use winit::platform::windows::IconExtWindows;

const RELOAD_COOLDOWN: Duration = Duration::from_millis(1000);
static RELOAD_GEN: AtomicU64 = AtomicU64::new(0);

struct WindowData {
    window: Arc<winit::window::Window>,
    size: winit::dpi::PhysicalSize<u32>,
    event_loop: winit::event_loop::EventLoop<()>,
}

struct FileWatchers {
    engine_core_source_files_watcher: FileWatcher,
    engine_engine_source_files_watcher: FileWatcher,
    engine_renderer_source_files_watcher: FileWatcher,
    game_dynamic_library_files_watcher: FileWatcher,
    game_project_source_files_watcher: FileWatcher,
    game_project_resources_files_watcher: FileWatcher,
}

struct ProjectPaths {
    build_data_directory_path: PathBuf,
    engine_source_directory_path: PathBuf, // TODO: what when the user just uses the precompiled
    // binary? Probably not a case tho - they they would use scripting and stuff
    game_project_directory_path: PathBuf,
    game_resources_directory_path: PathBuf,
    game_source_directory_path: PathBuf,
    config_path: PathBuf,
    engine_dynamic_library_path: PathBuf,
    game_dynamic_library_path: PathBuf,
    game_dynamic_library_hot_reloaded_path: PathBuf,
}

// --- Platform helpers ---

#[cfg(target_os = "windows")]
const DYLIB_PREFIX: &str = "";
#[cfg(not(target_os = "windows"))]
const DYLIB_PREFIX: &str = "lib";

#[cfg(target_os = "windows")]
const DYLIB_SUFFIX: &str = ".dll";
#[cfg(target_os = "linux")]
const DYLIB_SUFFIX: &str = ".so";
#[cfg(target_os = "macos")]
const DYLIB_SUFFIX: &str = ".dylib";

fn dylib(name: &str) -> String {
    format!("{DYLIB_PREFIX}{name}{DYLIB_SUFFIX}")
}

fn next_loaded_dylib_path(project_paths: &ProjectPaths) -> PathBuf {
    let gen = RELOAD_GEN.fetch_add(1, Ordering::Relaxed);
    project_paths
        .build_data_directory_path
        .join(dylib(&format!("pill_game_loaded_{gen}")))
}

fn encode_mouse_button(button: &winit::event::MouseButton) -> u32 {
    use winit::event::MouseButton::*;
    match button {
        Left => 0,
        Right => 1,
        Middle => 2,
        Back => 3,
        Forward => 4,
        Other(n) => 5u32.saturating_add(*n as u32),
    }
}

// --- Hot Reloading and Engine ABI ---

struct EngineHost {
    _lib: Library,
    api: *const PillEngineApiV1,
    handle: EngineHandle,
}

impl EngineHost {
    fn load(engine_dylib_path: &Path) -> Result<Self> {
        let lib = unsafe { Library::new(engine_dylib_path) }.with_context(|| {
            format!(
                "Failed to load engine dynamic library at {}",
                engine_dylib_path.display()
            )
        })?;

        let get_api: Symbol<unsafe extern "C" fn() -> *const PillEngineApiV1> =
            unsafe { lib.get(PILL_ENGINE_API_SYMBOL) }
                .context("Missing symbol get_pill_engine_api_v1")?;

        let api = unsafe { get_api() };
        if api.is_null() {
            bail!("pill_engine get_pill_engine_api_v1 returned null");
        }

        let a = unsafe { &*api };
        if a.abi_version != PILL_ENGINE_ABI_VERSION {
            bail!(
                "Engine ABI version mismatch engine {} host {}",
                a.abi_version,
                PILL_ENGINE_ABI_VERSION
            );
        }
        // TODO: the hash checking algo
        Ok(Self {
            _lib: lib,
            api,
            handle: std::ptr::null_mut(),
        })
    }

    fn create(&mut self, args: &PillEngineCreateArgsV1) -> Result<()> {
        let a = unsafe { &*self.api };
        let ret = (a.create)(args as *const _, &mut self.handle as *mut _);
        if ret != PILL_OK {
            let c = unsafe { std::ffi::CStr::from_ptr((a.last_error_utf8)()) };
            bail!("engine create failed: {}", c.to_string_lossy());
        }
        Ok(())
    }

    fn destroy(&mut self) {
        if self.handle.is_null() {
            return;
        }
        let a = unsafe { &*self.api };
        (a.destroy)(self.handle);
        self.handle = std::ptr::null_mut();
    }

    fn update(&mut self, dt: Duration) {
        if self.handle.is_null() {
            return;
        }
        let a = unsafe { &*self.api };
        (a.update)(self.handle, dt.as_nanos() as u64);
    }

    fn resize(&mut self, w: u32, h: u32) {
        if self.handle.is_null() {
            return;
        }
        let a = unsafe { &*self.api };
        (a.resize)(self.handle, w, h);
    }

    fn window_event(&mut self, we: &winit::event::WindowEvent) {
        if self.handle.is_null() {
            return;
        }
        let a = unsafe { &*self.api };
        (a.window_event)(self.handle, we as *const _ as *const c_void);
    }

    fn key_event(&mut self, ke: &winit::event::KeyEvent) {
        if self.handle.is_null() {
            return;
        }
        let a = unsafe { &*self.api };
        (a.key_event)(self.handle, ke as *const _ as *const c_void);
    }

    fn mouse_button(&mut self, button: u32, pressed: bool) {
        if self.handle.is_null() {
            return;
        }
        let a = unsafe { &*self.api };
        (a.mouse_button)(self.handle, button, pressed);
    }

    fn mouse_delta(&mut self, dx: f64, dy: f64) {
        if self.handle.is_null() {
            return;
        }
        let a = unsafe { &*self.api };
        (a.mouse_delta)(self.handle, dx, dy);
    }

    fn cursor_position(&mut self, x: f64, y: f64) {
        if self.handle.is_null() {
            return;
        }
        let a = unsafe { &*self.api };
        (a.cursor_position)(self.handle, x, y);
    }

    fn mouse_wheel_line(&mut self, dx: f32, dy: f32) {
        if self.handle.is_null() {
            return;
        }
        let a = unsafe { &*self.api };
        (a.mouse_wheel_line)(self.handle, dx, dy);
    }

    fn reload_game(&mut self, game_dylib_path: &Path) -> Result<()> {
        let a = unsafe { &*self.api };
        let path = CString::new(game_dylib_path.to_string_lossy().as_bytes())?;
        let ret = (a.reload_game)(self.handle, path.as_ptr());
        if ret != PILL_OK {
            let c = unsafe { std::ffi::CStr::from_ptr((a.last_error_utf8)()) };
            bail!("engine reload_game failed: {}", c.to_string_lossy());
        }
        Ok(())
    }
}

impl Drop for EngineHost {
    fn drop(&mut self) {
        self.destroy();
    }
}

fn configure_logging(config: &Config) {
    let (log_level, using_default_log_levels) = match config.get_str("LOG_LEVELS") {
        std::result::Result::Ok(val) => (val, false),
        Err(_) => {
            info!("xzxxx"); // TODO: what XD?
            (pill_core::get_default_log_levels(), true)
        }
    };

    set_log_levels(&log_level, false);

    if using_default_log_levels {
        warn!("Using default log levels: {}", log_level);
    }

    // // Configure logging
    // let log_level = config.get_str("LOG_LEVEL").unwrap_or("Info".to_string());
    // let log_level = match log_level.as_str() {
    //     "Info" => log::LevelFilter::Info,
    //     "Warning" => log::LevelFilter::Warn,
    //     "Debug" => log::LevelFilter::Debug,
    //     "Error" => log::LevelFilter::Error,
    //     "Off" => log::LevelFilter::Off,
    //     _ => log::LevelFilter::Info,
    // };

    // #[cfg(debug_assertions)]
    // env_logger::Builder::new()
    //     .format(|buf, record| {
    //         writeln!(buf, "[{}] {} {}:{}: {}",
    //             record.level(),
    //             chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
    //             record.file().unwrap_or("unknown"),
    //             record.line().unwrap_or(0),
    //             record.args()
    //         )
    //     })
    //     .filter_module("pill_core", log_level)
    //     .filter_module("pill_standalone", log_level)
    //     .filter_module("pill_engine", log_level)
    //     .filter_module("pill_renderer", log_level)
    //     .filter_module("pill_game",       log_level)
    //     .init();
}

pub fn load_window_icon(path: &Path) -> Option<Icon> {
    let image = image::open(path).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

fn create_window(config: &Config, game_resources_directory_path: PathBuf) -> WindowData {
    let window_title = config
        .get_str("WINDOW_TITLE")
        .context(EngineError::InvalidGameConfig())
        .unwrap();
    let window_width = config.get_int("WINDOW_WIDTH").unwrap_or(1280) as u32;
    let window_height = config.get_int("WINDOW_HEIGHT").unwrap_or(720) as u32;
    let window_fullscreen = config.get_bool("WINDOW_FULLSCREEN").unwrap_or(false);

    let default_icon_bytes = include_bytes!("../res/icon.raw");
    let game_icon_path = game_resources_directory_path.join("icon.ico"); // Icon has to in res folder of the game and has to be named icon.ico
    let window_icon = load_window_icon(&game_icon_path)
        .or_else(|| Icon::from_rgba(default_icon_bytes.to_vec(), 128, 128).ok());

    // Init window
    let window_event_loop = winit::event_loop::EventLoop::new().unwrap();

    // Initialize other window parameters
    let window_size = winit::dpi::PhysicalSize::<u32>::new(window_width, window_height);
    let window_min_size = winit::dpi::PhysicalSize::<u32>::new(100, 100);

    let window_attributes = winit::window::WindowAttributes::default()
        .with_title(window_title)
        .with_min_inner_size(window_min_size)
        .with_window_icon(window_icon)
        .with_visible(false);

    let window: Arc<winit::window::Window> = Arc::new(
        window_event_loop
            .create_window(window_attributes)
            .context("Failed to create window")
            .unwrap(),
    );

    // Possibly set window to fullscreen
    let window_fullscreen_mode = match window_fullscreen {
        true => {
            let monitor_handle = window.current_monitor();
            Some(winit::window::Fullscreen::Borderless(monitor_handle))
        }
        false => None,
    };
    window.set_fullscreen(window_fullscreen_mode);

    WindowData {
        window,
        event_loop: window_event_loop,
        size: window_size,
    }
}

fn ensure_launcher_binary() -> Result<PathBuf> {
    let v = std::env::var("PILL_LAUNCHER_BIN").context(
        "PILL_LAUNCHER_BIN not set (pill_launcher should set it when spawning standalone)",
    )?;
    let p = PathBuf::from(v);
    if !p.exists() {
        anyhow::bail!("PILL_LAUNCHER_BIN points to missing file: {}", p.display());
    }
    Ok(p)
}

fn build_game_hot_reload_via_launcher(project_paths: &ProjectPaths) -> Result<()> {
    let launcher_bin = ensure_launcher_binary()?;

    // output dir must be the folder containing the exe + data/
    // build_data_directory_path = <build>/data  => parent is <build>
    let out_dir = project_paths
        .build_data_directory_path
        .parent()
        .context("build_data_directory_path has no parent")?;

    let status = std::process::Command::new(&launcher_bin)
        .args([
            "-a",
            "build",
            "-p",
            project_paths.game_project_directory_path.to_str().unwrap(),
            "-c",
            "hot-reload",
            "-o",
            out_dir.to_str().unwrap(),
        ])
        .env(
            "PILL_ENGINE_WORKSPACE_DIR",
            &project_paths.engine_source_directory_path,
        ) // launcher will use this if you keep that logic
        .status()
        .context("Failed to invoke pill_launcher for hot reload")?;

    if !status.success() {
        anyhow::bail!("pill_launcher build hot-reload failed");
    }
    Ok(())
}

/// This function will reload game whenever any source file in <pill_engine_root>engine/* changes
/// It won't however reflect changes in standalone because it is the executable that is running the
/// hot-reload.
fn check_and_reload_game(
    engine_host: &mut Option<EngineHost>,
    project_paths: &ProjectPaths,
    last_reload_poll: &mut Instant,
    file_watchers: &mut FileWatchers,
) -> Result<()> {
    let now: Instant = Instant::now();

    if now.duration_since(*last_reload_poll) < RELOAD_COOLDOWN {
        return Ok(());
    }
    *last_reload_poll = Instant::now();

    let mut engine_source_changed = Vec::<PathBuf>::new();
    let mut game_source_changed = Vec::<PathBuf>::new();
    let mut game_resources_changed = Vec::<PathBuf>::new();

    // Check for engine source files changes
    if let Some(paths) = file_watchers.engine_core_source_files_watcher.get_changes() {
        info!(LogContext::HotReload => "Engine pill_core source file change detected: {:?}", paths);
        engine_source_changed.extend(paths);
    }
    if let Some(paths) = file_watchers
        .engine_engine_source_files_watcher
        .get_changes()
    {
        info!(LogContext::HotReload => "Engine pill_engine source file change detected: {:?}", paths);
        engine_source_changed.extend(paths);
    }
    if let Some(paths) = file_watchers
        .engine_renderer_source_files_watcher
        .get_changes()
    {
        info!(LogContext::HotReload => "Engine pill_renderer source file change detected: {:?}", paths);
        engine_source_changed.extend(paths);
    }

    // Game
    if let Some(paths) = file_watchers
        .game_project_resources_files_watcher
        .get_changes()
    {
        info!(LogContext::HotReload => "Game project resources file change detected: {:?}", paths);
        game_resources_changed.extend(paths);
    }
    if let Some(paths) = file_watchers
        .game_project_source_files_watcher
        .get_changes()
    {
        info!(LogContext::HotReload => "Game project source file change detected: {:?}", paths);
        game_source_changed.extend(paths);
    }

    // Resources only => no build
    if !game_resources_changed.is_empty()
        && game_source_changed.is_empty()
        && engine_source_changed.is_empty()
    {
        info!(LogContext::HotReload => "Game project resources changed (no rebuild): {:?}", game_resources_changed);
        return Ok(());
    }

    // Engine changed => prepared path, but not hot-reloading yet
    if !engine_source_changed.is_empty() {
        warn!(LogContext::HotReload =>
            "Engine changed (not hot-reloaded yet). Restart required. Changed: {:?}",
            engine_source_changed
        );
        return Ok(());
    }

    let t0 = std::time::Instant::now();
    // Game src changed => build game only
    if !game_source_changed.is_empty() {
        let t_build = std::time::Instant::now();
        build_game_hot_reload_via_launcher(project_paths)?;
        warn!("Build took: {:?} time", t_build.elapsed());
    }

    // detect change in build output
    let mut should_reload = false;
    if let Some(paths) = file_watchers
        .game_dynamic_library_files_watcher
        .get_changes()
    {
        let hot_name = dylib("pill_game_hot_reloaded");
        should_reload = paths
            .iter()
            .any(|p| p.file_name().and_then(|s| s.to_str()) == Some(&hot_name));
    }

    if should_reload {
        info!(LogContext::HotReload => "Reloading game project...");

        // Shutdown and drop engine
        // TODO: serialize here?
        // Two options:
        // - either serialize and shutown - reload the new dll and load + deserialize
        // - don't shutdown / replace the engine in memory? Not sure if can be achieved
        //   this might be nasty towards memory if user modifies significant portions of
        //   the layout of the new library. (I think it's quite unsafe to do because we
        //   have allocated specific amount of memory for the engine and then start to
        //   overwrite it randomly with a new memory layout?!)

        // Copy hot dylib to unique path (Windows-safe)
        let loaded_path = next_loaded_dylib_path(project_paths);
        std::fs::copy(
            &project_paths.game_dynamic_library_hot_reloaded_path,
            &loaded_path,
        )
        .context("Failed to copy hot-reloaded dylib to unique loaded dylib")?;

        if let Some(ref mut engine) = engine_host {
            engine.reload_game(&loaded_path)?;
        } else {
            bail!("Engine not initialized");
        }

        warn!("Reload took: {:?} time", t0.elapsed());
    }

    Ok(())
}

fn create_file_watchers(project_paths: &ProjectPaths) -> FileWatchers {
    let core_source_path = project_paths
        .engine_source_directory_path
        .join("pill_core/src");
    let engine_core_source_files_watcher = FileWatcher::new(core_source_path).set_recursive(true);
    let engine_source_path = project_paths
        .engine_source_directory_path
        .join("pill_engine/src");
    let engine_engine_source_files_watcher =
        FileWatcher::new(engine_source_path).set_recursive(true);
    let renderer_source_path = project_paths
        .engine_source_directory_path
        .join("pill_renderer/src");
    let engine_renderer_source_files_watcher =
        FileWatcher::new(renderer_source_path).set_recursive(true);

    let game_dynamic_library_files_watcher =
        FileWatcher::new(project_paths.build_data_directory_path.clone());
    let game_project_source_files_watcher =
        FileWatcher::new(project_paths.game_source_directory_path.clone()).set_recursive(true);
    let game_project_resources_files_watcher =
        FileWatcher::new(project_paths.game_resources_directory_path.clone()).set_recursive(true);

    FileWatchers {
        engine_core_source_files_watcher,
        engine_engine_source_files_watcher,
        engine_renderer_source_files_watcher, // TODO: resources as well? also track standalone?
        game_dynamic_library_files_watcher,
        game_project_source_files_watcher,
        game_project_resources_files_watcher,
    }
}

fn main_loop(
    engine_host: &mut Option<EngineHost>,
    project_paths: ProjectPaths,
    window_data: WindowData,
    config: Config,
    development_mode: bool,
) -> Result<()> {
    // Create a file watcher to monitor game project file changes as well as game output file changes
    let mut file_watchers: Option<FileWatchers> = if development_mode {
        Some(create_file_watchers(&project_paths))
    } else {
        None
    };

    let mut last_render_time = Instant::now();
    let mut last_reload_poll = Instant::now();

    // Main program loop
    let _ = window_data
        .event_loop
        .run(move |event, event_loop_window_target| {
            // Run function takes closure
            match event {
                Event::AboutToWait => {
                    window_data.window.request_redraw();
                }

                // Handle device events
                Event::DeviceEvent { ref event, .. } => {
                    if let DeviceEvent::MouseMotion { delta } = event {
                        if let Some(ref mut engine) = engine_host {
                            engine.mouse_delta(delta.0, delta.1);
                        }
                    }
                }

                // Handle window events
                Event::WindowEvent {
                    ref event,
                    window_id,
                } if window_id == window_data.window.id() => {
                    if let Some(ref mut engine) = engine_host {
                        engine.window_event(event);
                    }

                    match event {
                        WindowEvent::RedrawRequested => {
                            let now = std::time::Instant::now();
                            let delta_time = now - last_render_time;
                            last_render_time = now;

                            if let Some(ref mut engine) = engine_host {
                                engine.update(delta_time);
                            }

                            if development_mode {
                                check_and_reload_game(
                                    engine_host,
                                    &project_paths,
                                    &mut last_reload_poll,
                                    file_watchers.as_mut().unwrap(),
                                )
                                .unwrap();
                            }
                        }
                        WindowEvent::KeyboardInput { event, .. } => {
                            if let Some(ref mut engine) = engine_host {
                                engine.key_event(event);
                            }
                        }
                        WindowEvent::MouseInput { button, state, .. } => {
                            if let Some(ref mut engine) = engine_host {
                                let code = encode_mouse_button(button);
                                let pressed = *state == winit::event::ElementState::Pressed;
                                engine.mouse_button(code, pressed);
                            }
                        }
                        WindowEvent::MouseWheel { delta, .. } => {
                            if let Some(ref mut engine) = engine_host {
                                match delta {
                                    winit::event::MouseScrollDelta::LineDelta(dx, dy) => {
                                        engine.mouse_wheel_line(*dx, *dy)
                                    }
                                    _ => (),
                                }
                            }
                        }
                        WindowEvent::CursorMoved { position, .. } => {
                            if let Some(ref mut engine) = engine_host {
                                engine.cursor_position(position.x, position.y);
                            }
                        }
                        WindowEvent::Resized(physical_size) => {
                            if let Some(ref mut engine) = engine_host {
                                engine.resize(physical_size.width, physical_size.height);
                            }
                        }
                        WindowEvent::CloseRequested => {
                            drop(engine_host.take());
                            event_loop_window_target.exit();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        });

    Ok(())
}

fn main() {
    // In the development build, standalone will look for the resource files in the "res" directory of the game project directory
    // In the release build, "res" directory is copied to /build/release/data/res (TODO: pack all resources use by game into a single data file)

    // /<game_project_root>
    // ├── /build
    // │   ├── /dev
    // │   │   ├── pill_standalone.exe
    // │   │   └── /data
    // │   │       ├── pill_game.dll
    // │   │       └── pill_game_hot_reload.dll
    // │   └── /release
    // │       ├── pill_standalone.exe
    // │       └── /data
    // │           ├── /res
    // │           ├── pill_game.dll
    // │           └── pill_game_hot_reload.dll
    // ├── /src
    // ├── /res
    // │   ├── icon.raw
    // │   ├── icon.ico
    // │   └── config.ini
    // ├── Cargo.toml
    // └── Cargo.lock

    let development_mode = true;

    let current_directory_path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf(); // Path where the executable is located
    let game_project_directory_path = current_directory_path
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let build_data_directory_path = current_directory_path.join("data");
    let game_resources_directory_path = if development_mode {
        current_directory_path
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("res") // <GAME_PROJECT_ROOT>/res
    } else {
        build_data_directory_path.join("res") // <EXE_LOCATION>/data/res
    };
    let game_source_directory_path = current_directory_path
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("src"); // <GAME_PROJECT_ROOT>/src
    let config_path = game_resources_directory_path.join("config.ini");
    let engine_dynamic_library_path = build_data_directory_path.join(dylib("pill_runtime"));
    let game_dynamic_library_path = build_data_directory_path.join(dylib("pill_game"));
    let game_dynamic_library_hot_reloaded_path =
        build_data_directory_path.join(dylib("pill_game_hot_reloaded"));

    // For engine files they are under <pill_engine_root>/engine
    // Two options - some examples are nested deeper because they have sub_examples
    let engine_source_directory_path = std::env::var("PILL_ENGINE_WORKSPACE_DIR")
    .ok()
    .map(PathBuf::from)
    .unwrap_or_else(|| {
        let mut pill_engine_root = current_directory_path
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        if pill_engine_root.ends_with("Pill-Engine") {
            pill_engine_root.join("engine")
        } else {
            pill_engine_root = pill_engine_root.parent().unwrap().to_path_buf();
            if !pill_engine_root.ends_with("Pill-Engine") {
                panic!("Wrong project paths detected! Please follow proper convention when creating examples");
            }
            pill_engine_root
        }
    });

    let project_paths = ProjectPaths {
        build_data_directory_path,
        engine_source_directory_path,
        game_project_directory_path,
        game_source_directory_path,
        game_resources_directory_path,
        config_path,
        engine_dynamic_library_path,
        game_dynamic_library_path,
        game_dynamic_library_hot_reloaded_path,
    };

    // Load config
    let mut config = Config::default();
    let _ = config.merge(config::File::with_name(
        project_paths.config_path.to_str().unwrap(),
    ));

    // Configure logging context and levels
    configure_logging(&config);

    info!("Initializing {}", "Standalone".module_object_style());

    // Create windows
    let window_data = create_window(&config, project_paths.game_resources_directory_path.clone());

    // Load engine dynamic Library
    let mut engine_host = EngineHost::load(&project_paths.engine_dynamic_library_path).unwrap();

    // Create cstring paths
    let game_dylib_path_c = CString::new(
        project_paths
            .game_dynamic_library_path
            .to_string_lossy()
            .as_bytes(),
    )
    .unwrap();
    let resource_path_c = CString::new(
        project_paths
            .game_resources_directory_path
            .to_string_lossy()
            .as_bytes(),
    )
    .unwrap();
    let config_path_c =
        CString::new(project_paths.config_path.to_string_lossy().as_bytes()).unwrap();

    // Pass one cloned Arc<Window> ref and let engine take the ownership via Arc::from_raw
    let window_raw = Arc::into_raw(Arc::clone(&window_data.window)) as *const c_void;

    let args = PillEngineCreateArgsV1 {
        struct_size: std::mem::size_of::<PillEngineCreateArgsV1>() as u32,
        game_dylib_path: game_dylib_path_c.as_ptr(),
        game_resources_dir: resource_path_c.as_ptr(),
        config_path: config_path_c.as_ptr(),
        window_ptr: window_raw,
        initial_w: window_data.size.width,
        initial_h: window_data.size.height,
    };

    if let Err(e) = engine_host.create(&args) {
        panic!("EngineHost.create failed");
    }

    // Run loop
    window_data
        .event_loop
        .set_control_flow(winit::event_loop::ControlFlow::Poll);
    window_data
        .event_loop
        .set_control_flow(winit::event_loop::ControlFlow::Wait);

    // Show window (now the taskbar icon will be set correctly)
    window_data.window.set_visible(true);

    // Main program loop
    main_loop(
        &mut Some(engine_host),
        project_paths,
        window_data,
        config,
        development_mode,
    )
    .context("Main loop failed")
    .unwrap();
}
