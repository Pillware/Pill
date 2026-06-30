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

// Minimum time between successive hot-reload attempts to avoid
// triggering rebuilds on every file-system event.
const RELOAD_COOLDOWN: Duration = Duration::from_millis(1000);

// Monotonic counter used to generate unique suffixes for
// hot-reloaded dynamic libraries so that old copies can coexist
// with the newly loaded ones.
static RELOAD_GEN: AtomicU64 = AtomicU64::new(0);

// Holds the winit window attributes and desired fullscreen state
// before the window is actually created.
struct WindowInit {
    attributes: WindowAttributes,
    fullscreen: bool,
}

// Groups together all file watchers used during hot-reload.
// Each watcher tracks a different directory:
//   - Engine source crates (pill_core, pill_engine, pill_renderer)
//   - Dynamic libraries output folder
//   - Project source and resources folders
struct FileWatchers {
    engine_core_source_files_watcher: FileWatcher,
    engine_engine_source_files_watcher: FileWatcher,
    engine_renderer_source_files_watcher: FileWatcher,
    dynamic_libraries_files_watcher: FileWatcher,
    project_source_files_watcher: FileWatcher,
    project_resources_files_watcher: FileWatcher,
}

// Central store for every path the standalone runner needs at runtime.
// Avoids scattering path joins throughout the codebase.
struct ProjectPaths {
    build_data_directory_path: PathBuf,
    engine_source_directory_path: Option<PathBuf>,
    project_directory_path: PathBuf,
    project_resources_directory_path: PathBuf,
    project_source_directory_path: PathBuf,
    config_path: PathBuf,
    runtime_dynamic_library_path: PathBuf,
    runtime_dynamic_library_hot_reloaded_path: PathBuf,
    project_dynamic_library_path: PathBuf,
    project_dynamic_library_hot_reloaded_path: PathBuf,
}

// Platform-specific dynamic-library naming conventions.
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

// Builds a platform-appropriate dynamic-library file name from a base name.
fn dylib(name: &str) -> String {
    format!("{DYLIB_PREFIX}{name}{DYLIB_SUFFIX}")
}

// Describes whether the runtime (pill_runtime) is loaded as a separate
// dynamic library or compiled directly into the executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeLoadMode {
    Dylib,
    InProcess,
}

// Parses the runtime load mode from an environment variable value.
fn parse_runtime_load_mode(value: Option<String>) -> Option<RuntimeLoadMode> {
    match value.as_deref() {
        Some("dylib") => Some(RuntimeLoadMode::Dylib),
        Some("in_process") => Some(RuntimeLoadMode::InProcess),
        _ => None,
    }
}

// Distinguishes between a development workspace layout (project lives
// alongside the engine) and a packaged release layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunLayout {
    Development,
    Packaged,
}

// Checks whether a directory contains the minimum set of files
// required to be treated as a valid Pill project.
fn project_exists(path: &Path) -> bool {
    path.join("Cargo.toml").exists()
        && path.join("res").join("config.ini").exists()
        && path.join("src").exists()
}

// Determines the project directory from either the PROJECT_DIR
// environment variable or by walking up from the executable path.
fn infer_project_directory(current_directory_path: &Path) -> Result<PathBuf> {
    // First check the explicit environment override.
    if let Ok(value) = std::env::var("PROJECT_DIR") {
        let path = PathBuf::from(value);
        if project_exists(&path) {
            return Ok(path);
        }
        bail!(
            "PROJECT_DIR was set but {} is not a valid project",
            path.display()
        );
    }

    // Fall back: the executable is at <project>/build/<dev|release>/pill_native.exe,
    // so go up two levels to reach the project root.
    current_directory_path
        .parent()
        .context("Build directory has no parent")?
        .parent()
        .context("Project directory resolution failed")
        .map(Path::to_path_buf)
}

// Classifies the current run as Development or Packaged based on
// environment variables and filesystem heuristics.
fn resolve_run_layout(project_directory_path: &Path) -> RunLayout {
    match std::env::var("PILL_STANDALONE_LAYOUT").ok().as_deref() {
        Some("development") => RunLayout::Development,
        Some("packaged") => RunLayout::Packaged,
        _ if project_exists(project_directory_path) => RunLayout::Development,
        _ => RunLayout::Packaged,
    }
}

// Returns true when the engine workspace's Cargo.toml lists the given
// project directory name as a workspace member.
fn workspace_includes_project(
    engine_source_directory_path: &Path,
    project_directory_path: &Path,
) -> bool {
    let cargo_toml_path = engine_source_directory_path.join("Cargo.toml");
    let Ok(contents) = fs::read_to_string(cargo_toml_path) else {
        return false;
    };
    let Some(project_dir_name) = project_directory_path
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    contents.contains(project_dir_name)
}

