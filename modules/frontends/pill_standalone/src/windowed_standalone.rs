//! The windowed run loop: a `winit` window, the renderer on it, and one frame
//! per redraw.
//!
//! # Responsibilities
//!
//! - Run the project in a `winit` window: create the window, attach the
//!   renderer, and forward window and device events.
//! - Feed the run's input to the engine: keyboard and mouse from winit,
//!   gamepads polled before every frame, and rumble played once the frame is
//!   done.
//! - In a development build, start the two optional test modes: a second
//!   renderer surface (`PILL_SECOND_VIEW=1`, [`SecondView`]) and a picking
//!   test (`PILL_PICK_TEST=1`, [`PickTest`]).
//! - Tear the run down between two log lines, so a crash inside the teardown
//!   is visible in the log.
//!
//! # Design
//!
//! The loop is written against [`FrameDriver`], so the same code runs the
//! development host and a shipped game; [`crate::posture`] provides each
//! posture's project type, drivers and setup steps. Window-creation and setup
//! failures are deferred until the event loop exits, and each is reported
//! where it happens first. Input is forwarded through
//! [`FrameDriver::push_input`] as `pill_input` translates it, the same
//! translation the web uses.
//!
//! This file is compiled only in a windowed build: `main.rs` declares it
//! instead of `headless_standalone` while the `rendering` feature is on, which
//! is why nothing here repeats that feature on each item.

// Standard library
use std::sync::Arc;

// External crates
#[cfg(feature = "development")]
use pill_core::error;
use pill_core::info;
use pill_core::telemetry::telemetry_target;
use pill_input::gamepads::Gamepads;
use pill_input::winit_events::{translate_device_event, translate_window_event};
use pill_player::{report_failure, FrameDriver, WindowedFrameDriver};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

// Current crate
use crate::error::{FrontendError, RunError};
use crate::posture;

// =============================================================================
// Constants
// =============================================================================

/// Environment variable that opens a second window drawn as surface 1.
#[cfg(feature = "development")]
const SECOND_VIEW_ENVIRONMENT_VARIABLE: &str = "PILL_SECOND_VIEW";

/// Environment variable that closes the second window after that many seconds.
#[cfg(feature = "development")]
const SECOND_VIEW_SECONDS_ENVIRONMENT_VARIABLE: &str = "PILL_SECOND_VIEW_SECONDS";

/// The second view's surface id.
#[cfg(feature = "development")]
const SECOND_VIEW_SURFACE: pill_player::SurfaceId = pill_player::SurfaceId(1);

/// Environment variable that runs the picking test.
#[cfg(feature = "development")]
const PICK_TEST_ENVIRONMENT_VARIABLE: &str = "PILL_PICK_TEST";

/// Frames the picking test waits before it sends its first pick requests.
#[cfg(feature = "development")]
const PICK_TEST_WAIT_FRAMES: u32 = 30;

// =============================================================================
// SecondView
// =============================================================================

/// The second renderer surface: the same scene drawn through a second camera
/// in a separate window.
///
/// Its camera is a `CameraBinding::View`, which is plain data the driver
/// holds, not an entity in the world, so the view needs nothing added to the
/// project.
#[cfg(feature = "development")]
struct SecondView {
    /// The second window, kept alive while surface 1 draws on it.
    window: Arc<Window>,
    /// When to close the window, if `PILL_SECOND_VIEW_SECONDS` set one.
    close_at: Option<pill_core::platform::Instant>,
}

// =============================================================================
// PickTest
// =============================================================================

/// The picking test: after a fixed number of frames, ask what is under the
/// center of the main window and under the pixel just inside its top-left
/// corner. It logs both answers and outlines the entity the center hit.
#[cfg(feature = "development")]
#[derive(Default)]
struct PickTest {
    /// Frames drawn since the test started.
    frame_count: u32,
    /// The center and corner request ids, once asked.
    request_ids: Option<(u64, u64)>,
    /// The answer for the center request, once it arrives.
    center: Option<pill_player::PickHit>,
    /// The answer for the corner request, once it arrives.
    corner: Option<pill_player::PickHit>,
    /// Whether the test has stopped, after logging both answers or after
    /// giving up.
    finished: bool,
}

