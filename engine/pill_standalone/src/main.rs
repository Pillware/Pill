mod file_watcher;

use crate::file_watcher::FileWatcher;
use anyhow::{bail, Context, Result};
use config::Config;
use libloading::{Library, Symbol};
use pill_abi::*;
use pill_core::{info, set_log_levels, warn, LogContext, PillStyle};
use std::ffi::{c_void, CString, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Fullscreen, Icon, Window, WindowAttributes},
};

const RELOAD_COOLDOWN: Duration = Duration::from_millis(1000);
static RELOAD_GEN: AtomicU64 = AtomicU64::new(0);

struct WindowInit {
    attributes: WindowAttributes,
    fullscreen: bool,
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
    engine_source_directory_path: Option<PathBuf>,
    game_project_directory_path: PathBuf,
    game_resources_directory_path: PathBuf,
    game_source_directory_path: PathBuf,
    config_path: PathBuf,
    runtime_dynamic_library_path: PathBuf,
    runtime_dynamic_library_hot_reloaded_path: PathBuf,
    game_dynamic_library_path: PathBuf,
    game_dynamic_library_hot_reloaded_path: PathBuf,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunLayout {
    Development,
    Packaged,
}

fn game_project_exists(path: &Path) -> bool {
    path.join("Cargo.toml").exists()
        && path.join("res").join("config.ini").exists()
        && path.join("src").exists()
}

fn infer_game_project_directory(current_directory_path: &Path) -> Result<PathBuf> {
    if let Ok(value) = std::env::var("PILL_GAME_PROJECT_DIR") {
        let path = PathBuf::from(value);
        if game_project_exists(&path) {
            return Ok(path);
        }
        bail!(
            "PILL_GAME_PROJECT_DIR was set but {} is not a valid game project",
            path.display()
        );
    }

    current_directory_path
        .parent()
        .context("Build directory has no parent")?
        .parent()
        .context("Game project directory resolution failed")
        .map(Path::to_path_buf)
}

fn resolve_run_layout(game_project_directory_path: &Path) -> RunLayout {
    match std::env::var("PILL_STANDALONE_LAYOUT").ok().as_deref() {
        Some("development") => RunLayout::Development,
        Some("packaged") => RunLayout::Packaged,
        _ if game_project_exists(game_project_directory_path) => RunLayout::Development,
        _ => RunLayout::Packaged,
    }
}

fn workspace_includes_game(
    engine_source_directory_path: &Path,
    game_project_directory_path: &Path,
) -> bool {
    let cargo_toml_path = engine_source_directory_path.join("Cargo.toml");
    let Ok(contents) = fs::read_to_string(cargo_toml_path) else {
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

fn looks_like_engine_workspace(path: &Path) -> bool {
    path.join("pill_core").exists()
        && path.join("pill_engine").exists()
        && path.join("pill_renderer").exists()
}

fn engine_workspace_from_game_manifest(game_project_directory_path: &Path) -> Option<PathBuf> {
    let manifest_path = game_project_directory_path.join("Cargo.toml");
    let contents = fs::read_to_string(manifest_path).ok()?;

    for line in contents.lines() {
        let line = line.trim();
        if !line.starts_with("workspace") {
            continue;
        }
        let (_, rhs) = line.split_once('=')?;
        let rhs = rhs.trim().strip_prefix('"')?.strip_suffix('"')?;
        let path = PathBuf::from(rhs);
        if path.exists() {
            return Some(path);
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
        if looks_like_engine_workspace(&engine_candidate)
            || engine_candidate
                .join("pill_engine")
                .join("Cargo.toml")
                .exists()
        {
            return Some(engine_candidate);
        }

        if looks_like_engine_workspace(ancestor)
            || ancestor.join("pill_engine").join("Cargo.toml").exists()
        {
            return Some(ancestor.to_path_buf());
        }
    }

    if let Some(parent) = game_project_directory_path.parent() {
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let engine_candidate = path.join("engine");
                if looks_like_engine_workspace(&engine_candidate)
                    || engine_candidate
                        .join("pill_engine")
                        .join("Cargo.toml")
                        .exists()
                {
                    return Some(engine_candidate);
                }

                if looks_like_engine_workspace(&path)
                    || path.join("pill_engine").join("Cargo.toml").exists()
                {
                    return Some(path);
                }
            }
        }
    }

    None
}

fn resolve_engine_workspace_dir(
    current_directory_path: &Path,
    game_project_directory_path: &Path,
    require_workspace_membership: bool,
) -> Result<PathBuf> {
    let by_manifest = engine_workspace_from_game_manifest(game_project_directory_path);
    let by_env = std::env::var("PILL_ENGINE_WORKSPACE_DIR")
        .ok()
        .map(PathBuf::from);
    let by_scan = find_engine_source_directory(current_directory_path, game_project_directory_path);

    for candidate in [by_manifest, by_env, by_scan].into_iter().flatten() {
        if !looks_like_engine_workspace(&candidate) && !candidate.join("pill_engine").exists() {
            continue;
        }

        if require_workspace_membership
            && !workspace_includes_game(&candidate, game_project_directory_path)
        {
            continue;
        }

        return Ok(candidate);
    }

    bail!(
        "Engine workspace not detected. Set PILL_ENGINE_WORKSPACE_DIR to the engine directory{}.",
        if require_workspace_membership {
            " that includes the game workspace member"
        } else {
            ""
        }
    )
}

fn resolve_runtime_dylib_candidates(
    build_data_directory_path: &Path,
    engine_source_directory_path: Option<&Path>,
    name: &str,
) -> Vec<PathBuf> {
    let mut candidates = vec![build_data_directory_path.join(dylib(name))];

    if let Some(engine_source_directory_path) = engine_source_directory_path {
        if let Some(engine_workspace_root) = engine_source_directory_path.parent() {
            candidates.extend([
                engine_workspace_root
                    .join("target")
                    .join("debug")
                    .join(dylib(name)),
                engine_workspace_root
                    .join("target")
                    .join("release")
                    .join(dylib(name)),
            ]);
        }

        candidates.extend([
            engine_source_directory_path
                .join("target")
                .join("debug")
                .join(dylib(name)),
            engine_source_directory_path
                .join("target")
                .join("release")
                .join(dylib(name)),
        ]);
    }

    candidates
}

fn resolve_runtime_dylib(
    build_data_directory_path: &Path,
    engine_source_directory_path: Option<&Path>,
    name: &str,
) -> Result<PathBuf> {
    let candidates = resolve_runtime_dylib_candidates(
        build_data_directory_path,
        engine_source_directory_path,
        name,
    );

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    let candidates_display = candidates
        .iter()
        .map(|candidate| candidate.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    bail!("Failed to find {name} runtime dylib. Checked: {candidates_display}")
}

fn resolve_runtime_dylib_optional(
    build_data_directory_path: &Path,
    engine_source_directory_path: Option<&Path>,
    name: &str,
) -> Option<PathBuf> {
    resolve_runtime_dylib_candidates(
        build_data_directory_path,
        engine_source_directory_path,
        name,
    )
    .into_iter()
    .find(|candidate| candidate.exists())
}

fn next_loaded_runtime_dylib_path(project_paths: &ProjectPaths) -> PathBuf {
    let generation = RELOAD_GEN.fetch_add(1, Ordering::Relaxed);
    project_paths
        .build_data_directory_path
        .join(dylib(&format!("pill_runtime_loaded_{generation}")))
}

fn next_loaded_game_dylib_path(project_paths: &ProjectPaths) -> PathBuf {
    let generation = RELOAD_GEN.fetch_add(1, Ordering::Relaxed);
    project_paths
        .build_data_directory_path
        .join(dylib(&format!("pill_game_loaded_{generation}")))
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

struct RuntimeCreateContext {
    game_resources_dir: CString,
    config_path: CString,
    window: Arc<Window>,
}

impl RuntimeCreateContext {
    fn make_args(
        &self,
        game_dylib_path: &CString,
        window_size: winit::dpi::PhysicalSize<u32>,
    ) -> PillEngineCreateArgsV1 {
        // The runtime must reconstruct this with Arc::from_raw exactly once.
        let window_raw = Arc::into_raw(Arc::clone(&self.window)) as *const c_void;
        PillEngineCreateArgsV1 {
            struct_size: std::mem::size_of::<PillEngineCreateArgsV1>() as u32,
            window_ptr: window_raw,
            game_dylib_path: game_dylib_path.as_ptr(),
            game_resources_dir: self.game_resources_dir.as_ptr(),
            config_path: self.config_path.as_ptr(),
            initial_w: window_size.width,
            initial_h: window_size.height,
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

            let runtime_api = unsafe { &*api };
            if runtime_api.abi_version != PILL_ENGINE_ABI_VERSION {
                bail!(
                    "Engine ABI version mismatch runtime {} host {}",
                    runtime_api.abi_version,
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

        let runtime_api = unsafe { &*api };
        if runtime_api.abi_version != PILL_ENGINE_ABI_VERSION {
            bail!(
                "Engine ABI version mismatch runtime {} host {}",
                runtime_api.abi_version,
                PILL_ENGINE_ABI_VERSION
            );
        }

        Ok(Self {
            _lib: Some(lib),
            api,
            handle: std::ptr::null_mut(),
        })
    }

    fn create(&mut self, args: &PillEngineCreateArgsV1) -> Result<()> {
        let runtime_api = unsafe { &*self.api };
        let ret = (runtime_api.create)(args as *const _, &mut self.handle as *mut _);
        if ret != PILL_OK {
            let error = unsafe { std::ffi::CStr::from_ptr((runtime_api.last_error_utf8)()) };
            bail!("engine create failed: {}", error.to_string_lossy());
        }
        Ok(())
    }

    fn destroy(&mut self) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.destroy)(self.handle);
        self.handle = std::ptr::null_mut();
    }

    fn update(&mut self, dt: Duration) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.update)(self.handle, dt.as_nanos() as u64);
    }

    fn resize(&mut self, w: u32, h: u32) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.resize)(self.handle, w, h);
    }

    fn window_event(&mut self, window_event: &WindowEvent) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.window_event)(self.handle, window_event as *const _ as *const c_void);
    }

    fn key_event(&mut self, key_event: &winit::event::KeyEvent) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.key_event)(self.handle, key_event as *const _ as *const c_void);
    }

    fn mouse_button(&mut self, button: u32, pressed: bool) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.mouse_button)(self.handle, button, pressed);
    }

    fn mouse_delta(&mut self, dx: f64, dy: f64) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.mouse_delta)(self.handle, dx, dy);
    }

    fn cursor_position(&mut self, x: f64, y: f64) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.cursor_position)(self.handle, x, y);
    }

    fn mouse_wheel_line(&mut self, dx: f32, dy: f32) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.mouse_wheel_line)(self.handle, dx, dy);
    }

    fn reload_game(&mut self, game_dylib_path: &Path) -> Result<()> {
        if self.handle.is_null() {
            bail!("Engine not initialized");
        }

        let runtime_api = unsafe { &*self.api };
        let path = CString::new(game_dylib_path.to_string_lossy().as_bytes())?;
        let ret = (runtime_api.reload_game)(self.handle, path.as_ptr());
        if ret != PILL_OK {
            let error = unsafe { std::ffi::CStr::from_ptr((runtime_api.last_error_utf8)()) };
            bail!("engine reload_game failed: {}", error.to_string_lossy());
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
        Ok(value) => (value, false),
        Err(_) => (pill_core::get_default_log_levels(), true),
    };

    set_log_levels(&log_level, false);

    if using_default_log_levels {
        warn!("Using default log levels: {}", log_level);
    }
}