// Quick heuristic: a directory looks like the engine workspace if it
// contains the three core engine crates.
fn looks_like_engine_workspace(path: &Path) -> bool {
    path.join("pill_core").exists()
        && path.join("pill_engine").exists()
        && path.join("pill_renderer").exists()
}

// Attempts to read the `workspace` key from the project's Cargo.toml
// and returns the resolved path if it points to an existing directory.
fn engine_workspace_from_project_manifest(project_directory_path: &Path) -> Option<PathBuf> {
    let manifest_path = project_directory_path.join("Cargo.toml");
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

// Searches for the engine workspace directory by walking up from the
// executable location and by scanning sibling directories of the project.
// Returns the first path that looks like a valid engine workspace.
fn find_engine_source_directory(
    current_directory_path: &Path,
    project_directory_path: &Path,
) -> Option<PathBuf> {
    // Walk up the directory tree from the current executable path.
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

    // Scan sibling directories of the project as a fallback.
    if let Some(parent) = project_directory_path.parent() {
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

// Resolves the engine workspace directory using a priority chain:
//   1. The `workspace` key in the project's Cargo.toml
//   2. The PILL_ENGINE_WORKSPACE_DIR environment variable
//   3. Filesystem scanning (ancestor and sibling search)
// Optionally validates that the workspace actually includes the project.
fn resolve_engine_workspace_dir(
    current_directory_path: &Path,
    project_directory_path: &Path,
    require_workspace_membership: bool,
) -> Result<PathBuf> {
    let by_manifest = engine_workspace_from_project_manifest(project_directory_path);
    let by_env = std::env::var("PILL_ENGINE_WORKSPACE_DIR")
        .ok()
        .map(PathBuf::from);
    let by_scan = find_engine_source_directory(current_directory_path, project_directory_path);

    for candidate in [by_manifest, by_env, by_scan].into_iter().flatten() {
        // Skip candidates that don't look like valid engine workspaces.
        if !looks_like_engine_workspace(&candidate) && !candidate.join("pill_engine").exists() {
            continue;
        }

        // Optionally check that the workspace manifest lists this project.
        if require_workspace_membership
            && !workspace_includes_project(&candidate, project_directory_path)
        {
            continue;
        }

        return Ok(candidate);
    }

    bail!(
        "Engine workspace not detected. Set PILL_ENGINE_WORKSPACE_DIR to the engine directory{}.",
        if require_workspace_membership {
            " that includes the pill project workspace member"
        } else {
            ""
        }
    )
}

// Builds a list of candidate paths where a runtime dynamic library
// might be found, covering both the build data directory and the
// engine workspace target directories (debug and release).
fn resolve_runtime_dylib_candidates(
    build_data_directory_path: &Path,
    engine_source_directory_path: Option<&Path>,
    name: &str,
) -> Vec<PathBuf> {
    let mut candidates = vec![build_data_directory_path.join(dylib(name))];

    if let Some(engine_source_directory_path) = engine_source_directory_path {
        // Also search the workspace-level target directories.
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

// Finds the first existing candidate for a runtime dynamic library.
// Errors if no candidate exists on disk.
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

// Same search as resolve_runtime_dylib but returns None instead of
// an error when the library cannot be found.
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

// Generates a unique file path for a hot-reloaded runtime library by
// appending a monotonically increasing generation number.
fn next_loaded_runtime_dylib_path(project_paths: &ProjectPaths) -> PathBuf {
    let generation = RELOAD_GEN.fetch_add(1, Ordering::Relaxed);
    project_paths
        .build_data_directory_path
        .join(dylib(&format!("pill_runtime_loaded_{generation}")))
}

// Generates a unique file path for a hot-reloaded project library.
fn next_loaded_project_dylib_path(project_paths: &ProjectPaths) -> PathBuf {
    let generation = RELOAD_GEN.fetch_add(1, Ordering::Relaxed);
    project_paths
        .build_data_directory_path
        .join(dylib(&format!("project_loaded_{generation}")))
}

// Maps winit's MouseButton enum to a stable u32 for the FFI boundary.
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

// Holds the data needed to construct a PillEngineCreateArgsV1 each time
// the runtime is (re)created. The window Arc is cloned so the runtime
// receives its own counted reference.
struct RuntimeCreateContext {
    project_resources_dir: CString,
    config_path: CString,
    window: Arc<Window>,
}

impl RuntimeCreateContext {
    // Builds the FFI argument struct for pill_runtime::create.
    // Transfers one Arc<Window> reference to the runtime via Arc::into_raw.
    fn make_args(
        &self,
        project_dylib_path: &CString,
        window_size: winit::dpi::PhysicalSize<u32>,
    ) -> PillEngineCreateArgsV1 {
        // The runtime must reconstruct this with Arc::from_raw exactly once.
        let window_raw = Arc::into_raw(Arc::clone(&self.window)) as *const c_void;
        PillEngineCreateArgsV1 {
            struct_size: std::mem::size_of::<PillEngineCreateArgsV1>() as u32,
            window_ptr: window_raw,
            project_dylib_path: project_dylib_path.as_ptr(),
            project_resources_dir: self.project_resources_dir.as_ptr(),
            config_path: self.config_path.as_ptr(),
            initial_w: window_size.width,
            initial_h: window_size.height,
        }
    }
}

// Owns a loaded runtime dynamic library and provides typed access to every
// function in the pill_abi FFI table. All FFI calls go through the `api`
// vtable so that the host never links directly against pill_runtime symbols.
struct RuntimeHost {
    // Keeps the dynamic library loaded; dropped after `api` is no longer used.
    _lib: Option<Library>,
    // Pointer to the static PillEngineApiV1 exported by the runtime.
    api: *const PillEngineApiV1,
    // Opaque pointer the runtime uses to identify the engine instance.
    handle: EngineHandle,
}

impl RuntimeHost {
    // Loads the runtime, either by dynamically opening the shared library
    // or by calling the in-process symbol directly.
    fn load(runtime_dylib_path: &Path, load_mode: RuntimeLoadMode) -> Result<Self> {
        if load_mode == RuntimeLoadMode::InProcess {
            // When compiled in-process the vtable is a direct function call.
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

        // Dynamic-library path: use libloading to open the .dll/.so/.dylib.
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

    // Initialises the engine inside the runtime with the given
    // creation arguments.  Must be called after `load`.
    fn create(&mut self, args: &PillEngineCreateArgsV1) -> Result<()> {
        let runtime_api = unsafe { &*self.api };
        let ret = (runtime_api.create)(args as *const _, &mut self.handle as *mut _);
        if ret != PILL_OK {
            let error = unsafe { std::ffi::CStr::from_ptr((runtime_api.last_error_utf8)()) };
            bail!("engine create failed: {}", error.to_string_lossy());
        }
        Ok(())
    }

    // Tears down the engine and releases the runtime's resources.
    // Safe to call multiple times (no-op if already destroyed).
    fn destroy(&mut self) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.destroy)(self.handle);
        self.handle = std::ptr::null_mut();
    }

    // Advances the engine by one frame. `delta_time` is the wall-clock
    // duration since the previous call to `update`.
    fn update(&mut self, delta_time: Duration) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.update)(self.handle, delta_time.as_nanos() as u64);
    }

    // Notifies the engine that the window has been resized.
    fn resize(&mut self, width: u32, height: u32) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.resize)(self.handle, width, height);
    }

    // Forwards a raw winit WindowEvent to the engine for egui input
    // processing (no-op when the debug_ui feature is disabled).
    fn window_event(&mut self, window_event: &WindowEvent) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.window_event)(self.handle, window_event as *const _ as *const c_void);
    }

    // Forwards a keyboard event to the engine.
    fn key_event(&mut self, key_event: &winit::event::KeyEvent) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.key_event)(self.handle, key_event as *const _ as *const c_void);
    }

    // Forwards a mouse button press/release.
    fn mouse_button(&mut self, button: u32, pressed: bool) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.mouse_button)(self.handle, button, pressed);
    }

    // Forwards raw mouse motion delta.
    fn mouse_delta(&mut self, delta_x: f64, delta_y: f64) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.mouse_delta)(self.handle, delta_x, delta_y);
    }

    // Forwards cursor position in physical pixels.
    fn cursor_position(&mut self, x: f64, y: f64) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.cursor_position)(self.handle, x, y);
    }

    // Forwards mouse wheel line-scroll deltas.
    fn mouse_wheel_line(&mut self, delta_x: f32, delta_y: f32) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.mouse_wheel_line)(self.handle, delta_x, delta_y);
    }

    // Instructs the runtime to swap the currently loaded pill project
    // for a newly compiled one (hot-reload).
    fn reload_project(&mut self, project_dylib_path: &Path) -> Result<()> {
        if self.handle.is_null() {
            bail!("Engine not initialized");
        }

        let runtime_api = unsafe { &*self.api };
        let path = CString::new(project_dylib_path.to_string_lossy().as_bytes())?;
        let ret = (runtime_api.reload_project)(self.handle, path.as_ptr());
        if ret != PILL_OK {
            let error = unsafe { std::ffi::CStr::from_ptr((runtime_api.last_error_utf8)()) };
            bail!("engine reload_project failed: {}", error.to_string_lossy());
        }
        Ok(())
    }

    // Returns true when the engine has requested graceful shutdown
    // (for example, a benchmark that finishes after N frames).
    fn should_exit(&self) -> bool {
        if self.handle.is_null() {
            return false;
        }
        let runtime_api = unsafe { &*self.api };
        (runtime_api.is_exit_requested)(self.handle) != 0
    }
}