#[cfg(feature = "development")]
impl PickTest {
    /// Advance the test by one drawn frame of a `window_width_pixels` by
    /// `window_height_pixels` window.
    fn step(
        &mut self,
        windowed_driver: &mut posture::WindowedDriver,
        window_width_pixels: u32,
        window_height_pixels: u32,
    ) {
        if self.finished {
            return;
        }
        self.frame_count += 1;
        let world = windowed_driver.host_mut().engine_mut().world_mut();
        // Step 1: Once enough frames have been drawn, send the two requests:
        // the center and the pixel just inside the top-left corner, both on
        // the primary surface.
        let Some((center_id, corner_id)) = self.request_ids else {
            if self.frame_count < PICK_TEST_WAIT_FRAMES {
                return;
            }
            let Some(pick_requests) = world.get_resource_mut::<pill_player::PickRequests>() else {
                error!(target: telemetry_target::ENGINE, "[pick test] the world has no PickRequests");
                self.finished = true;
                return;
            };
            let surface = pill_player::SurfaceId::PRIMARY;
            let center =
                pick_requests.request(surface, window_width_pixels / 2, window_height_pixels / 2);
            let corner = pick_requests.request(surface, 1, 1);
            self.request_ids = Some((center, corner));
            info!(
                target: telemetry_target::ENGINE,
                "[pick test] requested center ({}, {}) and corner (1, 1)",
                window_width_pixels / 2,
                window_height_pixels / 2
            );
            return;
        };
        // Step 2: Take the answers that have arrived.
        if let Some(pick_results) = world.get_resource_mut::<pill_player::PickResults>() {
            for pick_result in pick_results.take() {
                if pick_result.request.id == center_id {
                    self.center = Some(pick_result.hit);
                } else if pick_result.request.id == corner_id {
                    self.corner = Some(pick_result.hit);
                }
            }
        }
        let (Some(center), Some(corner)) = (self.center, self.corner) else {
            return;
        };
        // Step 3: When both answers are in, log them and outline the entity
        // the center hit.
        self.finished = true;
        info!(target: telemetry_target::ENGINE, "[pick test] center: {center:?}");
        info!(target: telemetry_target::ENGINE, "[pick test] corner: {corner:?}");
        if let pill_player::PickHit::Entity(entity) = center {
            if let Some(selection_outlines) =
                world.get_resource_mut::<pill_player::SelectionOutlines>()
            {
                selection_outlines.entities = vec![entity];
                info!(target: telemetry_target::ENGINE, "[pick test] outlined {entity:?}");
            }
        }
    }
}

// =============================================================================
// WindowedApplication
// =============================================================================

/// State `winit` keeps for the lifetime of the standalone application.
///
/// It owns the configured project, the native window and the windowed driver.
/// It also saves the first failure that happens inside the event loop, so
/// [`run`] can return it after the loop exits.
struct WindowedApplication {
    /// The project this run hosts, as the compiled posture resolved it.
    project: posture::ProjectSource,
    /// The project running with the renderer attached: the development host or
    /// the shipped runtime, whichever this build compiled.
    windowed_driver: Option<posture::WindowedDriver>,
    /// The main window, once `resumed` has created it.
    window: Option<Arc<Window>>,
    /// Whether the hidden startup window has been revealed after its first frame.
    is_window_shown: bool,
    /// The first failure recorded inside the loop; returned after it exits.
    setup_error: Option<RunError>,
    /// The gamepads, polled before every frame.
    gamepads: Gamepads,
    /// The second window, when `PILL_SECOND_VIEW=1` asked for one.
    #[cfg(feature = "development")]
    second_view: Option<SecondView>,
    /// The picking test, when `PILL_PICK_TEST=1` asked for it.
    #[cfg(feature = "development")]
    pick_test: Option<PickTest>,
}

