#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

use pill_core::{ PillStyle, EngineError };
use pill_engine::internal::*;
use pill_renderer;

use anyhow::{ Error, Result, Context };
use winit::{
    event::{ Event, WindowEvent }, 
    platform::windows::{ WindowBuilderExtWindows, IconExtWindows },
};
use std::{
    io::{Write, BufReader, BufRead}, 
    fs::File, 
    env, 
    collections::HashMap, 
    path::PathBuf}
;
use log::{ debug, info, warn };

fn main() {
    // Get config file from game resource folder
    let current_path = env::current_dir().unwrap();
    let resource_folder_path = current_path.join("res");
    let config_path = resource_folder_path.join("config.ini");

    // Load config
    let mut config = config::Config::default();
    config.merge(config::File::with_name(config_path.to_str().unwrap())).unwrap();
     
    // Configure logging
    let log_level = config.get_str("LOG_LEVEL").unwrap_or("Info".to_string());
    let log_level = match log_level.as_str() {
        "Info" => log::LevelFilter::Info,
        "Warning" => log::LevelFilter::Warn,
        "Debug" => log::LevelFilter::Debug,
        "Error" => log::LevelFilter::Error,
        "Off" => log::LevelFilter::Off,
        _ => log::LevelFilter::Info,
    };

    #[cfg(debug_assertions)]
    env_logger::Builder::new()
        .format(|buf, record| {
            writeln!(buf, "[{}] {} {}:{}: {}",
                record.level(),
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
                record.file().unwrap_or("unknown"),
                record.line().unwrap_or(0),
                record.args()
            )
        })
        .filter_module("pill_standalone", log_level)
        .filter_module("pill_engine", log_level)
        .filter_module("pill_renderer", log_level)
        .init();

    info!("Initializing {}", "Standalone".mobj_style());
    
    // Read window config
    let window_title = config.get_str("WINDOW_TITLE").context(EngineError::InvalidGameConfig()).unwrap(); 
    let window_width = config.get_int("WINDOW_WIDTH").unwrap_or(1280) as u32;
    let window_height = config.get_int("WINDOW_HEIGHT").unwrap_or(720) as u32;
    let window_fullscreen = config.get_bool("WINDOW_FULLSCREEN").unwrap_or(false);

    let default_icon_bytes = include_bytes!("../res/icon.raw");
    let icon_path = resource_folder_path.join("icon.ico"); // Icon has to in res folder of the game and has to be named icon.ico
    let window_icon = match icon_path.exists() {
        true => match winit::window::Icon::from_path(icon_path, None) {
            Ok(icon) => Some(icon),
            Err(_) => { 
                warn!("Failed to load window icon"); 
                None 
            }
        },
        false => match winit::window::Icon::from_rgba(default_icon_bytes.to_vec(), 128, 128) { 
            Ok(icon) => Some(icon),
            Err(_) => { 
                warn!("Failed to load window icon"); 
                None 
            }
        }
    };

    // Init window
    let window_event_loop = winit::event_loop::EventLoop::new();
    
    // Initialize other window parameters
    let window_size = winit::dpi::PhysicalSize::<u32>::new(window_width, window_height);
    let window_min_size = winit::dpi::PhysicalSize::<u32>::new(100, 100);
    let window = winit::window::WindowBuilder::new()
        .with_title(window_title)
        .with_inner_size(window_size)
        .with_min_inner_size(window_min_size)
        .with_window_icon(window_icon.clone())
        .build(&window_event_loop)
        .context("Failed to initialize window").unwrap();
    
    // Possibly set window to fullscreen
    let window_fullscreen_mode = match window_fullscreen {
        true => {
            let monitor_handle = window.current_monitor();
            Some(winit::window::Fullscreen::Borderless(monitor_handle))
        }
        false => None
    };
    window.set_fullscreen(window_fullscreen_mode);

    let mut last_render_time = std::time::Instant::now();

    // Initialize engine
    let game: Box<dyn PillGame> = Box::new(pill_game::Game { });
    let renderer: Box<dyn PillRenderer> = Box::new(<pill_renderer::Renderer as PillRenderer>::new(&window, config.clone()));
    let mut engine = Engine::new(game, renderer, config.clone());
    engine.initialize(window_size).context("Failed to initialize engine").unwrap();

    // Run loop
    window_event_loop.run(move |event, _, control_flow| { // Run function takes closure
        *control_flow = winit::event_loop::ControlFlow::Poll; 
        match event {
            Event::MainEventsCleared => {
                window.request_redraw();
            }

            // Handle window events
            Event::WindowEvent {
                ref event,
                window_id,
            } 
            if window_id == window.id() => {
                match event {        

                    // Pass keyboard input to engine
                    WindowEvent::KeyboardInput { 
                        input,
                        .. // Skip other
                    } => { 
                        engine.pass_keyboard_key_input(&input);
                    },

                    // Pass mouse key input to engine
                    WindowEvent::MouseInput { 
                        button,
                        state,
                        .. // Skip other
                    } => { 
                        engine.pass_mouse_key_input( &button, &state);
                    },

                    // Pass mouse scroll input to engine
                    WindowEvent::MouseWheel { 
                        delta,
                        .. // Skip other
                    } => { 
                        engine.pass_mouse_wheel_input(&delta);
                    },

                    // Pass mouse motion input to engine
                    WindowEvent::CursorMoved {
                        position,
                        .. // Skip other
                    }=> { 
                        engine.pass_mouse_motion_input(&position);
                    },

                    // Close window
                    WindowEvent::CloseRequested => {
                        engine.shutdown();
                        *control_flow = winit::event_loop::ControlFlow::Exit
                    },

                    // Resize window
                    WindowEvent::Resized(physical_size) => {
                        engine.resize(*physical_size);
                    },

                    _ => {}
                }
            }

            // Handle redraw requests
            Event::RedrawRequested(_) => {
                let now = std::time::Instant::now();
                let delta_time = now - last_render_time;
                last_render_time = now;
                engine.update(delta_time);
            }
            _ => {}
        }
    });
}