// Ensures the runtime engine is always torn down, even if the App
// is dropped abnormally (e.g. during a panic unwind).
impl Drop for RuntimeHost {
    fn drop(&mut self) {
        self.destroy();
    }
}

// Reads the LOG_LEVELS key from the project's config.ini and applies
// it to the pill_core logger.  Falls back to built-in defaults when
// the key is missing.
fn configure_logging(config: &Config) {
    let (log_level, using_default_log_levels) = match config.get_str("LOG_LEVELS") {
        Ok(value) => (value, false),
        Err(_) => (pill_core::get_default_log_levels(), true),
    };

    set_log_levels(&log_level, false);

    if using_default_log_levels {
        info!("Using default log levels: {}", log_level);
    }
}

// Loads an icon from disk and converts it to a winit Icon.
// Returns None if the file is missing or cannot be decoded.
pub fn load_window_icon(path: &Path) -> Option<Icon> {
    let image = image::open(path).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

// Builds the WindowInit descriptor from the project configuration.
// Sets title, size, fullscreen mode, and attempts to load a custom
// window icon (falls back to the embedded default icon).
fn make_window_init(config: &Config, project_resources_directory_path: &Path) -> WindowInit {
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
    let project_icon_path = project_resources_directory_path.join("icon.ico");
    let window_icon = load_window_icon(&project_icon_path)
        .or_else(|| Icon::from_rgba(default_icon_bytes.to_vec(), 128, 128).ok());

    let minimum_window_size = winit::dpi::PhysicalSize::new(100, 100);
    let attributes = WindowAttributes::default()
        .with_title(window_title)
        .with_min_inner_size(minimum_window_size)
        .with_inner_size(window_size)
        .with_window_icon(window_icon)
        .with_visible(false);

    WindowInit {
        attributes,
        fullscreen,
    }
}

// Resolves the path (or command) to the PillLauncher binary used for
// hot-reload builds.  Checks, in order:
//   1. PILL_LAUNCHER_BIN env var (explicit path)
//   2. PILL_LAUNCHER_CMD env var (shell command)
//   3. Standard target directories under pill_launcher/
// Falls back to "PillLauncherUpstream" on PATH if nothing is found.
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

// Invokes PillLauncher to perform a hot-reload build of the project.
// Sets PILL_HOT_RELOAD_STATUS so external tooling can react to
// success / warning / failure.
fn build_hot_reload_via_launcher(project_paths: &ProjectPaths) -> Result<()> {
    let engine_source_directory_path = project_paths
        .engine_source_directory_path
        .as_ref()
        .context("engine_source_directory_path missing for hot reload")?;

    let launcher_command = resolve_launcher_command(engine_source_directory_path)?;
    let output_directory = project_paths
        .build_data_directory_path
        .parent()
        .context("build_data_directory_path has no parent")?;

    let arguments = [
        "build",
        "-p",
        project_paths.project_directory_path.to_str().unwrap(),
        "-c",
        "hot-reload",
        "-o",
        output_directory.to_str().unwrap(),
    ];

    // Try running the launcher binary; fall back to `cargo run` if it
    // hasn't been compiled yet.
    let output = std::process::Command::new(&launcher_command)
        .args(arguments)
        .env("PILL_HOT_RELOAD_CHILD", "1")
        .env("PILL_ENGINE_WORKSPACE_DIR", engine_source_directory_path)
        .output();

    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let manifest = engine_source_directory_path
                .join("pill_launcher")
                .join("Cargo.toml");
            std::process::Command::new("cargo")
                .args(["run", "--manifest-path", manifest.to_str().unwrap(), "--"])
                .args(arguments)
                .env("PILL_HOT_RELOAD_CHILD", "1")
                .env("PILL_ENGINE_WORKSPACE_DIR", engine_source_directory_path)
                .output()
                .context("Failed to invoke pill_launcher via cargo for hot reload")?
        }
        Err(error) => return Err(error).context("Failed to invoke pill_launcher for hot reload"),
    };

    let standard_output = String::from_utf8_lossy(&output.stdout);
    let standard_error = String::from_utf8_lossy(&output.stderr);

    print!("{standard_output}");
    eprint!("{standard_error}");

    if !output.status.success() {
        std::env::set_var("PILL_HOT_RELOAD_STATUS", "fail");
        bail!("pill_launcher build hot-reload failed");
    }

    let has_warnings = standard_output.contains("warning:") || standard_error.contains("warning:");
    if has_warnings {
        std::env::set_var("PILL_HOT_RELOAD_STATUS", "warn");
    } else {
        std::env::set_var("PILL_HOT_RELOAD_STATUS", "pass");
    }
    Ok(())
}

