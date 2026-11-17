//! The browser editor frontend: the egui editor in a page, with the scene on
//! one canvas and the shell's UI on a transparent canvas above it.
//!
//! # Responsibilities
//!
//! - [`run`]: put the scene on one canvas and the shell's tessellated draw
//!   lists on the canvas above it, and feed the shell's plain-data boundary
//!   (`EguiShellInput`/`EguiShellFrame`) the way the native executable does.
//! - Run one update per redraw, in this order: engine input, the queued command
//!   batches and scripted lines, the boundary input and snapshot, the world
//!   update (the shell's egui pass runs inside it), then the UI draw.
//! - [`scripted::editor_command`]: the scripted channel, the web counterpart of
//!   `PILL_EDITOR_TEST_COMMANDS`.
//!
//! # Design
//!
//! The browser counterpart of `pill_editor`, paired with `pill_web` the way
//! the native editor is paired with `pill_standalone`. `pill_web` runs the
//! start-up both browser frontends share (telemetry, the asset pack, project
//! setup) and holds the canvas helpers; this crate owns the editor's canvases,
//! its UI device and the order of one update. The shell crate registers itself
//! when the bundle's module init calls `register`, and its system runs inside
//! every engine update, so this crate only owns the browser-only half.
//!
//! The scene canvas carries the project's own renderer. The UI canvas gets its
//! own `wgpu` device and `egui-wgpu` renderer, because egui's meshes are plain
//! data and the UI must not depend on a renderer that can reload or fail. The
//! UI canvas stays transparent where the dock drew nothing, so the scene shows
//! through the viewport holes; that is the browser counterpart of the native
//! composition overlay.
//!
//! Wasm only: on native targets this crate compiles to nothing, like `pill_web`.

#![cfg(target_arch = "wasm32")]

/// The scripted channel the page drives the editor through.
pub mod scripted;

// Standard library
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

// External crates
use pill_core::platform::Instant;
use pill_core::telemetry::telemetry_target;
use pill_core::{error, info, warn};
use pill_editor_core::boundary::{EguiShellFrame, EguiShellInput};
use pill_editor_core::commands::{ensure_command_resources, PendingEditorCommands};
use pill_editor_core::drain::{self, HistoryDirection};
use pill_editor_core::editor_state::{EditorCommand, EditorSelection, EditorSnapshot};
use pill_editor_core::gizmo::{GizmoMode, GizmoSettings, GizmoSpace, GizmoState};
use pill_editor_core::history::CommandHistory;
use pill_editor_core::hooks::{EditorHookRegistry, EditorHookRequests, HookNotices};
use pill_editor_core::notices::EditorNotices;
use pill_editor_core::panels::ModuleLiveness;
use pill_editor_core::scripted::{Dump, ScriptedCommand};
use pill_editor_core::{entities, systems};
use pill_engine_core::input::InputEvent;
use pill_input::winit_events::translate_window_event;
use pill_player::{
    report_failure, FrameDriver, RenderingRuntime, Runtime, StaticProject, SurfaceId,
    WindowedFrameDriver,
};
use pill_web::{canvas_element, canvas_physical_size, focus_without_scrolling, start_project};
use web_sys::HtmlCanvasElement;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys};
use winit::window::{Window, WindowId};

// Current crate
use crate::scripted::take_scripted_lines;

// =============================================================================
// Constants
// =============================================================================

/// How much of each new frame's rate goes into the displayed FPS; the rest
/// keeps the running average. The native runner smooths the same way.
const FPS_SMOOTHING: f32 = 0.1;

/// Frames between console heartbeats, for the person reading the log.
const FRAME_HEARTBEAT_FRAMES: u64 = 60;

// =============================================================================
// Types
// =============================================================================

/// The UI canvas's own device, surface and egui renderer.
struct EditorUiSurface {
    /// The surface the UI frame is presented on.
    surface: wgpu::Surface<'static>,
    /// The surface's configuration, kept so a resize can update it.
    configuration: wgpu::SurfaceConfiguration,
    /// Draws egui's meshes through the device and queue below.
    renderer: egui_wgpu::Renderer,
    /// The device the surface and renderer were created on.
    device: wgpu::Device,
    /// The queue of that device.
    queue: wgpu::Queue,
}

impl EditorUiSurface {
    /// Resize the surface to the UI window's physical extent and reconfigure
    /// it.
    fn resize(&mut self, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        if self.configuration.width == width && self.configuration.height == height {
            return;
        }
        self.configuration.width = width;
        self.configuration.height = height;
        self.surface.configure(&self.device, &self.configuration);
    }
}

