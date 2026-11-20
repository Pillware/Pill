//! The browser frontend: a statically linked project running in a page's canvas.
//!
//! # Responsibilities
//!
//! - Start a shipped project ([`run`]): readable panics, console logging, the
//!   project's asset pack downloaded from beside the page, and the project's
//!   modules registered through `pill_player`.
//! - Put a `winit` window on the page's canvas, attach the renderer to it
//!   asynchronously, and run one frame per browser animation frame.
//! - Feed the canvas's keyboard and mouse events to the engine. The canvas is
//!   focusable and focused at start, so keys reach the game without a click.
//! - Provide the browser editor frontend (`pill_web_editor`) with the start-up
//!   and canvas helpers it shares with this one.
//!
//! # Design
//!
//! The web counterpart of `pill_standalone`'s shipping posture, over the same
//! `pill_player` and [`FrameDriver`]. Three things differ, all because the
//! browser only does work when control returns to its event loop: the event
//! loop is handed over with `spawn_app` instead of being run here, the
//! renderer attach is a future spawned onto that loop instead of a call that
//! blocks, and the asset pack is downloaded from beside the page instead of
//! being embedded in the module, which the browser downloads whole. Frames
//! start once the attach resolves.
//!
//! The browser editor frontend (`pill_web_editor`, under `modules/editor/`) is
//! a separate crate that builds on this one's start-up and canvas helpers.
//!
//! The crate is empty on every other target: its dependencies are wasm-only,
//! so the workspace's native builds do not see it.

#![cfg(target_arch = "wasm32")]

// Standard library
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_core::{error, info, warn};
use pill_input::winit_events::{translate_device_event, translate_window_event};
use pill_player::{
    report_failure, FrameDriver, RenderingRuntime, Runtime, StaticProject, SurfaceId,
    WindowedFrameDriver,
};
use wasm_bindgen::JsCast;
use web_sys::HtmlCanvasElement;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
use winit::window::{Window, WindowId};

// =============================================================================
// WebApplication
// =============================================================================

/// The application state `winit` keeps for as long as the page lives.
struct WebApplication {
    /// The element id of the canvas to draw on.
    canvas_id: String,
    /// The started project, until the canvas window exists to attach it to.
    runtime: Option<Runtime>,
    /// The window on the canvas.
    window: Option<Arc<Window>>,
    /// The frame driver once the renderer is attached: the project and its
    /// renderer together. The attach future on the browser's event loop fills
    /// it in.
    driver: Rc<RefCell<Option<RenderingRuntime>>>,
}

impl ApplicationHandler for WebApplication {
    /// Create the canvas window and start attaching the renderer to it.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Step 1: Only once: a later resume keeps the window that exists.
        let Some(runtime) = self.runtime.take() else {
            return;
        };