pub fn load_window_icon(path: &Path) -> Option<Icon> {
    let image = image::open(path).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

fn make_window_init(config: &Config, game_resources_directory_path: &Path) -> WindowInit {
    let window_title = config
        .get_str("WINDOW_TITLE")
        .or_else(|_| config.get_str("TITLE"))
        .unwrap_or_else(|_| "Pill".to_owned());

    let window_size = match (
        config.get_int("WINDOW_WIDTH"),
        config.get_int("WINDOW_HEIGHT"),
    ) {
        (Ok(width), Ok(height)) => winit::dpi::PhysicalSize::new(width as u32, height as u32),
        _ => winit::dpi::PhysicalSize::new(1280, 720),
    };

    let fullscreen = config.get_bool("WINDOW_FULLSCREEN").unwrap_or(false);

    let default_icon_bytes = include_bytes!("../res/icon.raw");
    let game_icon_path = game_resources_directory_path.join("icon.ico");
    let window_icon = load_window_icon(&game_icon_path)
        .or_else(|| Icon::from_rgba(default_icon_bytes.to_vec(), 128, 128).ok());

    let min_size = winit::dpi::PhysicalSize::new(100, 100);
    let attributes = WindowAttributes::default()
        .with_title(window_title)
        .with_min_inner_size(min_size)
        .with_inner_size(window_size)
        .with_window_icon(window_icon)
        .with_visible(false);

    WindowInit {
        attributes,
        fullscreen,
    }
}

fn resolve_launcher_command(engine_source_directory_path: &Path) -> Result<OsString> {
    if let Ok(value) = std::env::var("PILL_LAUNCHER_BIN") {
        let path = PathBuf::from(value);
        if !path.exists() {
            bail!(
                "PILL_LAUNCHER_BIN points to missing file: {}",
                path.display()
            );
        }
        return Ok(path.into_os_string());
    }

    if let Ok(value) = std::env::var("PILL_LAUNCHER_CMD") {
        return Ok(OsString::from(value));
    }

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
    let engine_source_directory_path = project_paths
        .engine_source_directory_path
        .as_ref()
        .context("engine_source_directory_path missing for hot reload")?;

    let launcher_cmd = resolve_launcher_command(engine_source_directory_path)?;
    let output_directory = project_paths
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
        output_directory.to_str().unwrap(),
    ];

    let status = std::process::Command::new(&launcher_cmd)
        .args(args)
        .env("PILL_HOT_RELOAD_CHILD", "1")
        .env("PILL_ENGINE_WORKSPACE_DIR", engine_source_directory_path)
        .status();

    let status = match status {
        Ok(status) => status,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let manifest = engine_source_directory_path
                .join("pill_launcher")
                .join("Cargo.toml");
            std::process::Command::new("cargo")
                .args(["run", "--manifest-path", manifest.to_str().unwrap(), "--"])
                .args(args)
                .env("PILL_HOT_RELOAD_CHILD", "1")
                .env("PILL_ENGINE_WORKSPACE_DIR", engine_source_directory_path)
                .status()
                .context("Failed to invoke pill_launcher via cargo for hot reload")?
        }
        Err(error) => return Err(error).context("Failed to invoke pill_launcher for hot reload"),
    };

    if !status.success() {
        bail!("pill_launcher build hot-reload failed");
    }

    Ok(())
}