// Polls all file watchers for changes and triggers a hot-reload build
// followed by runtime / project reload when needed.
//
// High-level flow:
//   1. Check that the cooldown period has elapsed.
//   2. Collect changed paths from every file watcher.
//   3. If only resource files changed, skip the build (no recompilation needed).
//   4. If source files changed, invoke pill_launcher to rebuild.
//   5. If new dylibs appeared, reload the runtime and/or project in-place.
fn check_and_reload(
    runtime_host: &mut Option<RuntimeHost>,
    runtime_context: &RuntimeCreateContext,
    project_paths: &ProjectPaths,
    last_reload_poll: &mut Instant,
    window_size: winit::dpi::PhysicalSize<u32>,
    file_watchers: &mut FileWatchers,
    runtime_load_mode: RuntimeLoadMode,
) -> Result<()> {
    // --- 1. Cooldown gate ---
    let now = Instant::now();
    if now.duration_since(*last_reload_poll) < RELOAD_COOLDOWN {
        return Ok(());
    }
    *last_reload_poll = now;

    // --- 2. Collect file changes ---
    let mut engine_source_changes = Vec::<PathBuf>::new();
    let mut project_source_changes = Vec::<PathBuf>::new();
    let mut project_resources_changes = Vec::<PathBuf>::new();

    if let Some(paths) = file_watchers.engine_core_source_files_watcher.get_changes() {
        info!(LogContext::HotReload => "Engine pill_core source file change detected: {:?}", paths);
        engine_source_changes.extend(paths);
    }
    if let Some(paths) = file_watchers
        .engine_engine_source_files_watcher
        .get_changes()
    {
        info!(LogContext::HotReload => "Engine pill_engine source file change detected: {:?}", paths);
        engine_source_changes.extend(paths);
    }
    if let Some(paths) = file_watchers
        .engine_renderer_source_files_watcher
        .get_changes()
    {
        info!(LogContext::HotReload => "Engine pill_renderer source file change detected: {:?}", paths);
        engine_source_changes.extend(paths);
    }

    if let Some(paths) = file_watchers.project_resources_files_watcher.get_changes() {
        info!(LogContext::HotReload => "Pill project resources file change detected: {:?}", paths);
        project_resources_changes.extend(paths);
    }
    if let Some(paths) = file_watchers.project_source_files_watcher.get_changes() {
        info!(LogContext::HotReload => "Pill project source file change detected: {:?}", paths);
        project_source_changes.extend(paths);
    }

    // --- 3. Resource-only changes: no rebuild needed ---
    if !project_resources_changes.is_empty()
        && project_source_changes.is_empty()
        && engine_source_changes.is_empty()
    {
        info!(LogContext::HotReload => "Pill project resources changed; no code rebuild needed: {:?}", project_resources_changes);
        return Ok(());
    }

    // --- 4. Build via launcher ---
    let build_start = Instant::now();
    if !project_source_changes.is_empty() || !engine_source_changes.is_empty() {
        if let Err(error) = build_hot_reload_via_launcher(project_paths) {
            warn!(
                LogContext::HotReload =>
                "Hot-reload build failed; keeping currently loaded runtime project. Error: {error:?}"
            );

            // Drain the dylib watcher so stale events don't trigger
            // another build immediately.
            let _ = file_watchers.dynamic_libraries_files_watcher.get_changes();

            return Ok(());
        }
        warn!("Build took: {:?} time", build_start.elapsed());
    }

    // --- 5. Detect which dylibs were (re)built ---
    let mut runtime_hot_reload = false;
    let mut project_hot_reload = false;
    if let Some(paths) = file_watchers.dynamic_libraries_files_watcher.get_changes() {
        let project_hot_name = dylib("project_hot_reloaded");
        let runtime_hot_name = dylib("pill_runtime_hot_reloaded");

        for path in paths {
            let filename = path.file_name().and_then(|value| value.to_str());
            if filename == Some(&runtime_hot_name) {
                runtime_hot_reload = true;
            } else if filename == Some(&project_hot_name) {
                project_hot_reload = true;
            }
        }
    }

    // In-process runtime cannot be hot-reloaded; skip with a warning.
    if runtime_hot_reload && runtime_load_mode == RuntimeLoadMode::InProcess {
        warn!(LogContext::HotReload => "Runtime hot-reload skipped for in-process runtime.");
        runtime_hot_reload = false;
    }

    // --- 5a. Full runtime reload (engine code changed) ---
    if runtime_hot_reload {
        info!(LogContext::HotReload => "Reloading runtime (engine hot-reload)...");
        let runtime_reload_start = Instant::now();

        // Drop the old runtime, which unloads the old engine and project.
        drop(runtime_host.take());

        let loaded_runtime_path = next_loaded_runtime_dylib_path(project_paths);
        fs::copy(
            &project_paths.runtime_dynamic_library_hot_reloaded_path,
            &loaded_runtime_path,
        )
        .context("Failed to copy hot-reloaded runtime dylib to unique loaded path")?;

        let project_path_for_create = if project_hot_reload {
            let loaded_project_path = next_loaded_project_dylib_path(project_paths);
            fs::copy(
                &project_paths.project_dynamic_library_hot_reloaded_path,
                &loaded_project_path,
            )
            .context("Failed to copy hot-reloaded pill project dylib to unique loaded path")?;
            loaded_project_path
        } else {
            project_paths.project_dynamic_library_path.clone()
        };

        let mut new_runtime = RuntimeHost::load(&loaded_runtime_path, runtime_load_mode)?;
        let project_dylib_path =
            CString::new(project_path_for_create.to_string_lossy().as_bytes())?;
        let args = runtime_context.make_args(&project_dylib_path, window_size);
        new_runtime.create(&args)?;
        *runtime_host = Some(new_runtime);

        warn!(
            "Runtime reload took: {:?} time",
            runtime_reload_start.elapsed()
        );
        warn!("Total reload took: {:?} time", build_start.elapsed());
    }
    // --- 5b. Project-only reload (game code changed, engine unchanged) ---
    else if project_hot_reload {
        info!(LogContext::HotReload => "Reloading pill project...");
        let project_reload_start = Instant::now();

        let loaded_project_path = next_loaded_project_dylib_path(project_paths);
        fs::copy(
            &project_paths.project_dynamic_library_hot_reloaded_path,
            &loaded_project_path,
        )
        .context("Failed to copy hot-reloaded pill project dylib to unique loaded path")?;

        if let Some(runtime) = runtime_host.as_mut() {
            runtime.reload_project(&loaded_project_path)?;
        } else {
            bail!("Engine not initialized");
        }

        warn!(
            "Pill project hot-reload took: {:?} time",
            project_reload_start.elapsed()
        );
        warn!("Total reload took: {:?} time", build_start.elapsed());
    }

    Ok(())
}

