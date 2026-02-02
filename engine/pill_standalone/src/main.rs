mod file_watcher;
use crate::file_watcher::FileWatcher;
use anyhow::{bail, Context, Result};
use config::Config;
use pill_abi::*;
use pill_core::{info, set_log_levels, warn, LogContext, PillStyle};
use winit::{
    event::{DeviceEvent, Event, WindowEvent},
    window::{self, Icon},
};

use libloading::{Library, Symbol};
use std::ffi::{c_void, CString, OsString};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::Arc,
    time::{Duration, Instant},
};
#[cfg(target_os = "windows")]
use winit::platform::windows::IconExtWindows;

const RELOAD_COOLDOWN: Duration = Duration::from_millis(100);
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
    dynamic_libraries_files_watcher: FileWatcher,
    game_project_source_files_watcher: FileWatcher,
    game_project_resources_files_watcher: FileWatcher,
}

struct ProjectPaths {
    build_data_directory_path: PathBuf,
    engine_source_directory_path: PathBuf, // TODO: what when the user just uses the precompiled
    game_project_directory_path: PathBuf,
    game_resources_directory_path: PathBuf,
    game_source_directory_path: PathBuf,
    config_path: PathBuf,

    runtime_dynamic_library_path: PathBuf,
    runtime_dynamic_library_hot_reloaded_path: PathBuf,

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeLoadMode {
    Dylib,
    InProcess,
}

fn parse_runtime_load_mode(value: Option<String>) -> Option<RuntimeLoadMode> {
    match value.as_deref() {
        Some("dylib") => Some(RuntimeLoadMode::Dylib),
        Some("in_process") => Some(RuntimeLoadMode::InProcess),
        _ => None,
    }
}

fn workspace_includes_game(
    engine_source_directory_path: &Path,
    game_project_directory_path: &Path,
) -> bool {
    let cargo_toml_path = engine_source_directory_path.join("Cargo.toml");
    let Ok(contents) = std::fs::read_to_string(cargo_toml_path) else {
        return false;
    };
    let Some(game_dir_name) = game_project_directory_path
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    contents.contains(game_dir_name)
}

fn engine_workspace_from_game_manifest(game_project_directory_path: &Path) -> Option<PathBuf> {
    // KISS parser: we only need `workspace = "/abs/path/to/engine"` from the game's Cargo.toml.
    // If it's not present or malformed, just fall back to the existing heuristics/env.
    let manifest_path = game_project_directory_path.join("Cargo.toml");
    let contents = std::fs::read_to_string(manifest_path).ok()?;
    for line in contents.lines() {
        let line = line.trim();
        if !line.starts_with("workspace") {
            continue;
        }
        let (_, rhs) = line.split_once('=')?;
        let rhs = rhs.trim();
        // Accept only the common `workspace = "..."` form.
        let rhs = rhs.strip_prefix('"')?;
        let rhs = rhs.strip_suffix('"')?;
        let p = PathBuf::from(rhs);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn find_engine_source_directory(
    current_directory_path: &Path,
    game_project_directory_path: &Path,
) -> Option<PathBuf> {
    for ancestor in current_directory_path.ancestors() {
        let engine_candidate = ancestor.join("engine");
        if engine_candidate
            .join("pill_engine")
            .join("Cargo.toml")
            .exists()
        {
            return Some(engine_candidate);
        }
        if ancestor.join("pill_engine").join("Cargo.toml").exists() {
            return Some(ancestor.to_path_buf());
        }
    }

    if let Some(parent) = game_project_directory_path.parent() {
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let engine_candidate = path.join("engine");
                if engine_candidate
                    .join("pill_engine")
                    .join("Cargo.toml")
                    .exists()
                {
                    return Some(engine_candidate);
                }
                if path.join("pill_engine").join("Cargo.toml").exists() {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn resolve_runtime_dylib_candidates(
    build_data_directory_path: &Path,
    engine_source_directory_path: &Path,
    name: &str,
) -> Vec<PathBuf> {
    let engine_workspace_root = engine_source_directory_path.parent().unwrap();
    vec![
        build_data_directory_path.join(dylib(name)),
        engine_workspace_root
            .join("target")
            .join("debug")
            .join(dylib(name)),
        engine_workspace_root
            .join("target")
            .join("release")
            .join(dylib(name)),
        engine_source_directory_path
            .join("target")
            .join("debug")
            .join(dylib(name)),
        engine_source_directory_path
            .join("target")
            .join("release")
            .join(dylib(name)),
    ]
}

fn resolve_runtime_dylib(
    build_data_directory_path: &Path,
    engine_source_directory_path: &Path,
    name: &str,
) -> PathBuf {
    let candidates = resolve_runtime_dylib_candidates(
        build_data_directory_path,
        engine_source_directory_path,
        name,
    );

    for candidate in &candidates {
        if candidate.exists() {
            return candidate.clone();
        }
    }

    let candidates_display = candidates
        .iter()
        .map(|candidate| candidate.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    panic!("Failed to find {name} runtime dylib. Checked: {candidates_display}");
}

fn resolve_runtime_dylib_optional(
    build_data_directory_path: &Path,
    engine_source_directory_path: &Path,
    name: &str,
) -> Option<PathBuf> {
    let candidates = resolve_runtime_dylib_candidates(
        build_data_directory_path,
        engine_source_directory_path,
        name,
    );
    for candidate in candidates {
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn next_loaded_runtime_dylib_path(project_paths: &ProjectPaths) -> PathBuf {
    let gen = RELOAD_GEN.fetch_add(1, Ordering::Relaxed);
    project_paths
        .build_data_directory_path
        .join(dylib(&format!("pill_runtime_loaded_{gen}")))
}

fn next_loaded_game_dylib_path(project_paths: &ProjectPaths) -> PathBuf {
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

struct RuntimeCreateContext {
    game_dylib_path: CString,
    game_resources_dir: CString,
    config_path: CString,
    window: Arc<window::Window>,
    initial_w: u32,
    initial_h: u32,
}

impl RuntimeCreateContext {
    fn make_args(&self) -> PillEngineCreateArgsV1 {
        // Pass one cloned Arc<Window> ref and let runtime take the ownership via Arc::from_raw
        let window_raw = Arc::into_raw(Arc::clone(&self.window)) as *const c_void;
        PillEngineCreateArgsV1 {
            struct_size: std::mem::size_of::<PillEngineCreateArgsV1>() as u32,
            window_ptr: window_raw,
            game_dylib_path: self.game_dylib_path.as_ptr(),
            game_resources_dir: self.game_resources_dir.as_ptr(),
            config_path: self.config_path.as_ptr(),
            initial_w: self.initial_w,
            initial_h: self.initial_h,
        }
    }
}

struct RuntimeHost {
    _lib: Option<Library>,
    api: *const PillEngineApiV1,
    handle: EngineHandle,
}

impl RuntimeHost {
    fn load(runtime_dylib_path: &Path, load_mode: RuntimeLoadMode) -> Result<Self> {
        if load_mode == RuntimeLoadMode::InProcess {
            let api = pill_runtime::get_pill_engine_api_v1();
            if api.is_null() {
                bail!("pill_runtime get_pill_engine_api_v1 returned null");
            }
            let a = unsafe { &*api };
            if a.abi_version != PILL_ENGINE_ABI_VERSION {
                bail!(
                    "Engine ABI version mismatch runtime {} host {}",
                    a.abi_version,
                    PILL_ENGINE_ABI_VERSION
                );
            }
            return Ok(Self {
                _lib: None,
                api,
                handle: std::ptr::null_mut(),
            });
        }

        let lib = unsafe { Library::new(runtime_dylib_path) }.with_context(|| {
            format!(
                "Failed to load runtime dynamic library at {}",
                runtime_dylib_path.display()
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
                "Engine ABI version mismatch runtime {} host {}",
                a.abi_version,
                PILL_ENGINE_ABI_VERSION
            );
        }
        // TODO: the hash checking algo
        Ok(Self {
            _lib: Some(lib),
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

impl Drop for RuntimeHost {
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
        .or_else(|_| config.get_str("TITLE"))
        .unwrap_or_default();
    let window_size_override = match (
        config.get_int("WINDOW_WIDTH"),
        config.get_int("WINDOW_HEIGHT"),
    ) {
        (Ok(width), Ok(height)) => Some(winit::dpi::PhysicalSize::<u32>::new(
            width as u32,
            height as u32,
        )),
        _ => None,
    };
    let window_fullscreen = config.get_bool("WINDOW_FULLSCREEN").unwrap_or(false);

    let default_icon_bytes = include_bytes!("../res/icon.raw");
    let game_icon_path = game_resources_directory_path.join("icon.ico"); // Icon has to in res folder of the game and has to be named icon.ico
    let window_icon = load_window_icon(&game_icon_path)
        .or_else(|| Icon::from_rgba(default_icon_bytes.to_vec(), 128, 128).ok());

    // Init window
    let window_event_loop = winit::event_loop::EventLoop::new().unwrap();

    // Initialize other window parameters
    let window_min_size = winit::dpi::PhysicalSize::<u32>::new(100, 100);

    let window_attributes = winit::window::WindowAttributes::default()
        .with_title(window_title)
        .with_min_inner_size(window_min_size)
        .with_window_icon(window_icon)
        .with_visible(false);

    let window_attributes = if let Some(size) = window_size_override {
        window_attributes.with_inner_size(size)
    } else {
        window_attributes
    };

    let window: Arc<winit::window::Window> = Arc::new(
        window_event_loop
            .create_window(window_attributes)
            .context("Failed to create window")
            .unwrap(),
    );
    let window_size = window.inner_size();

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

fn resolve_launcher_command(engine_source_directory_path: &Path) -> Result<OsString> {
    if let Ok(v) = std::env::var("PILL_LAUNCHER_BIN") {
        let p = PathBuf::from(v);
        if !p.exists() {
            anyhow::bail!("PILL_LAUNCHER_BIN points to missing file: {}", p.display());
        }
        return Ok(p.into_os_string());
    }

    if let Ok(v) = std::env::var("PILL_LAUNCHER_CMD") {
        return Ok(OsString::from(v));
    }

    // `pill_launcher` is excluded from the workspace, so its build output lives under
    // <engine>/pill_launcher/target/... rather than <workspace_root>/target/...
    let launcher_candidates = [
        engine_source_directory_path
            .join("pill_launcher")
            .join("target")
            .join("debug")
            .join("PillLauncher"),
        engine_source_directory_path
            .join("pill_launcher")
            .join("target")
            .join("release")
            .join("PillLauncher"),
        engine_source_directory_path
            .join("pill_launcher")
            .join("target")
            .join("debug")
            .join("PillLauncherUpstream"),
        engine_source_directory_path
            .join("pill_launcher")
            .join("target")
            .join("release")
            .join("PillLauncherUpstream"),
    ];
    for candidate in launcher_candidates {
        if candidate.exists() {
            return Ok(candidate.into_os_string());
        }
    }

    Ok(OsString::from("PillLauncherUpstream"))
}

fn build_hot_reload_via_launcher(project_paths: &ProjectPaths) -> Result<()> {
    let launcher_cmd = resolve_launcher_command(&project_paths.engine_source_directory_path)?;

    // output dir must be the folder containing the exe + data/
    // build_data_directory_path = <build>/data  => parent is <build>
    let out_dir = project_paths
        .build_data_directory_path
        .parent()
        .context("build_data_directory_path has no parent")?;

    let args = [
        "-a",
        "build",
        "-p",
        project_paths.game_project_directory_path.to_str().unwrap(),
        "-c",
        "hot-reload",
        "-o",
        out_dir.to_str().unwrap(),
    ];

    let status = std::process::Command::new(&launcher_cmd)
        .args(args)
        .env("PILL_HOT_RELOAD_CHILD", "1")
        .env(
            "PILL_ENGINE_WORKSPACE_DIR",
            &project_paths.engine_source_directory_path,
        ) // launcher will use this if you keep that logic
        .status();

    let status = match status {
        Ok(status) => status,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let manifest = project_paths
                .engine_source_directory_path
                .join("pill_launcher")
                .join("Cargo.toml");
            std::process::Command::new("cargo")
                .args(["run", "--manifest-path", manifest.to_str().unwrap(), "--"])
                .args(args)
                .env("PILL_HOT_RELOAD_CHILD", "1")
                .env(
                    "PILL_ENGINE_WORKSPACE_DIR",
                    &project_paths.engine_source_directory_path,
                )
                .status()
                .context("Failed to invoke pill_launcher via cargo for hot reload")?
        }
        Err(err) => return Err(err).context("Failed to invoke pill_launcher for hot reload"),
    };

    if !status.success() {
        anyhow::bail!("pill_launcher build hot-reload failed");
    }
    Ok(())
}

/// This function will reload engine or the entire runtime depending on files in <pill_engine_root>engine/* changes
/// It won't however reflect changes in standalone because it is the executable that is running the hot-reload.
fn check_and_reload(
    runtime_host: &mut Option<RuntimeHost>,
    runtime_context: &RuntimeCreateContext,
    project_paths: &ProjectPaths,
    last_reload_poll: &mut Instant,
    file_watchers: &mut FileWatchers,
    runtime_load_mode: RuntimeLoadMode,
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

    let t0 = std::time::Instant::now();
    // Game or Engine src changed => build
    if !game_source_changed.is_empty() || !engine_source_changed.is_empty() {
        build_hot_reload_via_launcher(project_paths)?;
        warn!("Build took: {:?} time", t0.elapsed());
    }

    // detect change in build output
    let mut runtime_hot_reload = false;
    let mut game_hot_reload = false;
    if let Some(paths) = file_watchers.dynamic_libraries_files_watcher.get_changes() {
        let game_hot_name = dylib("pill_game_hot_reloaded");
        let runtime_hot_name = dylib("pill_runtime_hot_reloaded");
        for path in paths {
            let filename = path.file_name().and_then(|s| s.to_str());
            if filename == Some(&runtime_hot_name) {
                runtime_hot_reload = true;
            } else if filename == Some(&game_hot_name) {
                game_hot_reload = true;
            }
        }
    }

    // Do a runtime reload when both have changed
    if runtime_hot_reload && runtime_load_mode == RuntimeLoadMode::InProcess {
        warn!(LogContext::HotReload => "Runtime hot-reload skipped for in-process runtime.");
        runtime_hot_reload = false;
    }

    if runtime_hot_reload {
        info!(LogContext::HotReload => "Reloading runtime (engine hot-reload)...");
        let t_runtime_reload = std::time::Instant::now();

        // Drop the old runtime
        drop(runtime_host.take());

        // Copy hot dylib to unique path (Windows-safe)
        let loaded_path = next_loaded_runtime_dylib_path(project_paths);
        std::fs::copy(
            &project_paths.runtime_dynamic_library_hot_reloaded_path,
            &loaded_path,
        )
        .context("Failed to copy hot-reloaded dylib to unique loaded dylib")?;

        // Reload runtime dynamic Library
        let mut new_runtime = RuntimeHost::load(&loaded_path, runtime_load_mode)?;
        let args = runtime_context.make_args();
        new_runtime.create(&args)?;

        *runtime_host = Some(new_runtime);

        warn!("Runtime reload took: {:?} time", t_runtime_reload.elapsed());
        warn!("Total reload took: {:?} time", t0.elapsed());
    } else if game_hot_reload {
        // Game-only hot-reload
        info!(LogContext::HotReload => "Reloading game project...");
        let t_game_reload = std::time::Instant::now();

        // Shutdown and drop runtime
        // TODO: serialize here?
        // Two options:
        // - either serialize and shutown - reload the new dll and load + deserialize
        // - don't shutdown / replace the runtime in memory? Not sure if can be achieved
        //   this might be nasty towards memory if user modifies significant portions of
        //   the layout of the new library. (I think it's quite unsafe to do because we
        //   have allocated specific amount of memory for the runtime and then start to
        //   overwrite it randomly with a new memory layout?!)

        // Copy hot dylib to unique path (Windows-safe)
        let loaded_path = next_loaded_game_dylib_path(project_paths);
        std::fs::copy(
            &project_paths.game_dynamic_library_hot_reloaded_path,
            &loaded_path,
        )
        .context("Failed to copy hot-reloaded dylib to unique loaded dylib")?;

        if let Some(ref mut runtime) = runtime_host {
            runtime.reload_game(&loaded_path)?;
        } else {
            bail!("Engine not initialized");
        }

        warn!("Game hot-reload took: {:?} time", t_game_reload.elapsed());
        warn!("Total reload took: {:?} time", t0.elapsed());
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

    let dynamic_libraries_files_watcher =
        FileWatcher::new(project_paths.build_data_directory_path.clone());
    let game_project_source_files_watcher =
        FileWatcher::new(project_paths.game_source_directory_path.clone()).set_recursive(true);
    let game_project_resources_files_watcher =
        FileWatcher::new(project_paths.game_resources_directory_path.clone()).set_recursive(true);

    FileWatchers {
        engine_core_source_files_watcher,
        engine_engine_source_files_watcher,
        engine_renderer_source_files_watcher, // TODO: resources as well? also track standalone?
        dynamic_libraries_files_watcher,
        game_project_source_files_watcher,
        game_project_resources_files_watcher,
    }
}

fn main_loop(
    runtime_host: &mut Option<RuntimeHost>,
    runtime_context: RuntimeCreateContext,
    project_paths: ProjectPaths,
    window_data: WindowData,
    hot_reload_enabled: bool,
    runtime_load_mode: RuntimeLoadMode,
) -> Result<()> {
    // Create a file watcher to monitor game project file changes as well as game output file changes
    let mut file_watchers: Option<FileWatchers> = if hot_reload_enabled {
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
                        if let Some(ref mut runtime) = runtime_host {
                            runtime.mouse_delta(delta.0, delta.1);
                        }
                    }
                }

                // Handle window events
                Event::WindowEvent {
                    ref event,
                    window_id,
                } if window_id == window_data.window.id() => {
                    if let Some(ref mut runtime) = runtime_host {
                        runtime.window_event(event);
                    }

                    match event {
                        WindowEvent::RedrawRequested => {
                            let now = std::time::Instant::now();
                            let delta_time = now - last_render_time;
                            last_render_time = now;

                            if let Some(ref mut runtime) = runtime_host {
                                runtime.update(delta_time);
                            }

                            if hot_reload_enabled {
                                check_and_reload(
                                    runtime_host,
                                    &runtime_context,
                                    &project_paths,
                                    &mut last_reload_poll,
                                    file_watchers.as_mut().unwrap(),
                                    runtime_load_mode,
                                )
                                .unwrap();
                            }
                        }
                        WindowEvent::KeyboardInput { event, .. } => {
                            if let Some(ref mut runtime) = runtime_host {
                                runtime.key_event(event);
                            }
                        }
                        WindowEvent::MouseInput { button, state, .. } => {
                            if let Some(ref mut runtime) = runtime_host {
                                let code = encode_mouse_button(button);
                                let pressed = *state == winit::event::ElementState::Pressed;
                                runtime.mouse_button(code, pressed);
                            }
                        }
                        WindowEvent::MouseWheel { delta, .. } => {
                            if let Some(ref mut runtime) = runtime_host {
                                if let winit::event::MouseScrollDelta::LineDelta(dx, dy) = delta {
                                    runtime.mouse_wheel_line(*dx, *dy)
                                }
                            }
                        }
                        WindowEvent::CursorMoved { position, .. } => {
                            if let Some(ref mut runtime) = runtime_host {
                                runtime.cursor_position(position.x, position.y);
                            }
                        }
                        WindowEvent::Resized(physical_size) => {
                            if let Some(ref mut runtime) = runtime_host {
                                runtime.resize(physical_size.width, physical_size.height);
                            }
                        }
                        WindowEvent::CloseRequested => {
                            drop(runtime_host.take());
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

    let mut hot_reload_enabled =
        std::env::var("PILL_ENABLE_HOT_RELOAD").ok().as_deref() == Some("1");

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
    let project_resources_directory_path = current_directory_path
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("res"); // <GAME_PROJECT_ROOT>/res
    let build_resources_directory_path = build_data_directory_path.join("res"); // <EXE_LOCATION>/data/res
    let game_resources_directory_path = if hot_reload_enabled {
        project_resources_directory_path
    } else if build_resources_directory_path.exists() {
        build_resources_directory_path
    } else {
        project_resources_directory_path
    };
    let game_source_directory_path = current_directory_path
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("src"); // <GAME_PROJECT_ROOT>/src
    let config_path = game_resources_directory_path.join("config.ini");
    let game_dynamic_library_path = build_data_directory_path.join(dylib("pill_game"));
    let game_dynamic_library_hot_reloaded_path =
        build_data_directory_path.join(dylib("pill_game_hot_reloaded"));

    // For engine files they are under <engine_root>/engine or <engine_root>.
    let engine_source_directory_path = engine_workspace_from_game_manifest(&game_project_directory_path)
        .filter(|candidate| workspace_includes_game(candidate, &game_project_directory_path))
        .or_else(|| {
            std::env::var("PILL_ENGINE_WORKSPACE_DIR")
                .ok()
                .map(PathBuf::from)
                .filter(|candidate| workspace_includes_game(candidate, &game_project_directory_path))
        })
        .or_else(|| {
            find_engine_source_directory(&current_directory_path, &game_project_directory_path)
        })
        .filter(|candidate| workspace_includes_game(candidate, &game_project_directory_path))
        .unwrap_or_else(|| {
            panic!("Engine workspace not detected for this game. Set PILL_ENGINE_WORKSPACE_DIR to the engine directory that includes the game workspace member.");
        });

    let runtime_load_mode = parse_runtime_load_mode(std::env::var("PILL_RUNTIME_MODE").ok())
        .or_else(|| {
            (std::env::var("PILL_RUNTIME_IN_PROCESS").ok().as_deref() == Some("1"))
                .then_some(RuntimeLoadMode::InProcess)
        })
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                RuntimeLoadMode::InProcess
            } else {
                RuntimeLoadMode::Dylib
            }
        });

    // Runtime dylibs: only required in dylib mode.
    let runtime_dynamic_library_path = if runtime_load_mode == RuntimeLoadMode::Dylib {
        resolve_runtime_dylib(
            &build_data_directory_path,
            &engine_source_directory_path,
            "pill_runtime",
        )
    } else {
        build_data_directory_path.join(dylib("pill_runtime"))
    };
    let runtime_dynamic_library_hot_reloaded_path =
        if hot_reload_enabled && runtime_load_mode == RuntimeLoadMode::Dylib {
            resolve_runtime_dylib_optional(
                &build_data_directory_path,
                &engine_source_directory_path,
                "pill_runtime_hot_reloaded",
            )
            .unwrap_or_else(|| {
                hot_reload_enabled = false;
                runtime_dynamic_library_path.clone()
            })
        } else {
            runtime_dynamic_library_path.clone()
        };

    let project_paths = ProjectPaths {
        build_data_directory_path,
        engine_source_directory_path,
        game_project_directory_path,
        game_source_directory_path,
        game_resources_directory_path,
        config_path,
        runtime_dynamic_library_path,
        runtime_dynamic_library_hot_reloaded_path,
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

    info!(
        LogContext::HotReload => "Hot reload {} (watching src: {}, res: {})",
        if hot_reload_enabled { "enabled" } else { "disabled" },
        project_paths.game_source_directory_path.display(),
        project_paths.game_resources_directory_path.display()
    );

    info!("Initializing {}", "Standalone".module_object_style());

    // Create windows
    let window_data = create_window(&config, project_paths.game_resources_directory_path.clone());

    // Load runtime dynamic Library
    let mut runtime_host = RuntimeHost::load(
        &project_paths.runtime_dynamic_library_path,
        runtime_load_mode,
    )
    .unwrap();

    // Save the context data for future reloads
    // window::Window will be leaked to the runtime DLL every time make_args is called
    let runtime_context = RuntimeCreateContext {
        game_dylib_path: CString::new(
            project_paths
                .game_dynamic_library_path
                .to_string_lossy()
                .as_bytes(),
        )
        .unwrap(),
        game_resources_dir: CString::new(
            project_paths
                .game_resources_directory_path
                .to_string_lossy()
                .as_bytes(),
        )
        .unwrap(),
        config_path: CString::new(project_paths.config_path.to_string_lossy().as_bytes()).unwrap(),
        window: Arc::clone(&window_data.window),
        initial_w: window_data.size.width,
        initial_h: window_data.size.height,
    };

    let args = runtime_context.make_args();

    if let Err(e) = runtime_host.create(&args) {
        panic!("RuntimeHost.create failed {e:#}");
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
        &mut Some(runtime_host),
        runtime_context,
        project_paths,
        window_data,
        hot_reload_enabled,
        runtime_load_mode,
    )
    .context("Main loop failed")
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_tmp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("pill_test_{name}_{nanos}"))
    }

    #[test]
    fn prefers_game_manifest_workspace_over_env_and_sibling_scan() {
        let root = unique_tmp_dir("hot_reload_workspace_pick");
        let _ = fs::remove_dir_all(&root);

        let game_dir = root.join("my_game");
        fs::create_dir_all(game_dir.join("src")).unwrap();
        fs::create_dir_all(game_dir.join("res")).unwrap();

        let engine_a = root.join("Pill-Engine").join("engine");
        let engine_b = root.join("Pill-Engine-Upstream").join("engine");
        fs::create_dir_all(engine_a.join("pill_engine")).unwrap();
        fs::create_dir_all(engine_b.join("pill_engine")).unwrap();

        // Both "workspaces" include the game dir name to satisfy `workspace_includes_game`.
        fs::write(
            engine_a.join("Cargo.toml"),
            r#"[workspace]
members = ["my_game"]
"#,
        )
        .unwrap();
        fs::write(
            engine_b.join("Cargo.toml"),
            r#"[workspace]
members = ["my_game"]
"#,
        )
        .unwrap();

        // Game manifest explicitly points at engine_b.
        fs::write(
            game_dir.join("Cargo.toml"),
            format!(
                r#"[package]
name = "my_game"
version = "0.1.0"
edition = "2021"
workspace = "{}"
"#,
                engine_b.display()
            ),
        )
        .unwrap();

        // Even if env points at the "wrong" engine, we should use the game manifest.
        std::env::set_var("PILL_ENGINE_WORKSPACE_DIR", &engine_a);
        let resolved = engine_workspace_from_game_manifest(&game_dir)
            .filter(|p| workspace_includes_game(p, &game_dir))
            .unwrap();
        assert_eq!(resolved, engine_b);
    }

    #[test]
    fn resolve_launcher_prefers_engine_pill_launcher_target_binary() {
        let root = unique_tmp_dir("hot_reload_launcher_pick");
        let _ = fs::remove_dir_all(&root);

        let engine_dir = root.join("engine");
        let launcher_bin = engine_dir
            .join("pill_launcher")
            .join("target")
            .join("debug")
            .join("PillLauncher");
        fs::create_dir_all(launcher_bin.parent().unwrap()).unwrap();
        fs::write(&launcher_bin, b"").unwrap();

        std::env::remove_var("PILL_LAUNCHER_BIN");
        std::env::remove_var("PILL_LAUNCHER_CMD");

        let resolved = resolve_launcher_command(&engine_dir).unwrap();
        assert_eq!(PathBuf::from(resolved), launcher_bin);
    }
}