fn check_and_reload(
    runtime_host: &mut Option<RuntimeHost>,
    runtime_context: &RuntimeCreateContext,
    project_paths: &ProjectPaths,
    last_reload_poll: &mut Instant,
    window_size: winit::dpi::PhysicalSize<u32>,
    file_watchers: &mut FileWatchers,
    runtime_load_mode: RuntimeLoadMode,
) -> Result<()> {
    let now = Instant::now();
    if now.duration_since(*last_reload_poll) < RELOAD_COOLDOWN {
        return Ok(());
    }
    *last_reload_poll = now;

    let mut engine_source_changed = Vec::<PathBuf>::new();
    let mut game_source_changed = Vec::<PathBuf>::new();
    let mut game_resources_changed = Vec::<PathBuf>::new();

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

    if !game_resources_changed.is_empty()
        && game_source_changed.is_empty()
        && engine_source_changed.is_empty()
    {
        info!(LogContext::HotReload => "Game project resources changed; no code rebuild needed: {:?}", game_resources_changed);
        return Ok(());
    }

    let build_start = Instant::now();
    if !game_source_changed.is_empty() || !engine_source_changed.is_empty() {
        build_hot_reload_via_launcher(project_paths)?;
        warn!("Build took: {:?} time", build_start.elapsed());
    }

    let mut runtime_hot_reload = false;
    let mut game_hot_reload = false;
    if let Some(paths) = file_watchers.dynamic_libraries_files_watcher.get_changes() {
        let game_hot_name = dylib("pill_game_hot_reloaded");
        let runtime_hot_name = dylib("pill_runtime_hot_reloaded");

        for path in paths {
            let filename = path.file_name().and_then(|value| value.to_str());
            if filename == Some(&runtime_hot_name) {
                runtime_hot_reload = true;
            } else if filename == Some(&game_hot_name) {
                game_hot_reload = true;
            }
        }
    }

    if runtime_hot_reload && runtime_load_mode == RuntimeLoadMode::InProcess {
        warn!(LogContext::HotReload => "Runtime hot-reload skipped for in-process runtime.");
        runtime_hot_reload = false;
    }

    if runtime_hot_reload {
        info!(LogContext::HotReload => "Reloading runtime (engine hot-reload)...");
        let runtime_reload_start = Instant::now();

        drop(runtime_host.take());

        let loaded_runtime_path = next_loaded_runtime_dylib_path(project_paths);
        fs::copy(
            &project_paths.runtime_dynamic_library_hot_reloaded_path,
            &loaded_runtime_path,
        )
        .context("Failed to copy hot-reloaded runtime dylib to unique loaded path")?;

        let game_path_for_create = if game_hot_reload {
            let loaded_game_path = next_loaded_game_dylib_path(project_paths);
            fs::copy(
                &project_paths.game_dynamic_library_hot_reloaded_path,
                &loaded_game_path,
            )
            .context("Failed to copy hot-reloaded game dylib to unique loaded path")?;
            loaded_game_path
        } else {
            project_paths.game_dynamic_library_path.clone()
        };

        let mut new_runtime = RuntimeHost::load(&loaded_runtime_path, runtime_load_mode)?;
        let game_dylib_path = CString::new(game_path_for_create.to_string_lossy().as_bytes())?;
        let args = runtime_context.make_args(&game_dylib_path, window_size);
        new_runtime.create(&args)?;
        *runtime_host = Some(new_runtime);

        warn!(
            "Runtime reload took: {:?} time",
            runtime_reload_start.elapsed()
        );
        warn!("Total reload took: {:?} time", build_start.elapsed());
    } else if game_hot_reload {
        info!(LogContext::HotReload => "Reloading game project...");
        let game_reload_start = Instant::now();

        let loaded_game_path = next_loaded_game_dylib_path(project_paths);
        fs::copy(
            &project_paths.game_dynamic_library_hot_reloaded_path,
            &loaded_game_path,
        )
        .context("Failed to copy hot-reloaded game dylib to unique loaded path")?;

        if let Some(runtime) = runtime_host.as_mut() {
            runtime.reload_game(&loaded_game_path)?;
        } else {
            bail!("Engine not initialized");
        }

        warn!(
            "Game hot-reload took: {:?} time",
            game_reload_start.elapsed()
        );
        warn!("Total reload took: {:?} time", build_start.elapsed());
    }

    Ok(())
}