/// State `winit` keeps for the lifetime of the page.
struct WebEditorApplication {
    /// The element id of the canvas the game draws on.
    scene_canvas_id: String,
    /// The element id of the canvas the shell's UI draws on.
    ui_canvas_id: String,
    /// The started project, until the scene window exists to attach it to.
    runtime: Option<Runtime>,
    /// The scene canvas's window.
    scene_window: Option<Arc<Window>>,
    /// The UI canvas's window: input and size only, no renderer of its own.
    ui_window: Option<Arc<Window>>,
    /// The scene's rendering driver, filled in when the renderer attach
    /// resolves.
    driver: Rc<RefCell<Option<RenderingRuntime>>>,
    /// The UI canvas's device, surface and renderer, filled in when the UI
    /// attach resolves.
    ui_surface: Rc<RefCell<Option<EditorUiSurface>>>,
    /// UI input events since the last frame, in physical pixels.
    pending_events: Vec<InputEvent>,
    /// Whether the UI window has keyboard focus.
    focused: bool,
    /// The editor's monotonic clock origin, for the shell's input.
    started_at: Instant,
    /// The scene's smoothed frame rate, for the shell's input and overlays.
    frames_per_second: f32,
    /// When the previous frame ran, for the frame-rate smoothing.
    last_frame_time: Option<Instant>,
    /// Frames run since startup.
    frame_index: u64,
    /// The liveness marker of every editor API callback the linked modules
    /// registered. A browser build never unloads an image, so one marker
    /// serves the whole session.
    hook_liveness: Arc<ModuleLiveness>,
}

impl ApplicationHandler for WebEditorApplication {
    /// Create both canvas windows and start the two asynchronous attaches.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };

        // Step 1: put a window on the scene canvas. It is built like the
        // game-only runner's window but is not focusable, because the UI
        // canvas takes the input.
        let scene_canvas = match canvas_element(&self.scene_canvas_id) {
            Ok(canvas) => canvas,
            Err(message) => {
                error!(
                    target: telemetry_target::EDITOR,
                    error = message.as_str(),
                    "scene canvas not found; the editor does not start"
                );
                return;
            }
        };
        let laid_out_size = canvas_physical_size(&scene_canvas);
        let scene_attributes = Window::default_attributes()
            .with_canvas(Some(scene_canvas))
            .with_focusable(false);
        let scene_window = match event_loop.create_window(scene_attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => return report_failure("scene canvas window creation", &error),
        };
        self.scene_window = Some(Arc::clone(&scene_window));

        // Step 2: put a window on the UI canvas above it. That window receives
        // the keyboard and pointer input.
        let ui_canvas = match canvas_element(&self.ui_canvas_id) {
            Ok(canvas) => canvas,
            Err(message) => {
                error!(
                    target: telemetry_target::EDITOR,
                    error = message.as_str(),
                    "UI canvas not found; the editor does not start"
                );
                return;
            }
        };
        let focus_target = ui_canvas.clone();
        let ui_laid_out_size = canvas_physical_size(&ui_canvas);
        let ui_attributes = Window::default_attributes()
            .with_canvas(Some(ui_canvas.clone()))
            .with_focusable(true);
        let ui_window = match event_loop.create_window(ui_attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => return report_failure("UI canvas window creation", &error),
        };
        if let Err(error) = focus_without_scrolling(&focus_target) {
            warn!(
                target: telemetry_target::EDITOR,
                error = ?error,
                "could not focus the UI canvas; click it to send keys"
            );
        }
        self.ui_window = Some(Arc::clone(&ui_window));

        // Step 3: attach the game renderer to the scene canvas. WebGPU refuses
        // a zero-sized surface, so a canvas winit has not measured yet is
        // attached at its laid-out size.
        let size = scene_window.inner_size();
        let (width, height) = if size.width > 0 && size.height > 0 {
            (size.width, size.height)
        } else {
            laid_out_size
        };
        let driver = Rc::clone(&self.driver);
        pill_core::platform::futures::spawn(async move {
            let attach =
                pill_player::attach_renderer(runtime, Arc::clone(&scene_window), width, height);
            match attach.await {
                Ok(mut attached) => {
                    // A resize that arrived during the attach went nowhere, so
                    // apply the canvas's size as winit measures it now.
                    let size = scene_window.inner_size();
                    if size.width > 0
                        && size.height > 0
                        && (size.width, size.height) != (width, height)
                    {
                        attached.resize_surface(SurfaceId::PRIMARY, size.width, size.height);
                    }
                    // The browser editor starts in edit mode too: the project's
                    // game systems wait for Play, while the shell's pass and the
                    // renderer's frame build keep running.
                    attached
                        .engine_mut()
                        .set_run_mode(pill_engine_core::RunMode::Editing);
                    *driver.borrow_mut() = Some(attached);
                    info!(target: telemetry_target::EDITOR, "renderer attached to the scene canvas");
                    scene_window.request_redraw();
                }
                Err(error) => report_failure("renderer attachment", &error),
            }
        });

