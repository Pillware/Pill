//! This file is the native standalone runner entry point for Pill projects.
//!
//! On startup it detects the project and engine workspace directories, resolves
//! paths to runtime and project dynamic libraries, loads configuration, creates
//! a winit window, and hands control to the event loop for rendering and input.
//!
//! At compile time, the `headless` feature gates between two runners:
//! - Windowed: winit event loop with full rendering, input, and hot-reload
//! - Headless: bare update loop with no window (benchmarks, CI)

mod file_watcher; // Low-level filesystem polling primitive
mod headless;
mod hot_reload;
mod paths;
mod runtime;

use crate::hot_reload::{check_and_reload, create_file_watchers, try_remove_files_starting_with};
use crate::paths::*;
use crate::runtime::{RuntimeCreateContext, RuntimeHost};
use anyhow::{bail, Context, Result};
use config::Config;
use pill_core::{info, set_log_levels, LogContext, PillStyle};
use std::ffi::CString;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, ElementState, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Fullscreen, Icon, Window, WindowAttributes},
};

// ---------------------------------------------------------------------------
// Input Encoding
// ---------------------------------------------------------------------------

/// Maps winit's MouseButton enum to a stable u32 for the FFI boundary.
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

// ---------------------------------------------------------------------------
// Configuration & Window Setup
// ---------------------------------------------------------------------------

/// Reads the LOG_LEVELS key from the project's config.ini and applies
/// it to the pill_core logger.  Falls back to built-in defaults when
/// the key is missing.
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