#[cfg(feature = "development")]
impl WindowedApplication {
    /// Open the second window as surface 1 when `PILL_SECOND_VIEW=1`, with a
    /// camera of its own. A failure is reported and the run goes on without
    /// the second view.
    fn open_second_view_if_requested(&mut self, event_loop: &ActiveEventLoop) {
        // Step 1: Leave unless the environment asked for the second view.
        if std::env::var(SECOND_VIEW_ENVIRONMENT_VARIABLE).as_deref() != Ok("1") {
            return;
        }
        let Some(windowed_driver) = self.windowed_driver.as_mut() else {
            return;
        };
        // Step 2: Create the window and attach it as surface 1.
        let window_attributes = Window::default_attributes()
            .with_title(format!("{} - second view", self.project.name))
            .with_inner_size(winit::dpi::LogicalSize::new(640.0, 400.0));
        let window = match event_loop.create_window(window_attributes) {
            Ok(window) => Arc::new(window),
            Err(source) => {
                report_failure("second view window creation", &source);
                return;
            }
        };
        let window_size = window.inner_size();
        if let Err(error) = windowed_driver.attach_surface(
            SECOND_VIEW_SURFACE,
            Arc::clone(&window),
            window_size.width,
            window_size.height,
            pill_player::SurfaceOptions::default(),
        ) {
            report_failure("second view surface", &error);
            return;
        }
        // Step 3: Show the same scene from higher up and further back, and
        // record when to close the window if a deadline was given.
        windowed_driver.set_surface_camera(
            SECOND_VIEW_SURFACE,
            Some(pill_player::CameraBinding::View {
                camera: pill_player::CameraComponent::default(),
                transform: pill_player::TransformComponent {
                    translation: [0.0, 3.0, 16.0],
                    ..Default::default()
                },
            }),
        );
        let close_at = std::env::var(SECOND_VIEW_SECONDS_ENVIRONMENT_VARIABLE)
            .ok()
            .and_then(|seconds_text| seconds_text.trim().parse::<f64>().ok())
            .map(|seconds| {
                pill_core::platform::Instant::now() + std::time::Duration::from_secs_f64(seconds)
            });
        info!(
            target: telemetry_target::ENGINE,
            "[second view] opened as surface {} ({}x{})",
            SECOND_VIEW_SURFACE.0,
            window_size.width,
            window_size.height
        );
        self.second_view = Some(SecondView { window, close_at });
    }

    /// Route one event of the second window. It handles close and resize; its
    /// redraws are drawn with the main window's frames.
    fn second_view_event(&mut self, event: WindowEvent) {
        let Some(windowed_driver) = self.windowed_driver.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                close_second_view(
                    windowed_driver,
                    &mut self.second_view,
                    "its window was closed",
                );
            }
            WindowEvent::Resized(size) => {
                windowed_driver.resize_surface(SECOND_VIEW_SURFACE, size.width, size.height);
            }
            _ => {}
        }
    }
}

impl WindowedApplication {
    /// Present one frame while the window is still hidden, then reveal it.
    ///
    /// The window opens holding a rendered frame, so the OS default white
    /// startup background is never visible: `winit` has no way to set the
    /// client area's background color instead. The frame is presented
    /// synchronously because a hidden window never receives redraw requests
    /// on Windows.
    fn present_first_frame_and_reveal(&mut self, event_loop: &ActiveEventLoop) {
        // Step 1: Do nothing until the window and the driver are ready.
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some(windowed_driver) = self.windowed_driver.as_mut() else {
            return;
        };

        // Step 2: Draw one frame into the hidden window's surface. Reveal the
        // window already holding it, or report the failure and leave the
        // window hidden.
        match windowed_driver.run_frame() {
            Ok(_) => {
                window.set_visible(true);
                self.is_window_shown = true;
                window.request_redraw();
                info!(target: telemetry_target::ENGINE, "First frame presented; window shown");
            }
            Err(source) => {
                report_failure("first frame render", &source);
                self.setup_error = Some(source.into());
                event_loop.exit();
            }
        }
    }

    /// Advance and present one update, then schedule the next redraw.
    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        // Step 1: Do nothing until the window and the driver have finished
        // setup.
        let (Some(window), Some(windowed_driver)) = (&self.window, &mut self.windowed_driver)
        else {
            return;
        };