        // Step 4: give the UI canvas its own device and egui renderer, above
        // the scene.
        let ui_surface = Rc::clone(&self.ui_surface);
        pill_core::platform::futures::spawn(async move {
            match attach_ui_surface(ui_canvas, ui_laid_out_size).await {
                Ok(surface) => {
                    info!(target: telemetry_target::EDITOR, "UI renderer attached to the UI canvas");
                    *ui_surface.borrow_mut() = Some(surface);
                    ui_window.request_redraw();
                }
                Err(message) => {
                    error!(
                        target: telemetry_target::EDITOR,
                        error = message.as_str(),
                        "UI renderer not attached; the editor UI is not drawn"
                    );
                }
            }
        });
    }

    /// Route each window's events: the scene window's input goes to the game,
    /// and the UI window's goes to the shell.
    fn window_event(&mut self, _event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let ui_window = self.ui_window.as_ref();
        if ui_window.is_some_and(|window| window.id() == id) {
            match event {
                WindowEvent::Focused(focused) => self.focused = focused,
                WindowEvent::Resized(size) => {
                    if let Some(ui_surface) = self.ui_surface.borrow_mut().as_mut() {
                        ui_surface.resize(size.width, size.height);
                    }
                }
                WindowEvent::RedrawRequested => self.frame(),
                other => {
                    for input in translate_window_event(&other) {
                        self.pending_events.push(input);
                    }
                }
            }
            return;
        }
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
            WindowEvent::RedrawRequested => self.frame(),
            _ => {}
        }
    }

    /// Forward raw mouse motion. winit reports it per device rather than per
    /// window, so it is added to the UI's input stream here.
    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let Some(input) = pill_input::winit_events::translate_device_event(&event) {
            self.pending_events.push(input);
        }
    }
}