        // Step 2: Put a window on the page's canvas.
        let canvas = match canvas_element(&self.canvas_id) {
            Ok(canvas) => canvas,
            Err(message) => {
                error!(
                    target: telemetry_target::ENGINE,
                    error = message.as_str(),
                    "canvas not found; the game does not start"
                );
                return;
            }
        };
        // The canvas's size as the page laid it out, in physical pixels. winit
        // measures the canvas only when its resize observer reports, which can
        // happen after the attach below.
        let canvas_laid_out_size = canvas_physical_size(&canvas);
        // Keyboard events only reach an element that can hold focus. Focusing
        // the canvas now saves the player a click before the first key works.
        let focus_target = canvas.clone();
        let attributes = Window::default_attributes()
            .with_canvas(Some(canvas))
            .with_focusable(true);
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => return report_failure("canvas window creation", &error),
        };
        if let Err(error) = focus_without_scrolling(&focus_target) {
            warn!(
                target: telemetry_target::ENGINE,
                error = ?error,
                "could not focus the canvas; click it to send keys"
            );
        }
        self.window = Some(Arc::clone(&window));

        // Step 3: Attach the renderer without blocking. The GPU device comes
        // from the browser, which answers only once control returns to its
        // event loop. WebGPU refuses a zero-sized surface, so a canvas winit
        // has not measured yet is attached at its laid-out size.
        let window_size = window.inner_size();
        let (width, height) = if window_size.width > 0 && window_size.height > 0 {
            (window_size.width, window_size.height)
        } else {
            canvas_laid_out_size
        };
        let driver = Rc::clone(&self.driver);
        pill_core::platform::futures::spawn(async move {
            let attach = pill_player::attach_renderer(runtime, Arc::clone(&window), width, height);
            match attach.await {
                Ok(mut attached) => {
                    // A resize that arrived during the attach went nowhere, so
                    // apply the canvas's size as winit measures it now. A zero
                    // is not applied: winit has not measured the canvas yet,
                    // and 0x0 reads as a minimized window.
                    let window_size = window.inner_size();
                    if window_size.width > 0
                        && window_size.height > 0
                        && (window_size.width, window_size.height) != (width, height)
                    {
                        attached.resize_surface(
                            SurfaceId::PRIMARY,
                            window_size.width,
                            window_size.height,
                        );
                    }
                    *driver.borrow_mut() = Some(attached);
                    info!(target: telemetry_target::RENDERING, "renderer attached to the canvas");
                    window.request_redraw();
                }
                Err(error) => report_failure("renderer attachment", &error),
            }
        });
    }

    /// Pass input and resizes on to the driver, and run one frame for each
    /// redraw request.
    ///
    /// Input that arrives before the renderer is attached has no game to go to
    /// and is dropped.
    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        for input in translate_window_event(&event) {
            if let Some(driver) = self.driver.borrow_mut().as_mut() {
                driver.push_input(input);
            }
        }
        match event {
            WindowEvent::Resized(size) => {
                if let Some(driver) = self.driver.borrow_mut().as_mut() {
                    driver.resize_surface(SurfaceId::PRIMARY, size.width, size.height);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    /// Pass on raw mouse motion, which the browser reports per device rather
    /// than per window.
    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let Some(input) = translate_device_event(&event) {
            if let Some(driver) = self.driver.borrow_mut().as_mut() {
                driver.push_input(input);
            }
        }
    }
}

impl WebApplication {
    /// Run one frame and ask the browser for the next one.
    ///
    /// Does nothing until the renderer is attached. A failed frame is reported
    /// and no further frame is requested, so the page stops on the error
    /// instead of repeating it every frame.
    fn redraw(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let mut driver = self.driver.borrow_mut();
        let Some(driver) = driver.as_mut() else {
            return;
        };
        match driver.run_frame() {
            Ok(()) => window.request_redraw(),
            Err(error) => report_failure("frame render", &error),
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Run `project` in the page's canvas with element id `canvas_id`.
///
/// Returns once the event loop is handed to the browser; the game then runs on
/// its animation frames. Every failure is reported to the browser console.
pub fn run(project: StaticProject, canvas_id: &str) {
    let canvas_id = canvas_id.to_owned();
    start_project(project, move |runtime| {
        // Step 1: Hand the event loop to the browser. The canvas window and
        // the renderer are created in `resumed`.
        let event_loop = match EventLoop::new() {
            Ok(event_loop) => event_loop,
            Err(error) => return report_failure("event loop creation", &error),
        };
        info!(target: telemetry_target::ENGINE, canvas = %canvas_id, "starting in the browser");
        event_loop.spawn_app(WebApplication {
            canvas_id: canvas_id.clone(),
            runtime: Some(runtime),
            window: None,
            driver: Rc::new(RefCell::new(None)),
        });
    });
}

/// Start `project` and pass its running [`Runtime`] to `launch`.
///
/// Readable panics and console logging are set up first, because everything
/// after them can fail. A project with an `asset_pack_url` keeps its assets in
/// a pack beside the page instead of inside the module: that pack is
/// downloaded and mounted before the project initializes, so an init that
/// loads a `res` file finds it, exactly as it would with an embedded pack.
/// Both browser frontends share this, so the game and editor postures cannot
/// drift apart.
pub fn start_project<F>(project: StaticProject, launch: F)
where
    F: FnOnce(Runtime) + 'static,
{
    // Step 1: Set up readable panics and console logging before anything can
    // fail.
    console_error_panic_hook::set_once();
    if let Err(error) = pill_player::init_telemetry(None) {
        web_sys::console::error_1(&format!("[web] telemetry failed to start: {error}").into());
    }

    // Step 2: With no pack URL, `pill_player::setup` starts the project and
    // mounts the pack it embeds, if there is one.
    let Some(url) = project.asset_pack_url else {
        match pill_player::setup(project) {
            Ok(runtime) => launch(runtime),
            Err(error) => report_failure("project setup", &error),
        }
        return;
    };

    // Step 3: With a pack URL, download the pack, mount it, then start the
    // project. The download runs on the browser's event loop, so the page
    // keeps painting while it waits.
    pill_core::platform::futures::spawn(async move {
        let bytes = match fetch_asset_pack(url).await {
            Ok(bytes) => bytes,
            Err(message) => {
                error!(target: telemetry_target::ENGINE, error = %message, "asset pack download failed");
                return;
            }
        };
        let database = match pill_player::mount_asset_pack(
            project.name,
            pill_player::PackSource::Fetched,
            bytes,
        ) {
            Ok(database) => database,
            Err(error) => return report_failure("asset pack mount", &error),
        };
        match pill_player::setup_with_asset_database(project, Some(database)) {
            Ok(runtime) => launch(runtime),
            Err(error) => report_failure("project setup", &error),
        }
    });
}

/// Download the asset pack at `url`, relative to the page.
///
/// The whole file arrives as one buffer: a pack is a single file with an index
/// at the front, so there is nothing to stream before it is parsed.
///
/// # Errors
///
/// Returns a description of the failure: the page has no window, the fetch
/// throws or answers with something that is not a response, the server answers
/// with an error status, or the body cannot be read.
async fn fetch_asset_pack(url: &str) -> Result<Vec<u8>, String> {
    // Step 1: Send the request and turn a missing window, a failed fetch or a
    // bad status into an error message.
    let window = web_sys::window().ok_or_else(|| "the page has no window".to_owned())?;
    let response = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
        .await
        .map_err(|error| format!("{url}: {error:?}"))?;
    let response: web_sys::Response = response
        .dyn_into()
        .map_err(|_| format!("{url}: the fetch did not answer with a response"))?;
    if !response.ok() {
        return Err(format!("{url}: HTTP {}", response.status()));
    }

    // Step 2: Read the whole body as one buffer.
    let buffer = wasm_bindgen_futures::JsFuture::from(
        response
            .array_buffer()
            .map_err(|error| format!("{url}: {error:?}"))?,
    )
    .await
    .map_err(|error| format!("{url}: {error:?}"))?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// The page's canvas element with id `canvas_id`.
///
/// # Errors
///
/// Returns a message saying what is wrong: the page has no document, it has no
/// element with that id, or the element is not a canvas.
pub fn canvas_element(canvas_id: &str) -> Result<HtmlCanvasElement, String> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or("the page has no document")?;
    let element = document
        .get_element_by_id(canvas_id)
        .ok_or_else(|| format!("the page has no element with id `{canvas_id}`"))?;
    element
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| format!("the element `{canvas_id}` is not a canvas"))
}

/// Focus `canvas` without scrolling it into view.
///
/// A plain `focus()` scrolls the element into view, and that cancels any
/// scroll already running (for example a smooth scroll to an anchor below the
/// canvas). `web-sys` has no typed `focus(options)`, so the call goes through
/// raw JS.
///
/// # Errors
///
/// Returns an error when the options cannot be set, when the element has no
/// `focus` function, or when the call throws.
pub fn focus_without_scrolling(canvas: &HtmlCanvasElement) -> Result<(), wasm_bindgen::JsValue> {
    let canvas: &wasm_bindgen::JsValue = canvas.as_ref();
    let focus_options = js_sys::Object::new();
    js_sys::Reflect::set(
        &focus_options,
        &wasm_bindgen::JsValue::from_str("preventScroll"),
        &wasm_bindgen::JsValue::TRUE,
    )?;

    let focus = js_sys::Reflect::get(canvas, &wasm_bindgen::JsValue::from_str("focus"))?;
    let focus = focus
        .dyn_into::<js_sys::Function>()
        .map_err(|_| wasm_bindgen::JsValue::from_str("focus is not a function"))?;
    focus.call1(canvas, &focus_options)?;

    Ok(())
}

/// The canvas's size as the page laid it out, in physical pixels; never
/// smaller than 1x1.
pub fn canvas_physical_size(canvas: &HtmlCanvasElement) -> (u32, u32) {
    let device_pixel_ratio = web_sys::window().map_or(1.0, |window| window.device_pixel_ratio());
    let to_physical_pixels = |css_pixels: i32| {
        (f64::from(css_pixels) * device_pixel_ratio)
            .round()
            .max(1.0) as u32
    };
    (
        to_physical_pixels(canvas.client_width()),
        to_physical_pixels(canvas.client_height()),
    )
}
