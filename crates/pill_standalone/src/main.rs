#![cfg_attr(debug_assertions, allow(dead_code))]

use pill_core::PillStyle;
use pill_engine::internal::{ PillGame, PillRenderer, Engine };
use pill_renderer;

use winit::event::{Event, WindowEvent};
use log::{ info };
use std::io::Write;

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

    // Init window
    let window_event_loop = winit::event_loop::EventLoop::new();
    let window_title = env!("CARGO_PKG_NAME");
    let window_size = winit::dpi::PhysicalSize::<u32>::new(600, 600);
    let window_min_size = winit::dpi::PhysicalSize::<u32>::new(100, 100);
    let window = winit::window::WindowBuilder::new()
        .with_title(window_title)
        .with_inner_size(window_size)
        .with_min_inner_size(window_min_size)
        .build(&window_event_loop)
        .unwrap();
    let mut last_render_time = std::time::Instant::now();

    // Init engine
    let game: Box<dyn PillGame> = Box::new(pill_game::Game {});
    let renderer: Box<dyn PillRenderer> = Box::new(<pill_renderer::Renderer as PillRenderer>::new(&window));
    let mut engine = Engine::new(game, renderer);
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

