mod file_watcher;
use crate::file_watcher::FileWatcher;
use anyhow::{Context, Ok, Result};
use config::Config;
use pill_core::{info, set_log_levels, warn, EngineError, LogContext, PillStyle};
use pill_engine::internal::*;
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Fullscreen, Icon, Window, WindowAttributes},
};

use libloading::{Library, Symbol};
use std::ffi::c_void;
use std::{
    fs::{remove_file, rename},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const RELOAD_COOLDOWN: Duration = Duration::from_millis(1000);

struct WindowInit {
    attributes: WindowAttributes,
    size: winit::dpi::PhysicalSize<u32>,
    fullscreen: bool,
}

struct FileWatchers {
    game_dynamic_library_files_watcher: FileWatcher,
    game_project_source_files_watcher: FileWatcher,
    game_project_resources_files_watcher: FileWatcher,
}

struct ProjectPaths {
    build_data_directory_path: PathBuf,
    game_project_directory_path: PathBuf,
    game_resources_directory_path: PathBuf,
    game_source_directory_path: PathBuf,
    config_path: PathBuf,
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

fn configure_logging(config: &Config) {
    let (log_level, using_default_log_levels) = match config.get_str("LOG_LEVELS") {
        std::result::Result::Ok(val) => (val, false),
        Err(_) => {
            info!("xzxxx");
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

fn make_window_init(config: &Config, game_resources_directory_path: PathBuf) -> WindowInit {
    let window_title = config
        .get_str("WINDOW_TITLE")
        .context(EngineError::InvalidGameConfig())
        .unwrap();

    let window_width = config.get_int("WINDOW_WIDTH").unwrap_or(1280) as u32;
    let window_height = config.get_int("WINDOW_HEIGHT").unwrap_or(720) as u32;
    let fullscreen = config.get_bool("WINDOW_FULLSCREEN").unwrap_or(false);

    let default_icon_bytes = include_bytes!("../res/icon.raw");
    let game_icon_path = game_resources_directory_path.join("icon.ico");
    let window_icon = load_window_icon(&game_icon_path)
        .or_else(|| Icon::from_rgba(default_icon_bytes.to_vec(), 128, 128).ok());

    let size = winit::dpi::PhysicalSize::new(window_width, window_height);
    let min_size = winit::dpi::PhysicalSize::new(100, 100);

    let attributes = WindowAttributes::default()
        .with_title(window_title)
        .with_min_inner_size(min_size)
        .with_window_icon(window_icon)
        .with_visible(false);

    WindowInit {
        attributes,
        size,
        fullscreen,
    }
}

fn load_game_dynamic_library(library_path: &PathBuf) -> (Library, Box<dyn PillGame>) {
    type CreateGameFn = unsafe extern "C" fn() -> *mut c_void;
    let game_dynamic_library = unsafe {
        Library::new(library_path)
            .context(format!(
                "Failed to load game dynamic library at {}",
                library_path.display()
            ))
            .unwrap()
    };
    let get_game_function: Symbol<CreateGameFn> =
        unsafe { game_dynamic_library.get(b"get_game").unwrap() };
    let game = unsafe { *Box::from_raw(get_game_function() as *mut Box<dyn PillGame>) };
    (game_dynamic_library, game)
}

fn build_standalone_and_game_crates(project_paths: &ProjectPaths) -> Result<()> {
    // Locate Pill Launcher manifest directly
    let launcher_manifest = project_paths
        .game_project_directory_path
        .parent()
        .unwrap() // …/examples
        .parent()
        .unwrap() // …/<Pill-Engine>
        .join("engine")
        .join("pill_launcher")
        .join("Cargo.toml");

    let output = std::process::Command::new("cargo")
        .args([
            "run",
            "--quiet",
            "--manifest-path",
            launcher_manifest.to_str().unwrap(),
            "--",
            "-a",
            "build",
            "-p",
            project_paths.game_project_directory_path.to_str().unwrap(),
            "-c",
            "hot-reload",
        ])
        // run inside the engine workspace so relative paths in Cargo.toml work
        .current_dir(launcher_manifest.parent().unwrap().parent().unwrap()) // …/<Pill-Engine>/engine
        .output()
        .context("failed to invoke PillLauncher via `cargo run`")?;

    if !output.status.success() {
        warn!(LogContext::HotReload => "Rebuilding game project failed:\n{}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn check_and_reload_game(
    engine: &mut Option<Engine>,
    game_dynamic_library: &mut Option<Library>,
    project_paths: &ProjectPaths,
    last_reload_time: &mut Instant,
    window_size: &winit::dpi::PhysicalSize<u32>,
    window: &Arc<winit::window::Window>,
    config: &Config,
    file_watchers: &mut FileWatchers,
) -> Result<()> {
    let now: Instant = Instant::now();
    let reload_cooldown = RELOAD_COOLDOWN; // Duration::from_millis(1000);

    if now.duration_since(*last_reload_time) < reload_cooldown {
        return Ok(());
    }

    // Check for game project source files changes
    if let Some(paths) = file_watchers
        .game_project_source_files_watcher
        .get_changes()
    {
        info!(LogContext::HotReload => "Game project source file change detected: {:?}", paths);
        build_standalone_and_game_crates(project_paths)?;
    }

    // Check for game project resource files changes
    if let Some(paths) = file_watchers
        .game_project_resources_files_watcher
        .get_changes()
    {
        info!(LogContext::HotReload => "Game project resource file change detected: {:?}", paths);
        build_standalone_and_game_crates(project_paths)?;
    }

    // Check for game dynamic library changes
    if file_watchers
        .game_dynamic_library_files_watcher
        .get_changes()
        .is_some()
    {
        info!(LogContext::HotReload => "Reloading game project...");

        // Shutdown and drop engine
        if let Some(mut engine) = engine.take() {
            engine.shutdown();
        }

        drop(game_dynamic_library.take()); // Unload current game dynamic library

        // Remove old game dynamic library file and rename new one
        remove_file(&project_paths.game_dynamic_library_path).unwrap();
        rename(
            &project_paths.game_dynamic_library_hot_reloaded_path,
            &project_paths.game_dynamic_library_path,
        )
        .unwrap();

        // Load new game dynamic library
        let (game_library, game) =
            load_game_dynamic_library(&project_paths.game_dynamic_library_path);
        let renderer: Box<dyn PillRenderer> = Box::new(
            <pill_renderer::Renderer as PillRenderer>::new(Arc::clone(window), config.clone())
                .unwrap(),
        );
        let mut new_engine = Engine::new(
            game,
            project_paths.game_resources_directory_path.clone(),
            renderer,
            config.clone(),
        );
        new_engine.initialize(Some(*window_size)).unwrap();
        *engine = Some(new_engine);
        *game_dynamic_library = Some(game_library);

        // Run again to clear changes (otherwise it will trigger reload again since file was renamed)
        file_watchers
            .game_dynamic_library_files_watcher
            .get_changes();

        *last_reload_time = now;
    }

    Ok(())
}

fn create_file_watchers(project_paths: &ProjectPaths) -> FileWatchers {
    let game_dynamic_library_files_watcher =
        FileWatcher::new(project_paths.build_data_directory_path.clone());
    let game_project_source_files_watcher =
        FileWatcher::new(project_paths.game_source_directory_path.clone()).set_recursive(true);
    let game_project_resources_files_watcher =
        FileWatcher::new(project_paths.game_resources_directory_path.clone()).set_recursive(true);

    FileWatchers {
        game_dynamic_library_files_watcher,
        game_project_source_files_watcher,
        game_project_resources_files_watcher,
    }
}

struct App {
    project_paths: ProjectPaths,
    config: Config,
    development_mode: bool,
    window_init: Option<WindowInit>,

    // runtime state
    window: Option<Arc<Window>>,
    window_size: winit::dpi::PhysicalSize<u32>,
    engine: Option<Engine>,
    game_dynamic_library: Option<Library>,
    file_watchers: Option<FileWatchers>,
    last_render_time: Instant,
    last_reload_time: Instant,
}

impl App {
    fn new(
        project_paths: ProjectPaths,
        config: Config,
        development_mode: bool,
        window_init: WindowInit,
    ) -> Self {
        Self {
            project_paths,
            config,
            development_mode,
            window_init: Some(window_init),

            window: None,
            window_size: winit::dpi::PhysicalSize::new(0, 0),
            engine: None,
            game_dynamic_library: None,
            file_watchers: None,
            last_render_time: Instant::now(),
            last_reload_time: Instant::now(),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // resumed() can be called again on some platforms; don’t recreate everything
        if self.window.is_some() {
            return;
        }

        let init = self.window_init.take().expect("WindowInit missing");
        self.window_size = init.size;

        let window = Arc::new(
            event_loop
                .create_window(init.attributes)
                .expect("Failed to create window"),
        );

        if init.fullscreen {
            let mh = window.current_monitor();
            window.set_fullscreen(Some(Fullscreen::Borderless(mh)));
        }

        // dev watchers
        self.file_watchers = if self.development_mode {
            Some(create_file_watchers(&self.project_paths))
        } else {
            None
        };

        // Load game dylib + create engine (now we have a window)
        let (game_library, game) =
            load_game_dynamic_library(&self.project_paths.game_dynamic_library_path);
        self.game_dynamic_library = Some(game_library);

        let renderer: Box<dyn PillRenderer> = Box::new(
            <pill_renderer::Renderer as PillRenderer>::new(
                Arc::clone(&window),
                self.config.clone(),
            )
            .unwrap(),
        );

        let mut engine = Engine::new(
            game,
            self.project_paths.game_resources_directory_path.clone(),
            renderer,
            self.config.clone(),
        );
        engine.initialize(Some(self.window_size)).unwrap();
        self.engine = Some(engine);

        window.set_visible(true);
        self.window = Some(window);
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if let Some(e) = self.engine.as_mut() {
                e.pass_mouse_delta_input(&delta);
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

        if let Some(e) = self.engine.as_mut() {
            e.pass_input_to_egui(&event);
        }

        match event {
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let delta = now - self.last_render_time;
                self.last_render_time = now;

                if let Some(e) = self.engine.as_mut() {
                    e.update(delta);
                }

                if self.development_mode {
                    if let Some(watchers) = self.file_watchers.as_mut() {
                        check_and_reload_game(
                            &mut self.engine,
                            &mut self.game_dynamic_library,
                            &self.project_paths,
                            &mut self.last_reload_time,
                            &self.window_size,
                            window,
                            &self.config,
                            watchers,
                        )
                        .unwrap();
                    }
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(e) = self.engine.as_mut() {
                    e.pass_keyboard_key_input(&event);
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                if let Some(e) = self.engine.as_mut() {
                    e.pass_mouse_key_input(&button, &state);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(e) = self.engine.as_mut() {
                    e.pass_mouse_wheel_input(&delta);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(e) = self.engine.as_mut() {
                    e.pass_mouse_position_input(&position);
                }
            }
            WindowEvent::Resized(size) => {
                self.window_size = size;
                if let Some(e) = self.engine.as_mut() {
                    e.resize(size);
                }
            }
            WindowEvent::CloseRequested => {
                if let Some(mut e) = self.engine.take() {
                    e.shutdown();
                }
                event_loop.exit();
            }
            _ => {}
        }
    }
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
    let game_dynamic_library_path = build_data_directory_path.join(dylib("pill_game"));
    let game_dynamic_library_hot_reloaded_path =
        build_data_directory_path.join(dylib("pill_game_hot_reloaded"));

    let project_paths = ProjectPaths {
        build_data_directory_path,
        game_project_directory_path,
        game_source_directory_path,
        game_resources_directory_path,
        config_path,
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
    let window_init =
        make_window_init(&config, project_paths.game_resources_directory_path.clone());

    let event_loop = EventLoop::new().unwrap();

    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(project_paths, config, development_mode, window_init);

    event_loop.run_app(&mut app).unwrap();
}
