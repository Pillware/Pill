use std::sync::Arc;

use wasm_bindgen::prelude::*;
use winit::{
    dpi::PhysicalSize,
    event::{DeviceEvent, Event, WindowEvent},
    event_loop::EventLoop,
    window::WindowAttributes,
};

use pill_engine::internal::*;
use pill_renderer::Renderer;
use pill_game::WebGame;

#[wasm_bindgen(start)]
pub fn wasm_main() {
    std::panic::set_hook(Box::new(console_error_panic_hook::hook));
    console_log::init_with_level(log::Level::Debug).expect("Failed to init logger");

    log::info!(
        "pill_web boot: debug_assertions={}, arch={}",
        cfg!(debug_assertions),
        std::env::consts::ARCH
    );

    wasm_bindgen_futures::spawn_local(run());
}

async fn run() {
    let event_loop = EventLoop::new().expect("Failed to create event loop");

    let window = {
        use winit::platform::web::WindowAttributesExtWebSys;

        let canvas = web_sys::window()
            .and_then(|win| win.document())
            .and_then(|doc| doc.get_element_by_id("canvas"))
            .and_then(|el| el.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            .expect("Failed to find canvas element with id 'canvas'");

        // WebGPU surface creation requires non-zero dimensions. If the canvas
        // hasn't been laid out by CSS yet (pre-layout / hidden parent), the
        // attribute is 0 — clamp to 1 as a crash guard. The real 1280×720
        // fallback a few lines below kicks in once winit surfaces the size.
        let width = canvas.width().max(1);
        let height = canvas.height().max(1);

        let attrs = WindowAttributes::default()
            .with_canvas(Some(canvas))
            .with_inner_size(PhysicalSize::new(width, height));

        #[allow(deprecated)]
        event_loop
            .create_window(attrs)
            .expect("Failed to create window")
    };

    let window = Arc::new(window);
    let mut window_size = window.inner_size();
    
    // Fallback if winit returns 0 (can happen on web before first layout)
    if window_size.width == 0 || window_size.height == 0 {
        window_size = PhysicalSize::new(1280, 720);
    }

    // The game's config.ini is copied into this scratch dir by the launcher
    // (wasm has no filesystem, so we can't read it at runtime — embed it).
    let mut config = config::Config::default();
    const GAME_CONFIG_INI: &str = include_str!("../config.ini");
    if let Err(e) = config.merge(config::File::from_str(
        GAME_CONFIG_INI,
        config::FileFormat::Ini,
    )) {
        log::warn!("Failed to parse embedded config.ini: {e}");
    }
    let _ = config.set("WINDOW_WIDTH", window_size.width as i64);
    let _ = config.set("WINDOW_HEIGHT", window_size.height as i64);

    log::info!("Creating renderer...");
    let renderer: Box<dyn PillRenderer> =
        Box::new(Renderer::new_async(Arc::clone(&window), config.clone()).await.expect("Failed to create renderer"));

    log::info!("Creating engine...");
    let game: Box<dyn PillGame> = Box::new(WebGame {});

    let mut engine = Engine::new(
        game,
        std::path::PathBuf::from("res"),
        renderer,
        config,
    );

    log::info!("Initializing engine...");
    match engine.initialize(Some(window_size)) {
        Ok(()) => log::info!("engine.initialize() OK"),
        Err(e) => {
            log::error!("engine.initialize() FAILED: {:#}", e);
            panic!("engine init failed: {:#}", e);
        }
    }
    log::info!("Engine ready, starting event loop");

    let mut last_time = web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0);

    let window_clone = Arc::clone(&window);

    #[allow(deprecated)]
    let _ = event_loop.run(move |event, elwt| {
        match event {
            Event::AboutToWait => {
                window_clone.request_redraw();
            }

            Event::DeviceEvent { ref event, .. } => {
                if let DeviceEvent::MouseMotion { delta } = event {
                    engine.pass_mouse_delta_input(&delta);
                }
            }

            Event::WindowEvent { ref event, window_id } if window_id == window_clone.id() => {
                engine.pass_input_to_egui(event);

                match event {
                    WindowEvent::RedrawRequested => {
                        let now = web_sys::window()
                            .and_then(|w| w.performance())
                            .map(|p| p.now())
                            .unwrap_or(0.0);
                        let dt_ms = now - last_time;
                        last_time = now;
                        let dt = std::time::Duration::from_secs_f64(dt_ms / 1000.0);

                        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            engine.update(dt);
                        })) {
                            log::error!("engine.update() panicked: {:?}", e);
                        }
                    }
                    WindowEvent::KeyboardInput { event, .. } => {
                        engine.pass_keyboard_key_input(event);
                    }
                    WindowEvent::MouseInput { button, state, .. } => {
                        engine.pass_mouse_key_input(button, state);
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        engine.pass_mouse_position_input(position);
                    }
                    WindowEvent::Resized(physical_size) => {
                        if physical_size.width > 0 && physical_size.height > 0 {
                            engine.resize(*physical_size);
                        }
                    }
                    WindowEvent::CloseRequested => {
                        elwt.exit();
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    });
}