impl WebEditorApplication {
    /// Run one editor frame: take in the input, apply the queued commands,
    /// publish the boundary input and the snapshot, run the world frame (the
    /// shell's egui pass runs inside it), then draw the UI.
    fn frame(&mut self) {
        let Some(scene_window) = self.scene_window.clone() else {
            return;
        };
        self.frame_index += 1;

        // Step 1: measure this frame's rate and smooth it for the shell.
        let now = Instant::now();
        if let Some(last) = self.last_frame_time {
            let delta_time = now.duration_since(last).as_secs_f32();
            if delta_time > 0.0 {
                let instant_frames_per_second = 1.0 / delta_time;
                self.frames_per_second = if self.frames_per_second > 0.0 {
                    self.frames_per_second * (1.0 - FPS_SMOOTHING)
                        + instant_frames_per_second * FPS_SMOOTHING
                } else {
                    instant_frames_per_second
                };
            }
        }
        self.last_frame_time = Some(now);

        // Step 2: pass the UI window's events on as engine input. The shell's
        // egui pass reads the same events.
        let events: Vec<InputEvent> = std::mem::take(&mut self.pending_events);
        let scripted_lines = take_scripted_lines();
        let ui_surface = Rc::clone(&self.ui_surface);
        let mut driver_slot = self.driver.borrow_mut();
        let Some(driver) = driver_slot.as_mut() else {
            return;
        };
        for event in &events {
            driver.push_input(event.clone());
        }

        // Step 3: create the resources the shell expects to find (the gizmo
        // state and the command registry, inbox and pending batches) before
        // its first pass reads them.
        {
            let world = driver.engine_mut().world_mut();
            ensure_command_resources(world);
            world.register_resource::<GizmoState>();
            if world.get_resource::<GizmoState>().is_none() {
                world.insert_resource(GizmoState::default());
            }
        }

        // Step 4: bind the callbacks the linked modules registered, then apply
        // the batches the shell queued in the previous frame and the lines the
        // page scripted. The marker is permanent: a static build never unloads
        // an image, so there is nothing to invalidate.
        sync_hooks(driver.engine_mut().world_mut(), &self.hook_liveness);
        apply_pending_commands(
            driver.engine_mut(),
            now.duration_since(self.started_at),
            &scripted_lines,
        );
        let scale_factor = self
            .ui_window
            .as_ref()
            .map_or(1.0, |window| window.scale_factor() as f32)
            .max(f32::EPSILON);
        let logical_size =
            self.ui_window
                .as_ref()
                .map_or(pill_core::math::Vector2f::new(1.0, 1.0), |window| {
                    let size = window.inner_size();
                    pill_core::math::Vector2f::new(size.width as f32, size.height as f32)
                        / scale_factor
                });
        {
            let world = driver.engine_mut().world_mut();
            if let Some(input) = world.get_resource_mut::<EguiShellInput>() {
                // The pointer position is this frame's newest move, or the one
                // the previous frame left: a release can arrive with no move
                // in front of it and still needs a position.
                let carried_cursor = input.cursor;
                let cursor = events
                    .iter()
                    .rev()
                    .find_map(|event| match event {
                        InputEvent::CursorMoved { position } => Some(*position),
                        _ => None,
                    })
                    .or(carried_cursor);
                *input = EguiShellInput {
                    screen_size: logical_size,
                    scale_factor,
                    focused: self.focused,
                    time: now.duration_since(self.started_at).as_secs_f64(),
                    frames_per_second: self.frames_per_second,
                    // The browser's window is the whole editor, so every event
                    // belongs to the shell as well as to the game.
                    events: events.clone(),
                    cursor,
                    // This posture never reads or writes the OS clipboard, so
                    // paste text stays empty; the clipboard verbs only exist
                    // in the native editor (see the dispatch below).
                    paste_text: None,
                    // Only the desktop runner fills in gizmo badges. The
                    // browser does not project entities into a scene view, so
                    // the list stays empty.
                    badges: Vec::new(),
                    // The marquee is the desktop runner's too, for the same
                    // reason: the browser editor does not run a scene
                    // viewport's own pointer gestures yet.
                    marquee: None,
                    // The page drives the editor through
                    // `window.pillEditor.command`, so the shell's row
                    // rectangles are logged for a suite to read.
                    scripted: true,
                };
            }
        }
        publish_snapshot(driver.engine_mut(), self.frame_index);

        // Step 5: run the world frame. The shell's egui pass runs inside it and
        // leaves `EguiShellFrame` for Step 6.
        match driver.run_frame() {
            Ok(()) => {
                self.frames_per_second = driver.frames_per_second() as f32;
            }
            Err(error) => {
                report_failure("frame render", &error);
                return;
            }
        }

        // Step 6: draw the shell's frame into the UI canvas.
        draw_ui(&ui_surface, driver.engine_mut());
        if self.frame_index.is_multiple_of(FRAME_HEARTBEAT_FRAMES) {
            let (entities, systems) = driver
                .engine()
                .world()
                .get_resource::<EditorSnapshot>()
                .map_or((0, 0), |snapshot| {
                    (snapshot.entities.len(), snapshot.systems.len())
                });
            info!(
                target: telemetry_target::EDITOR,
                frames = self.frame_index,
                entities,
                systems,
                "[editor] web frame"
            );
        }
        scene_window.request_redraw();
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Run `project` as the egui editor in the page: the scene on the canvas
/// named `scene_canvas_id`, the shell's UI on the canvas named
/// `ui_canvas_id`.
///
/// Returns once the event loop is handed to the browser; the editor then runs
/// on the scene canvas's animation frames. Every failure is logged to the
/// browser console.
pub fn run(project: StaticProject, scene_canvas_id: &str, ui_canvas_id: &str) {
    let scene_canvas_id = scene_canvas_id.to_owned();
    let ui_canvas_id = ui_canvas_id.to_owned();
    // The asset pack, the telemetry and the project registration belong to
    // `pill_web`'s shared start-up; only the event loop differs.
    start_project(project, move |runtime| {
        // Step 1: hand the event loop to the browser. The canvases and their
        // renderers are created in `resumed`.
        let event_loop = match EventLoop::new() {
            Ok(event_loop) => event_loop,
            Err(error) => return report_failure("event loop creation", &error),
        };
        info!(
            target: telemetry_target::EDITOR,
            scene = %scene_canvas_id,
            ui = %ui_canvas_id,
            "starting the editor in the browser"
        );
        event_loop.spawn_app(WebEditorApplication {
            scene_canvas_id: scene_canvas_id.clone(),
            ui_canvas_id: ui_canvas_id.clone(),
            runtime: Some(runtime),
            scene_window: None,
            ui_window: None,
            driver: Rc::new(RefCell::new(None)),
            ui_surface: Rc::new(RefCell::new(None)),
            pending_events: Vec::new(),
            focused: true,
            started_at: Instant::now(),
            frames_per_second: 0.0,
            last_frame_time: None,
            frame_index: 0,
            hook_liveness: ModuleLiveness::new(),
        });
    });
}

/// Apply the batches the shell queued in the previous frame and the lines the
/// page scripted.
///
/// Scripted lines go through the shared grammar
/// ([`pill_editor_core::scripted`]) and then the shared core drain, so a line
/// this posture supports parses, applies and is recorded exactly as it is in
/// the desktop editor: the history step, the remap of undo entries to an
/// entity's new handle, and the selection following a move. The drain's
/// `files_changed` flag needs nothing here, because the browser has no asset
/// panel to refresh.
fn apply_pending_commands(
    engine: &mut pill_engine_core::Engine,
    elapsed_since_start: std::time::Duration,
    scripted_lines: &[String],
) {
    // Step 1: run the scripted lines the page appended.
    run_scripted_lines(engine, scripted_lines);
    // Step 2: apply everything the shell queued, recording history the way
    // the desktop editor does.
    let now_millis = elapsed_since_start.as_millis() as u64;
    let _ = drain::apply_pending(engine, now_millis);
    // Step 3: hand the notices the callbacks posted to the editor's own list.
    // The browser has no executable republishing that resource, so it is
    // written directly.
    publish_hook_notices(engine);
}

/// Binds every callback request the linked modules queued.
///
/// A browser build links its modules into one image, so one permanent marker
/// is the whole liveness story; the registry still filters by it, exactly as
/// the desktop editor filters by each module's own marker.
fn sync_hooks(world: &mut pill_engine_core::World, liveness: &Arc<ModuleLiveness>) {
    let requests = world
        .get_resource_mut::<EditorHookRequests>()
        .map(|requests| requests.drain())
        .unwrap_or_default();
    if requests.is_empty() {
        return;
    }
    let Some(registry) = world.get_resource_mut::<EditorHookRegistry>() else {
        return;
    };
    for request in requests {
        registry.register(
            request.source(),
            request.event(),
            request.callback(),
            liveness,
        );
    }
}

/// Appends the notices the editor API's callbacks posted to the editor's list.
fn publish_hook_notices(engine: &mut pill_engine_core::Engine) {
    let notices = engine
        .world_mut()
        .get_resource_mut::<HookNotices>()
        .map(|notices| notices.drain())
        .unwrap_or_default();
    if notices.is_empty() {
        return;
    }
    let world = engine.world_mut();
    if world.get_resource::<EditorNotices>().is_none() {
        world.insert_resource(EditorNotices::default());
    }
    if let Some(editor_notices) = world.get_resource_mut::<EditorNotices>() {
        for notice in notices {
            editor_notices.push_event(notice.level, notice.text);
        }
    }
}

/// Run every scripted line the page has appended through the shared grammar.
///
/// A line the grammar refuses is logged with the same `[editor] scripted
/// command rejected` message the desktop editor's channel uses, so a suite that
/// drives the page sees a refusal at once instead of waiting for an answer.
fn run_scripted_lines(engine: &mut pill_engine_core::Engine, lines: &[String]) {
    for line in lines {
        match ScriptedCommand::parse(line) {
            Ok(command) => run_scripted_command(engine, line, command),
            Err(error) => warn!(
                target: telemetry_target::EDITOR,
                command = line.as_str(),
                reason = error.reason.as_str(),
                "[editor] scripted command rejected"
            ),
        }
    }
}

/// Run one parsed command that this posture supports.
///
/// Verbs whose machinery this posture does not have - window placement, the
/// asset panel, picking - are refused by name. A vocabulary that silently
/// ignores a line is worse than one that says which verb is missing.
fn run_scripted_command(
    engine: &mut pill_engine_core::Engine,
    line: &str,
    command: ScriptedCommand,
) {
    match command {
        ScriptedCommand::Undo => {
            drain::step_history(engine, HistoryDirection::Undo);
        }
        ScriptedCommand::Redo => {
            drain::step_history(engine, HistoryDirection::Redo);
        }
        ScriptedCommand::PauseToggle => {
            let paused = engine.systems_paused();
            if let Some(pending_commands) = engine
                .world_mut()
                .get_resource_mut::<PendingEditorCommands>()
            {
                pending_commands.push(vec![systems::pause_toggle_command(paused)]);
            }
            info!(
                target: telemetry_target::EDITOR,
                paused = !paused,
                "[editor] pause toggled"
            );
        }
        ScriptedCommand::Step => {
            if let Some(pending_commands) = engine
                .world_mut()
                .get_resource_mut::<PendingEditorCommands>()
            {
                pending_commands.push(vec![EditorCommand::StepOnce]);
            }
            info!(target: telemetry_target::EDITOR, "[editor] step queued");
        }
        ScriptedCommand::Play => queue_play_command(engine, EditorCommand::EnterPlay),
        ScriptedCommand::Stop => queue_play_command(engine, EditorCommand::ExitPlay),
        ScriptedCommand::PlayToggle => {
            let command = match engine.run_mode() {
                pill_engine_core::RunMode::Editing => EditorCommand::EnterPlay,
                pill_engine_core::RunMode::Playing => EditorCommand::ExitPlay,
            };
            queue_play_command(engine, command);
        }
        ScriptedCommand::EntityCreate => {
            let (command, label) = entities::entity_create();
            if let Some(pending_commands) = engine
                .world_mut()
                .get_resource_mut::<PendingEditorCommands>()
            {
                pending_commands.push_recorded(vec![command], label, None);
            }
            info!(target: telemetry_target::EDITOR, "[editor] new entity queued");
        }
        ScriptedCommand::EntityDelete => {
            let (entity, asset) = engine
                .world()
                .get_resource::<EditorSelection>()
                .map(|selection| (selection.primary(), selection.asset.clone()))
                .unwrap_or((None, None));
            let Some(entity) = entity else {
                info!(
                    target: telemetry_target::EDITOR,
                    "[editor] delete entity skipped nothing selected"
                );
                return;
            };
            let (commands, label) = entities::entity_delete(entity, asset);
            if let Some(pending_commands) = engine
                .world_mut()
                .get_resource_mut::<PendingEditorCommands>()
            {
                pending_commands.push_recorded(commands, label, None);
            }
            info!(
                target: telemetry_target::EDITOR,
                entity = %entity,
                "[editor] delete entity queued"
            );
        }
        ScriptedCommand::GizmoTool(mode) => {
            queue_gizmo_tool(engine, Some(mode), None, None);
        }
        ScriptedCommand::SnapToggle => {
            let snap = gizmo_settings(engine).snap;
            queue_gizmo_tool(engine, None, None, Some(!snap));
        }
        ScriptedCommand::SpaceToggle => {
            let space = match gizmo_settings(engine).space {
                GizmoSpace::World => GizmoSpace::Local,
                GizmoSpace::Local => GizmoSpace::World,
            };
            queue_gizmo_tool(engine, None, Some(space), None);
        }
        // The run mode, the pause flag and the world's restore epoch. The
        // browser has no hot reload, so nothing there moves the epoch; the
        // line still reports it, so a page and the desktop editor are read the
        // same way.
        ScriptedCommand::Dump(Dump::Play) => {
            let mode = match engine.run_mode() {
                pill_engine_core::RunMode::Editing => "editing",
                pill_engine_core::RunMode::Playing => "playing",
            };
            let (undo, redo) = engine
                .world()
                .get_resource::<CommandHistory>()
                .map(|history| (history.undo_depth(), history.redo_depth()))
                .unwrap_or((0, 0));
            info!(
                target: telemetry_target::EDITOR,
                mode,
                paused = engine.systems_paused(),
                restore_epoch = engine.world().restore_epoch(),
                undo,
                redo,
                "[test] play"
            );
        }
        // The page's data channel: the entity and system counts that the
        // shell's tabs list.
        ScriptedCommand::Dump(Dump::Tabs) => {
            let (entities, systems) = engine
                .world()
                .get_resource::<EditorSnapshot>()
                .map_or((0, 0), |snapshot| {
                    (snapshot.entities.len(), snapshot.systems.len())
                });
            info!(
                target: telemetry_target::EDITOR,
                entities,
                systems,
                "[editor] web rows"
            );
        }
        other => refuse_in_browser(line, &other),
    }
}

/// Queues one play command for the frame boundary's drain, unrecorded.
///
/// The same command the desktop editor's toolbar, its View menu row and its
/// scripted channel queue, so a session is never an undo step in either
/// posture.
fn queue_play_command(engine: &mut pill_engine_core::Engine, command: EditorCommand) {
    if let Some(pending_commands) = engine
        .world_mut()
        .get_resource_mut::<PendingEditorCommands>()
    {
        pending_commands.push(vec![command]);
    }
    info!(target: telemetry_target::EDITOR, "[editor] play control queued");
}

/// Refuse a command this posture cannot run, naming the line and its verb so
/// the gap is visible in the page's console.
fn refuse_in_browser(line: &str, command: &ScriptedCommand) {
    let reason = format!(
        "'{}' is not available in the browser editor yet",
        scripted_name(command)
    );
    warn!(
        target: telemetry_target::EDITOR,
        command = line,
        reason = reason.as_str(),
        "[editor] scripted command rejected"
    );
}

/// The name one parsed command is written with, for a refusal line.
fn scripted_name(command: &ScriptedCommand) -> &'static str {
    match command {
        ScriptedCommand::Quit => "quit",
        ScriptedCommand::Palette(_) => "palette",
        ScriptedCommand::ConsoleFilter(_) => "console filter",
        ScriptedCommand::ConsoleSearch(_) => "console search",
        ScriptedCommand::ProblemOpen(_) => "problem open",
        ScriptedCommand::SafeModeRetry => "safe_mode retry",
        ScriptedCommand::PanelUnload(_) => "panel unload",
        ScriptedCommand::PanelRestore(_) => "panel restore",
        ScriptedCommand::PanelWindow { .. } => "panel window",
        ScriptedCommand::PanelField { .. } => "panel field",
        ScriptedCommand::Undock(_) => "undock",
        ScriptedCommand::ActivateTab(_) => "tab",
        ScriptedCommand::Click { .. } => "click",
        ScriptedCommand::RightClick { .. } => "rclick",
        ScriptedCommand::Drag { .. } => "drag",
        ScriptedCommand::GizmoDrag { .. } => "gizmo drag",
        ScriptedCommand::GizmoPress { .. } => "gizmo press",
        ScriptedCommand::GizmoRelease => "gizmo release",
        ScriptedCommand::GizmoTool(_) => "gizmo",
        // The page drives the gizmo from its own rows and loads no tool
        // layer, so a named tool has nothing to activate here.
        ScriptedCommand::Tool(_) => "tool",
        ScriptedCommand::Extensions { .. } => "extensions",
        ScriptedCommand::Key(_) => "key",
        ScriptedCommand::Dump(_) => "dump",
        ScriptedCommand::Browser(_) => "browser",
        ScriptedCommand::Camera { .. } => "camera",
        ScriptedCommand::CameraZoom(_) => "camera zoom",
        ScriptedCommand::Hover { .. } => "hover",
        ScriptedCommand::HoverUi { .. } => "hover ui",
        ScriptedCommand::SelectAsset(_) => "select asset",
        ScriptedCommand::SelectEntity(_) => "select entity",
        ScriptedCommand::SelectAdd(_) => "select add",
        ScriptedCommand::SelectRange(_) => "select range",
        ScriptedCommand::SelectResource(_) => "select resource",
        // Element ids are the tool layer's own, and the page has no tool layer
        // to hand them out.
        ScriptedCommand::SelectElement { .. } => "select element",
        ScriptedCommand::EditResource { .. } => "edit resource",
        ScriptedCommand::EditSelection { .. } => "edit selection",
        ScriptedCommand::Toolbar(_) => "toolbar",
        ScriptedCommand::SceneNew => "scene new",
        ScriptedCommand::SceneOpen(_) => "scene open",
        ScriptedCommand::SceneSave(_) => "scene save",
        ScriptedCommand::DialogSetName(_) => "dialog set name",
        ScriptedCommand::DialogSubmit => "dialog submit",
        ScriptedCommand::EditAsset(_) => "edit asset",
        ScriptedCommand::PickEntity { .. } => "pick entity",
        ScriptedCommand::EditSet { .. } => "edit set",
        ScriptedCommand::EditArray { .. } => "edit array",
        ScriptedCommand::EditAssetSettings { .. } => "edit asset-settings",
        ScriptedCommand::EditSystem { .. } => "edit system",
        ScriptedCommand::EditPreference { .. } => "edit preference",
        ScriptedCommand::EditComponent { .. } => "edit add/remove",
        ScriptedCommand::Undo => "command editor.undo",
        ScriptedCommand::Redo => "command editor.redo",
        ScriptedCommand::PauseToggle => "command editor.pause_toggle",
        ScriptedCommand::Play => "command editor.play",
        ScriptedCommand::Stop => "command editor.stop",
        ScriptedCommand::PlayToggle => "command editor.play_toggle",
        ScriptedCommand::Step => "command editor.step",
        ScriptedCommand::EntityCreate => "command editor.entity_create",
        ScriptedCommand::EntityDelete => "command editor.entity_delete",
        ScriptedCommand::SnapToggle => "command editor.tool_snap_toggle",
        ScriptedCommand::SpaceToggle => "command editor.tool_space_toggle",
        // The element verbs run in the desktop editor only: a page draws no
        // element handles, so there is nothing to nudge and no tool menu to
        // open here yet.
        ScriptedCommand::DragElement { .. } => "drag element",
        ScriptedCommand::ElementMenu { .. } => "element menu",
        // The native-only verbs: a page has no OS clipboard, no OLE drag, a
        // single fixed scene view and no second OS window to move a tab into,
        // so these parse but cannot run here.
        ScriptedCommand::Copy => "command editor.copy",
        ScriptedCommand::Cut => "command editor.cut",
        ScriptedCommand::Paste => "command editor.paste",
        ScriptedCommand::NewSceneView => "command editor.scene_new_view",
        ScriptedCommand::Popout { .. } => "popout",
        ScriptedCommand::Drop(_) => "drop",
        ScriptedCommand::HoverFile(_) => "hover file",
        ScriptedCommand::DragOut(_) => "drag out",
        // The browser has no file manager to reveal a file in and no asset
        // picker for a settings row, so neither verb has anything to open.
        ScriptedCommand::Reveal(_) => "reveal",
        ScriptedCommand::PickReference { .. } => "pick reference",
    }
}

/// The gizmo settings held by the world, or the defaults when no state
/// resource was registered.
fn gizmo_settings(engine: &pill_engine_core::Engine) -> GizmoSettings {
    engine
        .world()
        .get_resource::<GizmoState>()
        .map(|state| state.settings)
        .unwrap_or_default()
}

/// Queue one `SetGizmoTool` edit: the same edit the desktop editor's menus,
/// shortcuts and scripted channel queue.
fn queue_gizmo_tool(
    engine: &mut pill_engine_core::Engine,
    mode: Option<GizmoMode>,
    space: Option<GizmoSpace>,
    snap: Option<bool>,
) {
    if let Some(pending_commands) = engine
        .world_mut()
        .get_resource_mut::<PendingEditorCommands>()
    {
        pending_commands.push(vec![EditorCommand::SetGizmoTool { mode, space, snap }]);
    }
}

/// Capture the snapshot the shell's tabs read.
///
/// The entity, system and component tiers come from the world. The asset rows,
/// the standalone types and the preferences document keep their defaults.
fn publish_snapshot(engine: &mut pill_engine_core::Engine, revision: u64) {
    // Step 1: read the entity selection the detail rows come from.
    let selected_entity = engine
        .world()
        .get_resource::<EditorSelection>()
        .and_then(|selection| selection.primary());
    let selected_entities: Vec<pill_engine_core::Entity> = engine
        .world()
        .get_resource::<EditorSelection>()
        .map(|selection| selection.entities().to_vec())
        .unwrap_or_default();
    // Step 2: capture the engine-owned tiers and the selected entities' rows.
    let mut snapshot = EditorSnapshot::capture_engine_tiers(engine, revision, &[], Vec::new());
    snapshot.detail =
        selected_entity.and_then(|entity| EditorSnapshot::capture_detail(engine, entity));
    snapshot.selected_details = selected_entities
        .iter()
        .take(pill_editor_core::editor_state::SELECTED_DETAILS_MAX)
        .filter_map(|entity| EditorSnapshot::capture_detail(engine, *entity))
        .collect();
    // Step 3: publish the snapshot for the shell.
    if let Some(resource) = engine.world_mut().get_resource_mut::<EditorSnapshot>() {
        *resource = snapshot;
    }
}

/// Draw the shell's frame into the UI canvas: apply its texture uploads, then
/// draw the meshes over a fully transparent clear.
///
/// The frame is a world resource, so its texture delta is cleared here on every
/// path. epaint debug-asserts that a dropped delta is empty, so leaving an
/// unapplied upload behind would panic a debug build.
fn draw_ui(
    ui_surface: &Rc<RefCell<Option<EditorUiSurface>>>,
    engine: &mut pill_engine_core::Engine,
) {
    let Some(shell_frame) = engine.world_mut().get_resource_mut::<EguiShellFrame>() else {
        return;
    };
    draw_ui_frame(ui_surface, shell_frame);
    shell_frame.textures.clear();
}

/// The drawing half of [`draw_ui`]. `draw_ui` owns the delta's lifetime.
fn draw_ui_frame(ui_surface: &Rc<RefCell<Option<EditorUiSurface>>>, shell_frame: &EguiShellFrame) {
    let mut ui_surface = ui_surface.borrow_mut();
    let Some(editor_ui_surface) = ui_surface.as_mut() else {
        return;
    };
    // A suboptimal frame can still be drawn. Any other result is reported, and
    // the next frame tries again.
    let texture = match editor_ui_surface.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(texture)
        | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
        other => {
            warn!(target: telemetry_target::EDITOR, error = ?other, "UI canvas surface unavailable");
            return;
        }
    };
    let view = texture
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let screen_descriptor = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [
            editor_ui_surface.configuration.width,
            editor_ui_surface.configuration.height,
        ],
        pixels_per_point: shell_frame.pixels_per_point.max(0.01),
    };
    // Step 1: apply the texture updates before anything samples them. One
    // texture can carry several deltas in one frame.
    for (texture_id, deltas) in &shell_frame.textures.set {
        for image_delta in deltas {
            editor_ui_surface.renderer.update_texture(
                &editor_ui_surface.device,
                &editor_ui_surface.queue,
                *texture_id,
                image_delta,
            );
        }
    }
    // Step 2: build the draw commands. egui's own vertex and index buffers go
    // into the same submission as the pass.
    let mut encoder =
        editor_ui_surface
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("pill editor egui"),
            });
    let user_buffers = editor_ui_surface.renderer.update_buffers(
        &editor_ui_surface.device,
        &editor_ui_surface.queue,
        &mut encoder,
        &shell_frame.meshes,
        &screen_descriptor,
    );
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pill editor egui pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Clear to transparent where the dock drew nothing, so the
                    // scene canvas below shows through the viewport holes.
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // `forget_lifetime` extends the pass's borrows to `'static`, which
        // egui's render call requires. The pass stays inside this block, so
        // nothing outlives `view`.
        let mut pass = pass.forget_lifetime();
        editor_ui_surface
            .renderer
            .render(&mut pass, &shell_frame.meshes, &screen_descriptor);
    }
    // Step 3: release the textures the shell asked to free, then submit.
    for texture_id in &shell_frame.textures.free {
        editor_ui_surface.renderer.free_texture(texture_id);
    }
    editor_ui_surface
        .queue
        .submit(user_buffers.into_iter().chain(Some(encoder.finish())));
    editor_ui_surface.queue.present(texture);
}