// Creates a full set of file watchers for all directories relevant
// to hot-reload: engine crates, output dylibs, and project source/resources.
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
    let project_source_files_watcher =
        FileWatcher::new(project_paths.project_source_directory_path.clone()).set_recursive(true);
    let project_resources_files_watcher =
        FileWatcher::new(project_paths.project_resources_directory_path.clone())
            .set_recursive(true);

    FileWatchers {
        engine_core_source_files_watcher,
        engine_engine_source_files_watcher,
        engine_renderer_source_files_watcher,
        dynamic_libraries_files_watcher,
        project_source_files_watcher,
        project_resources_files_watcher,
    }
}

// Removes files whose names start with the given prefix from a directory.
// Used during startup to clean up stale hot-reloaded libraries from
// previous runs. Failures are logged but otherwise ignored.
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

        // Only remove regular files with the expected prefix.
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

// Top-level application state.
// Owns the window, the runtime host, file watchers, and all path
// configuration.  Implements winit's ApplicationHandler so it can
// respond to window and device events.
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
    // Constructs the App in a pre-initialised state.
    // The window and runtime are created lazily when the event loop
    // calls `resumed` for the first time.
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
    // Called once when the event loop is ready.
    // Creates the window, loads the runtime, initialises the engine,
    // and sets up file watchers if hot-reload is enabled.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Guard against multiple resumes.
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
            project_resources_dir: CString::new(
                self.project_paths
                    .project_resources_directory_path
                    .to_string_lossy()
                    .as_bytes(),
            )
            .expect("Failed to create pill project resources path CString"),
            config_path: CString::new(self.project_paths.config_path.to_string_lossy().as_bytes())
                .expect("Failed to create config path CString"),
            window: Arc::clone(&window),
        };

        let project_dylib_path = CString::new(
            self.project_paths
                .project_dynamic_library_path
                .to_string_lossy()
                .as_bytes(),
        )
        .expect("Failed to create pill project dylib path CString");

        let args = runtime_context.make_args(&project_dylib_path, self.window_size);
        runtime_host
            .create(&args)
            .expect("RuntimeHost.create failed");

        // Show the window only after everything is initialised to avoid
        // a flash of unrendered content.
        window.set_visible(true);

        self.runtime_context = Some(runtime_context);
        self.runtime_host = Some(runtime_host);
        self.window = Some(window);
    }

    // Called when the event loop is about to wait for new events.
    // We use a Poll control flow, so this requests a redraw on every
    // iteration to drive continuous rendering.
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    // Handles raw device events (currently only mouse motion).
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

    // Main window event handler.
    // Dispatches every event to the runtime for input processing,
    // then handles engine updates, rendering, hot-reload polling,
    // and shutdown.
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = &self.window else {
            return;
        };
        // Ignore events for other windows (shouldn't happen, but safe).
        if window_id != window.id() {
            return;
        }

        // Forward every event to the engine for egui/input handling.
        if let Some(runtime_host) = self.runtime_host.as_mut() {
            runtime_host.window_event(&event);
        }

        match event {
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let delta = now - self.last_render_time;
                self.last_render_time = now;

                // Advance the engine by one frame.
                if let Some(runtime_host) = self.runtime_host.as_mut() {
                    runtime_host.update(delta);

                    // Check if the engine has requested graceful exit
                    // (e.g. benchmark finished after N frames).
                    if runtime_host.should_exit() {
                        event_loop.exit();
                        return;
                    }
                }

                // Poll for hot-reload changes after each frame.
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
                    if let MouseScrollDelta::LineDelta(delta_x, delta_y) = delta {
                        runtime_host.mouse_wheel_line(delta_x, delta_y);
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
            // Let the event loop exit cleanly.
            // The runtime/engine/renderer/wgpu stack is torn down later
            // when `App` is dropped, after the event loop has stopped.
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            _ => {}
        }
    }
}