/// Loads an icon from disk and converts it to a winit Icon.
/// Returns None if the file is missing or cannot be decoded.
pub fn load_window_icon(path: &Path) -> Option<Icon> {
    let image = image::open(path).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

/// Builds the WindowInit descriptor from the project configuration.
/// Sets title, size, fullscreen mode, and attempts to load a custom
/// window icon (falls back to the embedded default icon).
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

// ---------------------------------------------------------------------------
// Data Structures
// ---------------------------------------------------------------------------

/// Holds the winit window attributes and desired fullscreen state
/// before the window is actually created.
struct WindowInit {
    attributes: WindowAttributes,
    fullscreen: bool,
}

/// Top-level application state.
/// Owns the window, the runtime host, file watchers, and all path
/// configuration.  Implements winit's ApplicationHandler so it can
/// respond to window and device events.
struct App {
    project_paths: ProjectPaths,
    hot_reload_enabled: bool,
    runtime_load_mode: RuntimeLoadMode,
    window_init: Option<WindowInit>,

    window: Option<Arc<Window>>,
    window_size: winit::dpi::PhysicalSize<u32>,
    runtime_host: Option<RuntimeHost>,
    runtime_context: Option<RuntimeCreateContext>,
    file_watchers: Option<crate::hot_reload::FileWatchers>,
    last_render_time: Instant,
    last_reload_poll: Instant,
}

// ---------------------------------------------------------------------------
// Application - App
// ---------------------------------------------------------------------------

impl App {
    /// Constructs the App in a pre-initialised state.
    /// The window and runtime are created lazily when the event loop
    /// calls `resumed` for the first time. This avoids GPU work before
    /// the event loop is ready to drive rendering.
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

// ---------------------------------------------------------------------------
// Application - ApplicationHandler
// ---------------------------------------------------------------------------

impl ApplicationHandler for App {
    /// Called once when the event loop is ready.
    /// Creates the window, loads the runtime, initialises the engine,
    /// and sets up file watchers if hot-reload is enabled.
    ///
    /// This is the real "main" of the windowed app — everything before
    /// this point is path resolution and configuration loading.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Guard against multiple resumes — winit may call this more
        // than once on some platforms (e.g. after a suspend/resume).
        if self.window.is_some() {
            return;
        }

        // 1. Create the native window from the prepared attributes.
        //    The window starts hidden to avoid a white flash before
        //    the renderer draws the first frame.
        let init = self.window_init.take().expect("WindowInit missing");
        let window = Arc::new(
            event_loop
                .create_window(init.attributes)
                .expect("Failed to create window"),
        );

        // 2. Apply borderless fullscreen on the primary monitor if
        //    requested in the project configuration.
        if init.fullscreen {
            let monitor_handle = window.current_monitor();
            window.set_fullscreen(Some(Fullscreen::Borderless(monitor_handle)));
        }

        // Store the initial window size so the renderer can create
        // a swapchain matching the actual pixel dimensions.
        self.window_size = window.inner_size();

        // 3. Set up file watchers if hot-reload is active.  These
        //    monitor engine crate sources, project sources, resources,
        //    and the output dylib directory for changes.
        self.file_watchers = if self.hot_reload_enabled {
            Some(create_file_watchers(&self.project_paths))
        } else {
            None
        };

        // 4. Load the runtime dynamic library.  On Windows this opens
        //    pill_runtime.dll via libloading and fetches the FFI vtable
        //    so we can call into the runtime without linking against it.
        let mut runtime_host = RuntimeHost::load(
            &self.project_paths.runtime_dynamic_library_path,
            self.runtime_load_mode,
        )
        .expect("Failed to load runtime");

        // 5. Build the context object that holds everything the runtime
        //    needs to bootstrap the engine:
        //    - Paths to project resources and config.ini (as CStrings
        //      for the FFI boundary).
        //    - A clone of the window Arc so the runtime can create a
        //      wgpu Surface from the window handle.
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

        // Convert the project dylib path to a CString for the FFI call.
        let project_dylib_path = CString::new(
            self.project_paths
                .project_dynamic_library_path
                .to_string_lossy()
                .as_bytes(),
        )
        .expect("Failed to create pill project dylib path CString");

        // 6. Call into the runtime to create the engine.  This:
        //    - Loads the project dylib and calls PillProject::start().
        //    - Creates the wgpu Instance, Adapter, Device, and Surface.
        //    - Initialises the renderer, ECS, and resource manager.
        //    - The window is passed via Arc::into_raw / Arc::from_raw
        //      so ownership is shared across the FFI boundary.
        let args = runtime_context.make_args(&project_dylib_path, self.window_size);
        runtime_host
            .create(&args)
            .expect("RuntimeHost.create failed");

        // 7. Make the window visible now that the engine and renderer
        //    are fully initialised — the very next RedrawRequested will
        //    produce a complete frame instead of a blank surface.
        window.set_visible(true);

        // Store everything in the App state so the event loop can
        // access them during subsequent callbacks.
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

    /// Main window event handler.
    /// Dispatches every event to the runtime for input processing,
    /// then handles engine updates, rendering, hot-reload polling,
    /// and shutdown.
    ///
    /// RedrawRequested is the core frame driver — it computes delta time,
    /// advances the engine, checks exit conditions, and polls hot-reload.
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
                            Some(runtime_context),
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

// ---------------------------------------------------------------------------
// Entry Point
// ---------------------------------------------------------------------------

/// Main application entry point.
///
/// Responsibilities:
///   - Detect the project and engine workspace directories.
///   - Resolve paths to runtime and project dynamic libraries.
///   - Load configuration, set up logging, and create the winit window.
///   - Initialise the App and hand control to the winit event loop.
fn run_app() -> Result<()> {
    // --- 1. Detect project directory, run layout, and hot-reload mode ---
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

    // --- 2. Build all path information (build dir, resources, config) ---
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

    // --- 3. Determine runtime load mode (dylib vs in-process) ---
    // Decide how to load the runtime (dynamic library or in-process).
    let in_process = std::env::var("PILL_RUNTIME_IN_PROCESS").ok().as_deref() == Some("1");

    let runtime_load_mode = parse_runtime_load_mode(std::env::var("PILL_RUNTIME_MODE").ok())
        .or(in_process.then_some(RuntimeLoadMode::InProcess))
        .unwrap_or(if cfg!(target_os = "macos") {
            RuntimeLoadMode::InProcess
        } else {
            RuntimeLoadMode::Dylib
        });

    // --- 4. Resolve engine workspace (required for hot-reload, optional for dylib) ---
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

    // --- 5. Resolve paths to runtime and project dynamic libraries ---
    // Resolve paths to the runtime dynamic library.
    let runtime_dynamic_library_path = if runtime_load_mode == RuntimeLoadMode::Dylib {
        crate::runtime::resolve_runtime_dylib(
            &build_data_directory_path,
            engine_source_directory_path.as_deref(),
            "pill_runtime",
        )?
    } else {
        build_data_directory_path.join(dylib("pill_runtime"))
    };

    let runtime_dynamic_library_hot_reloaded_path =
        if hot_reload_enabled && runtime_load_mode == RuntimeLoadMode::Dylib {
            crate::runtime::resolve_runtime_dylib_optional(
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

    // --- 6. Assemble ProjectPaths and clean up stale hot-reload artifacts ---
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

    // --- 7. Load project configuration and initialize logging ---
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
            LogContext::HotReload => "Hot-reload enabled (watching src: {}, res: {})",
            project_paths.project_source_directory_path.display(),
            project_paths.project_resources_directory_path.display()
        );
    } else {
        info!(LogContext::HotReload => "Hot-reload disabled");
    }
    info!(
        "Initializing {} ({:?} layout, {:?} runtime)",
        "Standalone".module_object_style(),
        run_layout,
        runtime_load_mode
    );

    // --- 8. Build window descriptor from config, create event loop, launch ---
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

    // --- Graceful shutdown ---
    // The last frame's GPU commands may still be in-flight when the
    // event loop exits.  Give the GPU a chance to drain its command
    // queue before we tear down the wgpu Surface during runtime drop.
    std::thread::sleep(Duration::from_millis(100));

    // Explicitly destroy the runtime host now, while the event loop
    // and window are both still alive. This ensures wgpu can clean
    // up its surface resources before the native window is destroyed.
    drop(app.runtime_host.take());

    Ok(())
}

/// Process entry point.
/// Dispatches to headless or windowed runner depending on compile-time feature.
/// Prints any fatal error to stderr and exits with code 1 on failure.
fn main() {
    #[cfg(feature = "headless")]
    {
        if let Err(error) = headless::run_app_headless() {
            eprintln!("Error: {error:#}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(not(feature = "headless"))]
    {
        if let Err(error) = run_app() {
            eprintln!("Error: {error:#}");
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Unit tests for path resolution and launcher discovery logic.
    //! Each test creates a temporary directory structure mimicking a real
    //! Pill project layout and verifies the resolution functions behave correctly.

    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Creates a unique temporary directory for a test, keyed by name and
    /// nanosecond timestamp to prevent collisions between parallel test runs.
    fn unique_temporary_directory(name: &str) -> std::path::PathBuf {
        let nanoseconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("pill_test_{name}_{nanoseconds}"))
    }

    /// Verifies that the workspace declared in the project's Cargo.toml
    /// (via `workspace = "..."`) takes priority over both the
    /// PILL_ENGINE_WORKSPACE_DIR env var and filesystem scanning.
    #[test]
    fn prefers_project_manifest_workspace_over_env_and_sibling_scan() {
        let root = unique_temporary_directory("hot_reload_workspace_pick");
        let _ = fs::remove_dir_all(&root);

        let project_directory = root.join("my_project");
        fs::create_dir_all(project_directory.join("src")).unwrap();
        fs::create_dir_all(project_directory.join("res")).unwrap();

        let engine_a = root.join("Pill-Engine").join("engine");
        let engine_b = root.join("Pill-Engine-Upstream").join("engine");
        fs::create_dir_all(engine_a.join("pill_core")).unwrap();
        fs::create_dir_all(engine_a.join("pill_engine")).unwrap();
        fs::create_dir_all(engine_a.join("pill_renderer")).unwrap();
        fs::create_dir_all(engine_b.join("pill_core")).unwrap();
        fs::create_dir_all(engine_b.join("pill_engine")).unwrap();
        fs::create_dir_all(engine_b.join("pill_renderer")).unwrap();

        // Both engine workspaces claim the project as a member.
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

        // The project manifest points to engine_b.
        fs::write(
            project_directory.join("Cargo.toml"),
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

        // The env var points to engine_a, but the manifest should win.
        std::env::set_var("PILL_ENGINE_WORKSPACE_DIR", &engine_a);
        let resolved =
            resolve_engine_workspace_dir(&project_directory, &project_directory, true).unwrap();
        assert_eq!(resolved, engine_b);

        let _ = fs::remove_dir_all(root);
    }

    /// Verifies that resolve_launcher_command prefers a compiled
    /// PillLauncher binary in the engine workspace over a PATH fallback.
    #[test]
    fn resolve_launcher_prefers_engine_pill_launcher_target_binary() {
        let root = unique_temporary_directory("hot_reload_launcher_pick");
        let _ = fs::remove_dir_all(&root);

        let engine_directory = root.join("engine");
        let launcher_binary = engine_directory
            .join("pill_launcher")
            .join("target")
            .join("debug")
            .join("PillLauncher");
        fs::create_dir_all(launcher_binary.parent().unwrap()).unwrap();
        fs::write(&launcher_binary, b"").unwrap();

        // Clear env overrides so the filesystem fallback is tested.
        std::env::remove_var("PILL_LAUNCHER_BIN");
        std::env::remove_var("PILL_LAUNCHER_CMD");

        // Verify the binary exists at the expected path.
        assert!(launcher_binary.exists());

        let _ = fs::remove_dir_all(root);
    }
}
