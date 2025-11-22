mod file_watcher;
use crate::file_watcher::FileWatcher;
use config::Config;
use pill_core::{ info, set_log_levels, warn, EngineError, LogContext, PillStyle };
use pill_engine::internal::*;
use anyhow::{ Context, Ok, Result };
use winit::{
    event::{ Event, WindowEvent, DeviceEvent },
    window::{Icon},
};

#[cfg(target_os = "windows")]
use winit::platform::windows::IconExtWindows;
use std::{
    fs::{remove_file, rename}, path::{Path, PathBuf}, sync::Arc, time::{Duration, Instant}
};
use libloading::{Library, Symbol};
use std::ffi::c_void;

const RELOAD_COOLDOWN: Duration = Duration::from_millis(1000);

struct WindowData {
    window: Arc<winit::window::Window>,
    size: winit::dpi::PhysicalSize<u32>,
    event_loop: winit::event_loop::EventLoop<()>,
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
        Err(_) => { info!("xzxxx"); (pill_core::get_default_log_levels(), true) },
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
    // Fast path on Windows: let the OS decode common formats for us.
    #[cfg(target_os = "windows")]
    {
        let icon = Icon::from_path(path, None).ok()?;
        return Some(icon);
    }

    // Cross‑platform path: decode with the `image` crate.
    #[cfg(not(target_os = "windows"))]
    {
        let image = image::open(path).ok()?.into_rgba8();
        let (width, height) = image.dimensions();
        Icon::from_rgba(image.into_raw(), width, height).ok()
    }
}

fn create_window(config: &Config, game_resources_directory_path: PathBuf) -> WindowData {
    let window_title = config.get_str("WINDOW_TITLE").context(EngineError::InvalidGameConfig()).unwrap();
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
        window_event_loop.create_window(window_attributes)
            .context("Failed to create window")
            .unwrap()
        );

    // Possibly set window to fullscreen
    let window_fullscreen_mode = match window_fullscreen {
        true => {
            let monitor_handle = window.current_monitor();
            Some(winit::window::Fullscreen::Borderless(monitor_handle))
        }
        false => None
    };
    window.set_fullscreen(window_fullscreen_mode);

    WindowData { window, event_loop: window_event_loop, size: window_size }
}

fn load_game_dynamic_library(library_path: &PathBuf) -> (Library, Box<dyn PillGame>) {
    type CreateGameFn = unsafe extern "C" fn() -> *mut c_void;
    let game_dynamic_library = unsafe { Library::new(library_path).context(format!("Failed to load game dynamic library at {}", library_path.display())).unwrap() };
    let get_game_function: Symbol<CreateGameFn> = unsafe { game_dynamic_library.get(b"get_game").unwrap() };
    let game = unsafe { *Box::from_raw(get_game_function() as *mut Box<dyn PillGame>) };
    (game_dynamic_library, game)
}