// Main application entry point.
//
// Responsibilities:
//   - Detect the project and engine workspace directories.
//   - Resolve paths to runtime and project dynamic libraries.
//   - Load configuration, set up logging, and create the winit window.
//   - Initialise the App and hand control to the winit event loop.
fn run_app() -> Result<()> {
    // In the development build, standalone will look for the resource files in the "res" directory of the pill project directory
    // In the release build, "res" directory is copied to /build/release/data/res (TODO: pack all resources use by pill project into a single data file)

    // /<project_root>
    // ├── /build
    // │   ├── /dev
    // │   │   ├── pill_native.exe
    // │   │   └── /data
    // │   │       ├── pill_project.dll
    // │   │       └── pill_project_hot_reload.dll
    // │   └── /release
    // │       ├── pill_native.exe
    // │       └── /data
    // │           ├── /res
    // │           ├── pill_project.dll
    // │           └── pill_project_hot_reload.dll
    // ├── /src
    // ├── /res
    // │   ├── icon.raw
    // │   ├── icon.ico
    // │   └── config.ini
    // ├── Cargo.toml
    // └── Cargo.lock

    let mut hot_reload_enabled =
        std::env::var("PILL_COMPILE_MODE").ok().as_deref() == Some("hot-reload");

    // Determine the project directory relative to the executable.
    let current_directory_path = std::env::current_exe()
        .context("Failed to get current executable path")?
        .parent()
        .context("Executable has no parent directory")?
        .to_path_buf();

    let project_directory_path = infer_project_directory(&current_directory_path)?;
    let run_layout = resolve_run_layout(&project_directory_path);

    if hot_reload_enabled && run_layout != RunLayout::Development {
        bail!("Hot reload requires development layout paths");
    }

    // Build all the path information the app will need.
    let build_data_directory_path = current_directory_path.join("data");
    let project_resources_directory_path = project_directory_path.join("res");
    let build_resources_directory_path = build_data_directory_path.join("res");

    let project_resources_directory_path = match run_layout {
        RunLayout::Development => project_resources_directory_path,
        RunLayout::Packaged if build_resources_directory_path.exists() => {
            build_resources_directory_path
        }
        RunLayout::Packaged => project_resources_directory_path,
    };

    let project_source_directory_path = project_directory_path.join("src");
    let config_path = project_resources_directory_path.join("config.ini");

    // Decide how to load the runtime (dynamic library or in-process).
    let in_process = std::env::var("PILL_RUNTIME_IN_PROCESS").ok().as_deref() == Some("1");

    let runtime_load_mode = parse_runtime_load_mode(std::env::var("PILL_RUNTIME_MODE").ok())
        .or(in_process.then_some(RuntimeLoadMode::InProcess))
        .unwrap_or(if cfg!(target_os = "macos") {
            RuntimeLoadMode::InProcess
        } else {
            RuntimeLoadMode::Dylib
        });

    let engine_source_directory_path = if hot_reload_enabled {
        Some(resolve_engine_workspace_dir(
            &current_directory_path,
            &project_directory_path,
            true,
        )?)
    } else if runtime_load_mode == RuntimeLoadMode::Dylib {
        resolve_engine_workspace_dir(&current_directory_path, &project_directory_path, false).ok()
    } else {
        None
    };

    // Resolve paths to the runtime dynamic library.
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

    let project_dynamic_library_path = build_data_directory_path.join(dylib("project"));
    let project_dynamic_library_hot_reloaded_path =
        build_data_directory_path.join(dylib("project_hot_reloaded"));

    let project_paths = ProjectPaths {
        build_data_directory_path,
        engine_source_directory_path,
        project_directory_path,
        project_resources_directory_path,
        project_source_directory_path,
        config_path,
        runtime_dynamic_library_path,
        runtime_dynamic_library_hot_reloaded_path,
        project_dynamic_library_path,
        project_dynamic_library_hot_reloaded_path,
    };

    // Clean up stale hot-reloaded libraries from previous runs.
    if hot_reload_enabled {
        try_remove_files_starting_with(
            &project_paths.build_data_directory_path,
            &format!("{DYLIB_PREFIX}pill_runtime_loaded"),
        );
        try_remove_files_starting_with(
            &project_paths.build_data_directory_path,
            &format!("{DYLIB_PREFIX}project_loaded"),
        );
    }

    // Load and apply project configuration.
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

    // Log hot-reload status; only include watch paths when enabled.
    if hot_reload_enabled {
        info!(
            LogContext::HotReload => "Hot reload enabled (watching src: {}, res: {})",
            project_paths.project_source_directory_path.display(),
            project_paths.project_resources_directory_path.display()
        );
    } else {
        info!(LogContext::HotReload => "Hot reload disabled");
    }
    info!(
        "Initializing {} ({:?} layout, {:?} runtime)",
        "Standalone".module_object_style(),
        run_layout,
        runtime_load_mode
    );

    let window_init = make_window_init(&config, &project_paths.project_resources_directory_path);

    // Create the event loop and hand control to the App.
    let event_loop = EventLoop::new().context("Failed to create event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(
        project_paths,
        hot_reload_enabled,
        runtime_load_mode,
        window_init,
    );
    event_loop.run_app(&mut app).context("run_app failed")?;

    // When the event loop exits, `app` is dropped, which tears down
    // the runtime, renderer, window, and file watchers in the correct order.
    Ok(())
}

// Process entry point.  Runs the app and prints any fatal error to stderr.
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
    fn prefers_project_manifest_workspace_over_env_and_sibling_scan() {
        let root = unique_tmp_dir("hot_reload_workspace_pick");
        let _ = fs::remove_dir_all(&root);

        let project_dir = root.join("my_project");
        fs::create_dir_all(project_dir.join("src")).unwrap();
        fs::create_dir_all(project_dir.join("res")).unwrap();

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
members = ["my_project"]
"#,
        )
        .unwrap();
        fs::write(
            engine_b.join("Cargo.toml"),
            r#"[workspace]
members = ["my_project"]
"#,
        )
        .unwrap();

        fs::write(
            project_dir.join("Cargo.toml"),
            format!(
                r#"[package]
name = "my_project"
version = "0.1.0"
edition = "2021"
workspace = "{}"
"#,
                engine_b.display()
            ),
        )
        .unwrap();

        std::env::set_var("PILL_ENGINE_WORKSPACE_DIR", &engine_a);
        let resolved = resolve_engine_workspace_dir(&project_dir, &project_dir, true).unwrap();
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