        // Step 2: Queue what happened since the last frame: the gamepad
        // changes, since keyboard and mouse already arrived as window events.
        // A development build also closes the second window here once its
        // deadline has passed.
        self.gamepads
            .poll(|input| windowed_driver.push_input(input));
        #[cfg(feature = "development")]
        let should_close_second_view = self.second_view.as_ref().is_some_and(|view| {
            view.close_at
                .is_some_and(|close_at| pill_core::platform::Instant::now() >= close_at)
        });
        #[cfg(feature = "development")]
        if should_close_second_view {
            close_second_view(windowed_driver, &mut self.second_view, "its time is up");
        }

        // Step 3: Advance simulation and rendering by one frame, play the
        // rumble its systems asked for, and let the picking test read the frame
        // it just drew.
        match windowed_driver.run_frame() {
            Ok(()) => {
                self.gamepads
                    .play_rumble(windowed_driver.take_rumble_requests());
                #[cfg(feature = "development")]
                if let Some(pick_test) = &mut self.pick_test {
                    let window_size = window.inner_size();
                    pick_test.step(windowed_driver, window_size.width, window_size.height);
                }

                // Step 4: Reveal the window if it has not been shown yet, then
                // schedule the next redraw. The window title stays the project
                // name; the frame statistics are logged by the runtime itself,
                // under the project's own log settings.
                if !self.is_window_shown {
                    window.set_visible(true);
                    self.is_window_shown = true;
                }
                window.request_redraw();
            }
            Err(source) => {
                // The renderer failed on this frame. Report the failure where
                // it happens and stop the loop.
                report_failure("frame render", &source);
                self.setup_error = Some(source.into());
                event_loop.exit();
            }
        }
    }
}