fn build_standalone_and_game_crates(project_paths: &ProjectPaths) -> Result<()> {
    // Locate Pill Launcher manifest directly
    let launcher_manifest = project_paths
        .game_project_directory_path
        .parent().unwrap()          // …/examples
        .parent().unwrap()          // …/<Pill-Engine>
        .join("engine")
        .join("pill_launcher")
        .join("Cargo.toml");

    let output = std::process::Command::new("cargo")
        .args([
            "run", "--quiet",
            "--manifest-path", launcher_manifest.to_str().unwrap(),
            "--",
            "-a", "build",
            "-p", project_paths.game_project_directory_path.to_str().unwrap(),
            "-c", "hot-reload",
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
    if let Some(paths) = file_watchers.game_project_source_files_watcher.get_changes() {
        info!(LogContext::HotReload => "Game project source file change detected: {:?}", paths);
        build_standalone_and_game_crates(project_paths)?;
    }

    // Check for game project resource files changes
    if let Some(paths) = file_watchers.game_project_resources_files_watcher.get_changes() {
        info!(LogContext::HotReload => "Game project resource file change detected: {:?}", paths);
        build_standalone_and_game_crates(project_paths)?;
    }

    // Check for game dynamic library changes
    if file_watchers.game_dynamic_library_files_watcher.get_changes().is_some() {
        info!(LogContext::HotReload => "Reloading game project...");

        // Shutdown and drop engine
        if let Some(mut engine) = engine.take() {
            engine.shutdown();
        }

        drop(game_dynamic_library.take()); // Unload current game dynamic library

        // Remove old game dynamic library file and rename new one
        remove_file(&project_paths.game_dynamic_library_path).unwrap();
        rename(&project_paths.game_dynamic_library_hot_reloaded_path, &project_paths.game_dynamic_library_path).unwrap();

        // Load new game dynamic library
        let (game_library, game) = load_game_dynamic_library(&project_paths.game_dynamic_library_path);
        let renderer: Box<dyn PillRenderer> = Box::new(<pill_renderer::Renderer as PillRenderer>::new(Arc::clone(window), config.clone()).unwrap());
        let mut new_engine = Engine::new(game, project_paths.game_resources_directory_path.clone(), renderer, config.clone());
        new_engine.initialize(*window_size).unwrap();
        *engine = Some(new_engine);
        *game_dynamic_library = Some(game_library);

        // Run again to clear changes (otherwise it will trigger reload again since file was renamed)
        file_watchers.game_dynamic_library_files_watcher.get_changes();

        *last_reload_time = now;
    }

    Ok(())
}

fn create_file_watchers(project_paths: &ProjectPaths) -> FileWatchers {
    let game_dynamic_library_files_watcher = FileWatcher::new(project_paths.build_data_directory_path.clone());
    let game_project_source_files_watcher = FileWatcher::new(project_paths.game_source_directory_path.clone()).set_recursive(true);
    let game_project_resources_files_watcher = FileWatcher::new(project_paths.game_resources_directory_path.clone()).set_recursive(true);

    FileWatchers {
        game_dynamic_library_files_watcher,
        game_project_source_files_watcher,
        game_project_resources_files_watcher,
    }
}

fn main_loop(
    engine: &mut Option<Engine>,
    game_dynamic_library: &mut Option<Library>,
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
    let mut last_reload_time = Instant::now();

    // Main program loop
    let _ = window_data.event_loop.run(move |event, event_loop_window_target| { // Run function takes closure
        match event {
            Event::AboutToWait => {
                window_data.window.request_redraw();
            }

            // Handle device events
            Event::DeviceEvent {
                ref event,
                ..
            } => {
                if let DeviceEvent::MouseMotion {
                        delta,
                    } = event {
                    if let Some(ref mut engine) = engine {
                        engine.pass_mouse_delta_input(delta);
                    }
                }
            }

            // Handle window events
            Event::WindowEvent {
                ref event,
                window_id,
            }
            if window_id == window_data.window.id() => {
                if let Some(ref mut engine) = engine {
                    engine.pass_input_to_egui(event);
                }

                match event {
                    WindowEvent::RedrawRequested => {
                        let now = std::time::Instant::now();
                        let delta_time = now - last_render_time;
                        last_render_time = now;

                        if let Some(ref mut e) = engine {
                            e.update(delta_time);
                        }

                        if development_mode {
                            check_and_reload_game(
                                engine,
                                game_dynamic_library,
                                &project_paths,
                                &mut last_reload_time,
                                &window_data.size,
                                &window_data.window,
                                &config,
                                file_watchers.as_mut().unwrap(),
                            ).unwrap();
                        }
                    }
                    WindowEvent::KeyboardInput { event, .. } => {
                        if let Some(ref mut e) = engine {
                            e.pass_keyboard_key_input(event);
                        }
                    }
                    WindowEvent::MouseInput { button, state, .. } => {
                        if let Some(ref mut e) = engine {
                            e.pass_mouse_key_input(button, state);
                        }
                    }
                    WindowEvent::MouseWheel { delta, .. } => {
                        if let Some(ref mut e) = engine {
                            e.pass_mouse_wheel_input(delta);
                        }
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        if let Some(ref mut e) = engine {
                            e.pass_mouse_position_input(position);
                        }
                    }
                    WindowEvent::CloseRequested => {
                        if let Some(mut e) = engine.take() {
                            e.shutdown();
                        }
                        event_loop_window_target.exit();
                    }
                    WindowEvent::Resized(physical_size) => {
                        if let Some(ref mut e) = engine {
                            e.resize(*physical_size);
                        }
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

    let current_directory_path = std::env::current_exe().unwrap().parent().unwrap().to_path_buf(); // Path where the executable is located
    let game_project_directory_path = current_directory_path.parent().unwrap().parent().unwrap().to_path_buf();
    let build_data_directory_path = current_directory_path.join("data");
    let game_resources_directory_path = if development_mode {
        current_directory_path.parent().unwrap().parent().unwrap().join("res") // <GAME_PROJECT_ROOT>/res
    } else {
        build_data_directory_path.join("res") // <EXE_LOCATION>/data/res
    };
    let game_source_directory_path = current_directory_path.parent().unwrap().parent().unwrap().join("src"); // <GAME_PROJECT_ROOT>/src
    let config_path = game_resources_directory_path.join("config.ini");
    let game_dynamic_library_path = build_data_directory_path.join(dylib("pill_game"));
    let game_dynamic_library_hot_reloaded_path = build_data_directory_path.join(dylib("pill_game_hot_reloaded"));

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
    let _ = config.merge(config::File::with_name(project_paths.config_path.to_str().unwrap()));

    // Configure logging context and levels
    configure_logging(&config);
  
    info!("Initializing {}", "Standalone".module_object_style());

    // Create windows
    let window_data = create_window(&config, project_paths.game_resources_directory_path.clone());

    // Load game dynamic library
    let mut game_dynamic_library: Option<Library>;
    let (game_library, game) = load_game_dynamic_library(&project_paths.game_dynamic_library_path);
    game_dynamic_library = Some(game_library);

    // Initialize renderer and engine
    let renderer: Box<dyn PillRenderer> = Box::new(<pill_renderer::Renderer as PillRenderer>::new(Arc::clone(&window_data.window), config.clone()).unwrap());
    let mut engine: Option<Engine> = Some(Engine::new(game, project_paths.game_resources_directory_path.clone(), renderer, config.clone()));
    engine.as_mut().unwrap().initialize(window_data.size).context("Failed to initialize engine").unwrap();

    // Run loop
    window_data.event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    window_data.event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);

    // Show window (now the taskbar icon will be set correctly)
    window_data.window.set_visible(true);

    // Main program loop
    main_loop(
        &mut engine,
        &mut game_dynamic_library,
        project_paths,
        window_data,
        config,
        development_mode,
    ).context("Main loop failed").unwrap();
}
