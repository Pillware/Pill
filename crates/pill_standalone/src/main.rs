#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

use pill_core::PillStyle;
use pill_engine::internal::*;
use pill_renderer;
use winit::{event::{Event, WindowEvent}, platform::windows::{WindowBuilderExtWindows, IconExtWindows}};

use std::{io::{Write, BufReader, BufRead}, fs::File, env};
use log::{ debug, info };
use ini::Ini;

pub const LOG_LEVEL: log::LevelFilter = log::LevelFilter::Debug;

fn main() {
    
    // Configure logging
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
        .filter_module("pill_standalone", LOG_LEVEL)
        .filter_module("pill_engine", LOG_LEVEL)
        .filter_module("pill_renderer", LOG_LEVEL)
        .init();

    info!("Initializing {}", "Standalone".mobj_style());
    
    // Get path to standalone's cargo.toml
    // If we run pill_launcher.exe, then it is located in Pill-Engine/scripts
    // If we run game exe, then the engine path is in config located in parent diectory, and it's Pill-Engine/crates/pill_standalone
    // Assumption: game exe is not called "pill_launcher.exe"
    let mut current_path = std::env::current_exe().unwrap();
    let mut standalone_cargo_path = std::path::PathBuf::new();
    let path_tail = current_path.file_stem().unwrap().to_str().unwrap();

    match path_tail {
        "pill_standalone" => {
            current_path.pop();
            current_path.pop();
            current_path.pop();
            current_path.push("crates");
            current_path.push("pill_standalone");
            current_path.push("Cargo.toml");
            standalone_cargo_path.push(current_path);
        },
        _ => {
            current_path.pop();
            current_path.push("config.ini");
            println!("{:?}", current_path);
            let config_file = Ini::load_from_file(current_path).unwrap();
            for (_, properties) in config_file.iter() {
                for (key, value) in properties.iter() {
                    match key {
                        "PATH" => standalone_cargo_path.push(value),
                        _ => {}
                    }
                }
            }
            standalone_cargo_path.push("crates");
            standalone_cargo_path.push("pill_standalone");
            standalone_cargo_path.push("Cargo.toml");
        }
    }
    
    println!("{:?}", standalone_cargo_path.as_path());

    // Open cargo file
    let cargo_file = File::open(standalone_cargo_path).unwrap();

    // Read lines from cargo file
    let cargo_lines = BufReader::new(cargo_file).lines()
        .map(|x| x.unwrap()).collect::<Vec<String>>();
    
    // Prepare path for fetching data from game's config.ini
    let mut game_path = std::path::PathBuf::new();
    for line in cargo_lines {
        if line.contains("pill_game") {

            // Split the line by '"' character
            // The path is always going to be the second element of the vector
            let mut splits_from_line: Vec<String> = line.split('"').map(|s| s.to_string()).collect();
            println!("{:?}", splits_from_line);
            splits_from_line.pop();
            game_path.push(splits_from_line.pop().unwrap());
        }
    }

    // Set default window values 
    let mut window_title = env!("CARGO_PKG_NAME");
    let mut window_width = 600;
    let mut window_height = 600;

    // Set default engine values
    let mut max_entity_count = 1000;
    let mut max_render_queue_capacity = 1000;
    let mut max_ambient_sink_count = 10;
    let mut max_spatial_sink_count = 10;

    // Set default renderer values
    let mut max_pipelines_count = 10;
    let mut max_textures_count = 10;
    let mut max_materials_count = 10;
    let mut max_meshes_count = 10;
    let mut max_cameras_count = 10;

    // Read game's config.ini
    let mut config_path = std::path::PathBuf::new();
    config_path.push(game_path.clone());
    config_path.push("config.ini");

    // Pass in the values read from config file
    let mut is_game_fullscreen = false;
    let config_file = Ini::load_from_file(config_path).unwrap();
    for (_, properties) in config_file.iter() {
        for (key, value) in properties.iter() {
            match key {
                "NAME" => window_title = value,
                "WINDOW_WIDTH" => window_width = value.parse::<u32>().unwrap_or(600),
                "WINDOW_HEIGTH" => window_height = value.parse::<u32>().unwrap_or(600),
                "FULLSCREEN" => is_game_fullscreen = value.parse::<bool>().unwrap_or(false),
                "MAX_ENTITY_COUNT" => max_entity_count = value.parse::<usize>().unwrap_or(1000),
                "MAX_RENDER_QUEUE_CAPACITY" => max_render_queue_capacity = value.parse::<usize>().unwrap_or(1000),
                "MAX_AMBIENT_SINK_COUNT" => max_ambient_sink_count = value.parse::<usize>().unwrap_or(10),
                "MAX_SPATIAL_SINK_COUNT" => max_spatial_sink_count = value.parse::<usize>().unwrap_or(10),
                "MAX_PIPELINES_COUNT" => max_pipelines_count = value.parse::<usize>().unwrap_or(10),
                "MAX_TEXTURES_COUNT" => max_textures_count = value.parse::<usize>().unwrap_or(10),
                "MAX_MATERIALS_COUNT" => max_materials_count = value.parse::<usize>().unwrap_or(10),
                "MAX_MESHES_COUNT" => max_meshes_count = value.parse::<usize>().unwrap_or(10),
                "MAX_CAMERAS_COUNT" => max_cameras_count = value.parse::<usize>().unwrap_or(10),
                _ => {}
            }
        }
    }

    // Read icon from game path
    let mut icon_path = std::path::PathBuf::new();
    icon_path.push(game_path.clone());
    icon_path.push("game.ico");
    let window_icon = match winit::window::Icon::from_path(icon_path, None) {
        Ok(icon) => Some(icon),
        Err(_) => None
    };

    // Init window
    let window_event_loop = winit::event_loop::EventLoop::new();
    
    // Init other window parameters
    let window_size = winit::dpi::PhysicalSize::<u32>::new(window_width, window_height);
    let window_min_size = winit::dpi::PhysicalSize::<u32>::new(100, 100);
    let window = winit::window::WindowBuilder::new()
        .with_title(window_title)
        .with_inner_size(window_size)
        .with_min_inner_size(window_min_size)
        .with_window_icon(window_icon.clone())
        .with_taskbar_icon(window_icon)
        .build(&window_event_loop)
        .unwrap();
    
    // Possibly set window to fullscreen
    let window_fullscreen_mode = match is_game_fullscreen {
        true => {
            let monitor_handle = window.current_monitor();
            Some(winit::window::Fullscreen::Borderless(monitor_handle))
        }
        false => None
    };
    window.set_fullscreen(window_fullscreen_mode);

    let mut last_render_time = std::time::Instant::now();

    // Init engine
    let game: Box<dyn PillGame> = Box::new(pill_game::Game { path: String::from(game_path.to_str().unwrap())});
    let renderer: Box<dyn PillRenderer> = Box::new(<pill_renderer::Renderer as PillRenderer>::new(&window, max_pipelines_count, max_textures_count, max_materials_count, max_meshes_count, max_cameras_count));
    let mut engine = Engine::new(game, renderer, max_render_queue_capacity, max_entity_count, max_ambient_sink_count, max_spatial_sink_count);
    engine.initialize(window_size);

    // Run loop
    window_event_loop.run(move |event, _, control_flow|  { // Run function takes closure
        *control_flow = winit::event_loop::ControlFlow::Poll; 
        match event {
            Event::MainEventsCleared => {
                window.request_redraw();
            },

            // Raw events not associated with any specific window
            // Event::DeviceEvent {
            //     ref event,
            //     .. // We're not using device_id currently
            // } => {
            //     state.input(event);
            // }

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



                    // Change window scale factor
                    // WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                    //     state.resize(**new_inner_size);
                    // },


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