fn create_file_watchers(project_paths: &ProjectPaths) -> FileWatchers {
    let engine_workspace_directory_path = project_paths
        .engine_source_directory_path
        .as_ref()
        .expect("engine_source_directory_path missing for hot reload");

    let core_source_path = engine_workspace_directory_path.join("pill_core/src");
    let engine_core_source_files_watcher = FileWatcher::new(core_source_path).set_recursive(true);

    let engine_source_path = engine_workspace_directory_path.join("pill_engine/src");
    let engine_engine_source_files_watcher =
        FileWatcher::new(engine_source_path).set_recursive(true);

    let renderer_source_path = engine_workspace_directory_path.join("pill_renderer/src");
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
        engine_renderer_source_files_watcher,
        dynamic_libraries_files_watcher,
        game_project_source_files_watcher,
        game_project_resources_files_watcher,
    }
}

fn try_remove_files_starting_with(directory_path: &Path, file_name_prefix: &str) {
    if !directory_path.exists() || !directory_path.is_dir() {
        return;
    }

    let entries = match fs::read_dir(directory_path) {
        Ok(entries) => entries,
        Err(error) => {
            warn!(
                LogContext::HotReload => "Failed to read directory {} during cleanup: {}",
                directory_path.display(),
                error
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };

        if !path.is_file() || !name.starts_with(file_name_prefix) {
            continue;
        }

        if let Err(error) = fs::remove_file(&path) {
            warn!(
                LogContext::HotReload => "Ignoring cleanup failure for {}: {}",
                path.display(),
                error
            );
        }
    }
}

struct App {
    project_paths: ProjectPaths,
    hot_reload_enabled: bool,
    runtime_load_mode: RuntimeLoadMode,
    window_init: Option<WindowInit>,

    window: Option<Arc<Window>>,
    window_size: winit::dpi::PhysicalSize<u32>,
    runtime_host: Option<RuntimeHost>,
    runtime_context: Option<RuntimeCreateContext>,
    file_watchers: Option<FileWatchers>,
    last_render_time: Instant,
    last_reload_poll: Instant,
}

impl App {
    fn new(
        project_paths: ProjectPaths,
        hot_reload_enabled: bool,
        runtime_load_mode: RuntimeLoadMode,
        window_init: WindowInit,
    ) -> Self {
        Self {
            project_paths,
            hot_reload_enabled,
            runtime_load_mode,
            window_init: Some(window_init),
            window: None,
            window_size: winit::dpi::PhysicalSize::new(0, 0),
            runtime_host: None,
            runtime_context: None,
            file_watchers: None,
            last_render_time: Instant::now(),
            last_reload_poll: Instant::now(),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let init = self.window_init.take().expect("WindowInit missing");
        let window = Arc::new(
            event_loop
                .create_window(init.attributes)
                .expect("Failed to create window"),
        );

        if init.fullscreen {
            let monitor_handle = window.current_monitor();
            window.set_fullscreen(Some(Fullscreen::Borderless(monitor_handle)));
        }

        self.window_size = window.inner_size();

        self.file_watchers = if self.hot_reload_enabled {
            Some(create_file_watchers(&self.project_paths))
        } else {
            None
        };

        let mut runtime_host = RuntimeHost::load(
            &self.project_paths.runtime_dynamic_library_path,
            self.runtime_load_mode,
        )
        .expect("Failed to load runtime");

        let runtime_context = RuntimeCreateContext {
            game_resources_dir: CString::new(
                self.project_paths
                    .game_resources_directory_path
                    .to_string_lossy()
                    .as_bytes(),
            )
            .expect("Failed to create game resources path CString"),
            config_path: CString::new(self.project_paths.config_path.to_string_lossy().as_bytes())
                .expect("Failed to create config path CString"),
            window: Arc::clone(&window),
        };

        let game_dylib_path = CString::new(
            self.project_paths
                .game_dynamic_library_path
                .to_string_lossy()
                .as_bytes(),
        )
        .expect("Failed to create game dylib path CString");

        let args = runtime_context.make_args(&game_dylib_path, self.window_size);
        runtime_host
            .create(&args)
            .expect("RuntimeHost.create failed");

        window.set_visible(true);

        self.runtime_context = Some(runtime_context);
        self.runtime_host = Some(runtime_host);
        self.window = Some(window);
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if let Some(runtime_host) = self.runtime_host.as_mut() {
                runtime_host.mouse_delta(delta.0, delta.1);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = &self.window else {
            return;
        };
        if window_id != window.id() {
            return;
        }

        if let Some(runtime_host) = self.runtime_host.as_mut() {
            runtime_host.window_event(&event);
        }

        match event {
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let delta = now - self.last_render_time;
                self.last_render_time = now;

                if let Some(runtime_host) = self.runtime_host.as_mut() {
                    runtime_host.update(delta);
                }

                if self.hot_reload_enabled {
                    if let (Some(runtime_context), Some(file_watchers)) =
                        (self.runtime_context.as_ref(), self.file_watchers.as_mut())
                    {
                        check_and_reload(
                            &mut self.runtime_host,
                            runtime_context,
                            &self.project_paths,
                            &mut self.last_reload_poll,
                            self.window_size,
                            file_watchers,
                            self.runtime_load_mode,
                        )
                        .unwrap();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(runtime_host) = self.runtime_host.as_mut() {
                    runtime_host.key_event(&event);
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                if let Some(runtime_host) = self.runtime_host.as_mut() {
                    runtime_host
                        .mouse_button(encode_mouse_button(&button), state == ElementState::Pressed);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(runtime_host) = self.runtime_host.as_mut() {
                    if let MouseScrollDelta::LineDelta(dx, dy) = delta {
                        runtime_host.mouse_wheel_line(dx, dy);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(runtime_host) = self.runtime_host.as_mut() {
                    runtime_host.cursor_position(position.x, position.y);
                }
            }
            WindowEvent::Resized(size) => {
                self.window_size = size;
                if let Some(runtime_host) = self.runtime_host.as_mut() {
                    runtime_host.resize(size.width, size.height);
                }
            }
            WindowEvent::CloseRequested => {
                drop(self.runtime_host.take());
                event_loop.exit();
            }
            _ => {}
        }
    }
}

fn run_app() -> Result<()> {
    let mut hot_reload_enabled =
        std::env::var("PILL_ENABLE_HOT_RELOAD").ok().as_deref() == Some("1");

    let current_directory_path = std::env::current_exe()
        .context("Failed to get current executable path")?
        .parent()
        .context("Executable has no parent directory")?
        .to_path_buf();

    let game_project_directory_path = infer_game_project_directory(&current_directory_path)?;
    let run_layout = resolve_run_layout(&game_project_directory_path);

    if hot_reload_enabled && run_layout != RunLayout::Development {
        bail!("Hot reload requires development layout paths");
    }

    let build_data_directory_path = current_directory_path.join("data");
    let project_resources_directory_path = game_project_directory_path.join("res");
    let build_resources_directory_path = build_data_directory_path.join("res");

    let game_resources_directory_path = match run_layout {
        RunLayout::Development => project_resources_directory_path,
        RunLayout::Packaged if build_resources_directory_path.exists() => {
            build_resources_directory_path
        }
        RunLayout::Packaged => project_resources_directory_path,
    };

    let game_source_directory_path = game_project_directory_path.join("src");
    let config_path = game_resources_directory_path.join("config.ini");

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

    let engine_source_directory_path = if hot_reload_enabled {
        Some(resolve_engine_workspace_dir(
            &current_directory_path,
            &game_project_directory_path,
            true,
        )?)
    } else if runtime_load_mode == RuntimeLoadMode::Dylib {
        resolve_engine_workspace_dir(&current_directory_path, &game_project_directory_path, false)
            .ok()
    } else {
        None
    };

    let runtime_dynamic_library_path = if runtime_load_mode == RuntimeLoadMode::Dylib {
        resolve_runtime_dylib(
            &build_data_directory_path,
            engine_source_directory_path.as_deref(),
            "pill_runtime",
        )?
    } else {
        build_data_directory_path.join(dylib("pill_runtime"))
    };

    let runtime_dynamic_library_hot_reloaded_path =
        if hot_reload_enabled && runtime_load_mode == RuntimeLoadMode::Dylib {
            resolve_runtime_dylib_optional(
                &build_data_directory_path,
                engine_source_directory_path.as_deref(),
                "pill_runtime_hot_reloaded",
            )
            .unwrap_or_else(|| {
                hot_reload_enabled = false;
                runtime_dynamic_library_path.clone()
            })
        } else {
            runtime_dynamic_library_path.clone()
        };

    let game_dynamic_library_path = build_data_directory_path.join(dylib("pill_game"));
    let game_dynamic_library_hot_reloaded_path =
        build_data_directory_path.join(dylib("pill_game_hot_reloaded"));

    let project_paths = ProjectPaths {
        build_data_directory_path,
        engine_source_directory_path,
        game_project_directory_path,
        game_resources_directory_path,
        game_source_directory_path,
        config_path,
        runtime_dynamic_library_path,
        runtime_dynamic_library_hot_reloaded_path,
        game_dynamic_library_path,
        game_dynamic_library_hot_reloaded_path,
    };

    if hot_reload_enabled {
        try_remove_files_starting_with(
            &project_paths.build_data_directory_path,
            &format!("{DYLIB_PREFIX}pill_runtime_loaded"),
        );
        try_remove_files_starting_with(
            &project_paths.build_data_directory_path,
            &format!("{DYLIB_PREFIX}pill_game_loaded"),
        );
    }

    let mut config = Config::default();
    config
        .merge(config::File::with_name(
            project_paths.config_path.to_str().unwrap(),
        ))
        .with_context(|| {
            format!(
                "Failed to load config from {}",
                project_paths.config_path.display()
            )
        })?;

    configure_logging(&config);

    info!(
        LogContext::HotReload => "Hot reload {} (watching src: {}, res: {})",
        if hot_reload_enabled { "enabled" } else { "disabled" },
        project_paths.game_source_directory_path.display(),
        project_paths.game_resources_directory_path.display()
    );
    info!(
        "Initializing {} ({:?} layout, {:?} runtime)",
        "Standalone".module_object_style(),
        run_layout,
        runtime_load_mode
    );

    let window_init = make_window_init(&config, &project_paths.game_resources_directory_path);

    let event_loop = EventLoop::new().context("Failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(
        project_paths,
        hot_reload_enabled,
        runtime_load_mode,
        window_init,
    );
    event_loop.run_app(&mut app).context("run_app failed")?;

    Ok(())
}

fn main() {
    if let Err(error) = run_app() {
        eprintln!("Error: {error:#}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        fs::create_dir_all(engine_a.join("pill_core")).unwrap();
        fs::create_dir_all(engine_a.join("pill_engine")).unwrap();
        fs::create_dir_all(engine_a.join("pill_renderer")).unwrap();
        fs::create_dir_all(engine_b.join("pill_core")).unwrap();
        fs::create_dir_all(engine_b.join("pill_engine")).unwrap();
        fs::create_dir_all(engine_b.join("pill_renderer")).unwrap();

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

        std::env::set_var("PILL_ENGINE_WORKSPACE_DIR", &engine_a);
        let resolved = resolve_engine_workspace_dir(&game_dir, &game_dir, true).unwrap();
        assert_eq!(resolved, engine_b);

        let _ = fs::remove_dir_all(root);
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

        let _ = fs::remove_dir_all(root);
    }
}
