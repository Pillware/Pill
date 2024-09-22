#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

use pill_core::{ PillStyle, EngineError };
use pill_engine::internal::*;
use pill_renderer;

use anyhow::{ Error, Result, Context };
use winit::{
    event::{ Event, WindowEvent, DeviceEvent }, 
    platform::windows::{ WindowBuilderExtWindows, IconExtWindows },
};
use std::{
    collections::HashMap, env, fs::File, io::{ BufRead, BufReader, Write }, path::PathBuf, sync::Arc
};
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
    let window_event_loop = winit::event_loop::EventLoop::new().unwrap();
    // Initialize other window parameters
    let window_size = winit::dpi::PhysicalSize::<u32>::new(window_width, window_height);
    let window_min_size = winit::dpi::PhysicalSize::<u32>::new(100, 100);
    let window = Arc::new(winit::window::WindowBuilder::new()
        .with_title(window_title)
        .with_inner_size(window_size)
        .with_min_inner_size(window_min_size)
        .with_window_icon(window_icon.clone())
        .build(&window_event_loop)
        .context("Failed to initialize window").unwrap());
    
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
    let windowx = Arc::clone(&window);
    let renderer: Box<dyn PillRenderer> = Box::new(<pill_renderer::Renderer as PillRenderer>::new(windowx, config.clone()));
    let mut engine = Engine::new(game, renderer, config.clone());
    engine.initialize(window_size).context("Failed to initialize engine").unwrap();

    // Run loop
    window_event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    window_event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
  
   
    let _ = window_event_loop.run(move |event, event_loop_window_target| { // Run function takes closure
        match event {
            Event::AboutToWait => {
                window.request_redraw();
            }

            // Handle device events
            Event::DeviceEvent {
                ref event,
                ..
            } => {
                match event {
                    DeviceEvent::MouseMotion { 
                        delta, 
                    } => {
                        engine.pass_mouse_delta_input(delta);
                    },
                    _ => {}
                }
            }

            // Handle window events
            Event::WindowEvent {
                ref event,
                window_id,
            } 
            if window_id == window.id() => {
                match event {    
                    WindowEvent::RedrawRequested => {
                        let now = std::time::Instant::now();
                        let delta_time = now - last_render_time;
                        last_render_time = now;
                        engine.update(delta_time);
                    }

                    WindowEvent::KeyboardInput { // Pass keyboard input to engine
                        event,
                        .. // Skip other
                    } => { 
                        engine.pass_keyboard_key_input(&event);
                    },
                    WindowEvent::MouseInput {   // Pass mouse key input to engine
                        button,
                        state,
                        .. // Skip other
                    } => { 
                        engine.pass_mouse_key_input(&button, &state);
                    },
                    WindowEvent::MouseWheel { // Pass mouse scroll input to engine
                        delta,
                        .. // Skip other
                    } => { 
                        engine.pass_mouse_wheel_input(&delta);
                    },
                    WindowEvent::CursorMoved { // Pass mouse motion input to engine
                        position,
                        .. // Skip other
                    }=> { 
                        engine.pass_mouse_position_input(&position);
                    },
                    WindowEvent::CloseRequested => { // Close window
                        engine.shutdown();
                        event_loop_window_target.exit();
                    },
                    WindowEvent::Resized(physical_size) => { // Resize window
                        engine.resize(*physical_size);
                    },
                    _ => {}
                }
            }
            _ => {}
        }
    });
}