impl ApplicationHandler for WindowedApplication {
    /// Create the native window and complete setup on resume.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() || self.setup_error.is_some() {
            return;
        }

        // Step 1: Set the project up before creating any window. The first
        // development launch compiles the game, which takes tens of seconds,
        // and a window created before that would show a blank surface for the
        // whole time. `winit` only allows a window while the event loop is
        // active, so setup cannot run before `resumed`.
        let windowed_driver = match posture::setup_driver(self.project.clone()) {
            Ok(windowed_driver) => windowed_driver,
            Err(error) => {
                report_failure("host setup", &error);
                self.setup_error = Some(error.into());
                event_loop.exit();
                return;
            }
        };

        // Step 2: Create the native window, hidden. `winit` has no way to set
        // the client area's background color, so the window is shown only
        // after its first frame and never shows the OS default white surface.
        let window_attributes = Window::default_attributes()
            .with_title(self.project.name.to_owned())
            .with_inner_size(winit::dpi::LogicalSize::new(800.0, 600.0))
            .with_visible(false);
        let window = match event_loop.create_window(window_attributes) {
            Ok(window) => Arc::new(window),
            Err(source) => {
                report_failure("window creation", &source);
                self.setup_error = Some(FrontendError::WindowCreation { source }.into());
                event_loop.exit();
                return;
            }
        };
        let window_size = window.inner_size();

        // Step 3: Attach the renderer to the window. The project is already
        // set up, so the surface opens on a live world instead of a blank one.
        match posture::attach_renderer(
            windowed_driver,
            Arc::clone(&window),
            window_size.width,
            window_size.height,
        ) {
            Ok(windowed_driver) => {
                // Store the driver and window, render one frame while the
                // window is still hidden, and then reveal it.
                self.windowed_driver = Some(windowed_driver);
                self.window = Some(window);
                self.present_first_frame_and_reveal(event_loop);
                #[cfg(feature = "development")]
                self.open_second_view_if_requested(event_loop);
                #[cfg(feature = "development")]
                if std::env::var(PICK_TEST_ENVIRONMENT_VARIABLE).as_deref() == Ok("1") {
                    self.pick_test = Some(PickTest::default());
                }
            }
            Err(error) => {
                report_failure("renderer attachment", &error);
                self.setup_error = Some(error.into());
                event_loop.exit();
            }
        }
    }

    /// Route lifecycle, drawing and input events to the windowed driver.
    ///
    /// An event of the second window goes to `second_view_event` instead.
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        #[cfg(feature = "development")]
        if self
            .second_view
            .as_ref()
            .is_some_and(|view| view.window.id() == window_id)
        {
            self.second_view_event(event);
            return;
        }
        #[cfg(not(feature = "development"))]
        let _ = window_id;
        for input in translate_window_event(&event) {
            if let Some(windowed_driver) = &mut self.windowed_driver {
                windowed_driver.push_input(input);
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                // The loop also ends when a frame fails, so this close gets a
                // log line of its own. That line is what tells a clean shutdown
                // apart from a loop that stopped because a frame could not be
                // drawn.
                info!(target: telemetry_target::ENGINE, "Close requested; leaving the event loop");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(windowed_driver) = &mut self.windowed_driver {
                    WindowedFrameDriver::resize_surface(
                        windowed_driver,
                        pill_player::SurfaceId::PRIMARY,
                        size.width,
                        size.height,
                    );
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    /// Forward raw mouse motion. winit reports it per device rather than per
    /// window, and only while the window has focus.
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let (Some(input), Some(windowed_driver)) =
            (translate_device_event(&event), &mut self.windowed_driver)
        {
            windowed_driver.push_input(input);
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Detach surface 1 and drop its window. The main window keeps drawing.
#[cfg(feature = "development")]
fn close_second_view(
    windowed_driver: &mut posture::WindowedDriver,
    second_view: &mut Option<SecondView>,
    close_reason: &str,
) {
    if second_view.is_none() {
        return;
    }
    // The surface first, then the window it draws on.
    windowed_driver.detach_surface(SECOND_VIEW_SURFACE);
    windowed_driver.set_surface_camera(SECOND_VIEW_SURFACE, None);
    *second_view = None;
    info!(
        target: telemetry_target::ENGINE,
        "[second view] closed: {close_reason}"
    );
}

/// Drop the frontend's state and log a line before and after it.
///
/// The drop unmaps every module copy (in development) and drops the engine.
/// This is the one step where the process can die without saying anything
/// itself: a call into an already-unmapped image crashes natively. The two log
/// lines bracket that step, so a crash inside the drop shows the first line
/// and never the second.
fn teardown_frontend_state<T>(frontend_state: T) {
    info!(target: telemetry_target::ENGINE, "Shutting down");
    drop(frontend_state);
    info!(target: telemetry_target::ENGINE, "Shutdown complete");
}

/// Run the project in a native window.
///
/// # Errors
///
/// Returns [`RunError`] if the event loop cannot be created or run, or if
/// window creation, setup or the renderer fails inside the event loop.
pub fn run(project: posture::ProjectSource) -> Result<(), RunError> {
    // Step 1: Create the event loop, polling continuously so frames run as
    // fast as possible instead of waiting for user input.
    let event_loop =
        EventLoop::new().map_err(|source| FrontendError::EventLoopCreation { source })?;
    event_loop.set_control_flow(ControlFlow::Poll);

    // Step 2: Build the application state around the project.
    let mut application = WindowedApplication {
        project,
        window: None,
        windowed_driver: None,
        is_window_shown: false,
        setup_error: None,
        gamepads: Gamepads::new(),
        #[cfg(feature = "development")]
        second_view: None,
        #[cfg(feature = "development")]
        pick_test: None,
    };

    // Step 3: Run the event loop until the window is closed, then tear the
    // application down with the failure it recorded still held. The failure is
    // deliberately not reported here. Every path that records one reports it
    // where it happens, before this teardown, because the drop unmaps the
    // module images; a crash inside the drop would end the process before a
    // report made here could reach the console.
    let event_loop_result = event_loop.run_app(&mut application);
    let deferred_error = application.setup_error.take();
    teardown_frontend_state(application);

    // Step 4: Return the first failure, so the event loop's own error cannot
    // hide the deferred one that explains it.
    if let Some(error) = deferred_error {
        return Err(error);
    }
    event_loop_result.map_err(|source| FrontendError::EventLoopCreation { source })?;
    Ok(())
}