/// Build the UI canvas's device, surface and egui renderer.
///
/// # Errors
///
/// Returns a message naming the step that failed: the canvas refused a
/// surface, no adapter accepted it, the device request was refused, or the
/// adapter offered no surface configuration.
async fn attach_ui_surface(
    canvas: HtmlCanvasElement,
    laid_out_size: (u32, u32),
) -> Result<EditorUiSurface, String> {
    // Step 1: create the WebGPU instance and a surface on the UI canvas.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::PRIMARY,
        flags: wgpu::InstanceFlags::from_build_config(),
        backend_options: wgpu::BackendOptions::default(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|error| format!("UI canvas surface refused: {error}"))?;
    // Step 2: pick an adapter and request its device and queue.
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        })
        .await
        .map_err(|error| format!("no adapter for the UI canvas: {error}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("pill editor egui device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::default(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        })
        .await
        .map_err(|error| format!("UI canvas device refused: {error}"))?;
    // Step 3: choose the configuration, build the renderer and configure the
    // surface.
    let capabilities = surface.get_capabilities(&adapter);
    let surface_format = capabilities
        .formats
        .iter()
        .copied()
        .find(wgpu::TextureFormat::is_srgb)
        .unwrap_or(capabilities.formats[0]);
    let mut configuration = surface
        .get_default_config(&adapter, laid_out_size.0.max(1), laid_out_size.1.max(1))
        .ok_or("the UI canvas offers no surface configuration")?;
    configuration.format = surface_format;
    // egui's colours are premultiplied, and the page draws this canvas over
    // the scene one, so the canvas must composite with alpha. wgpu's web
    // backend draws that mode but lists only `Opaque` in the surface
    // capabilities (the list is hard-coded there), so the mode is set directly
    // instead of being picked from the list.
    configuration.alpha_mode = wgpu::CompositeAlphaMode::PreMultiplied;
    info!(
        target: telemetry_target::EDITOR,
        format = ?surface_format,
        alpha_mode = ?configuration.alpha_mode,
        "UI canvas configured"
    );
    surface.configure(&device, &configuration);
    let renderer = egui_wgpu::Renderer::new(
        &device,
        surface_format,
        egui_wgpu::RendererOptions::default(),
    );
    Ok(EditorUiSurface {
        surface,
        configuration,
        renderer,
        device,
        queue,
    })
}
