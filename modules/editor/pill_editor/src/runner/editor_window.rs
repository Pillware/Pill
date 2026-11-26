//! The editor window's state and the code that opens it.
//!
//! # Responsibilities
//!
//! - Hold the state one editor frame reads and writes: the main window and
//!   the editor's GPU device, the host (which owns the project, the world and
//!   the shell session), the scene views and popout windows, the cameras, the
//!   gizmo drag in flight, and the caches the frame keeps.
//! - Create all of it in [`EditorWindow::open`]: the device, the overlay, the
//!   game window, the host with its modules, and the scene views.
//!
//! # Design
//!
//! The state is one struct because the frame's phases all work on one borrow
//! of it. The methods that use it live in the sibling modules, one per
//! concern. The fields are `pub(super)` so those modules can read them; a
//! getter for each field would only wrap plain data.

// Current crate
use super::*;

// =============================================================================
// Types
// =============================================================================

/// The editor's window, its host, and everything the frame loop owns.
pub(super) struct EditorWindow {
    /// The main window: the editor's own window, and the game view's primary
    /// surface only when the game window could not be created. The shell's UI
    /// is drawn into the overlay, not here: the composition visual on Windows
    /// and the floating window on Unix, with the window surface as the
    /// fallback posture.
    pub(super) window: Arc<Window>,
    /// The editor's own GPU device, which draws the UI; the project's
    /// renderer keeps its own device inside the host.
    pub(super) device: wgpu::Device,
    /// The queue of the editor's device.
    pub(super) queue: wgpu::Queue,
    /// The editor's instance, kept for late overlay creation.
    pub(super) instance: wgpu::Instance,
    /// The adapter the editor's device runs on, kept for late overlay
    /// creation.
    pub(super) adapter: wgpu::Adapter,
    /// The host: the project, its extensions and the shell module.
    pub(super) host: RenderingHost,
    /// The scene views' child windows, in view-index order: each has its own
    /// camera and renderer surface. A slot is `None` when its window could not
    /// be created. The vector grows when the dock reports a view it does not
    /// hold yet.
    pub(super) views: Vec<Option<SceneView>>,
    /// The game viewport's window, when one could be created: it carries
    /// the primary surface, so the project's renderer presents the game
    /// view there. `None` leaves the primary on the main window.
    pub(super) game_surface: Option<SurfaceWindow>,
    /// One window per popped-out tab, keyed by the tab's wire name. Each has
    /// its own swapchain and its own egui renderer, because the UI that draws
    /// into it is a separate pass.
    pub(super) popouts: std::collections::BTreeMap<String, PopoutWindow>,
    /// The editor's cameras, one per scene viewport. Navigation moves only the
    /// active viewport's camera, and a scene ray always projects through the
    /// camera of the view it was cast in.
    pub(super) cameras: Vec<OrbitCamera>,
    /// The scene cameras as Play found them. A session may move a view - the
    /// Game tab has its own camera, but a person can still orbit a scene one -
    /// and Stop puts these back, so the edit view returns where it was.
    pub(super) play_cameras: Option<Vec<OrbitCamera>>,
    /// The run mode the last frame saw, so entering and leaving a session is
    /// noticed exactly once.
    pub(super) last_run_mode: Option<pill_engine_core::RunMode>,
    /// The viewport the last scene click activated; the gizmo and the
    /// viewport commands read it.
    pub(super) active_view: usize,
    /// Whether the left mouse button is down in each scene view. A pick
    /// answers a frame or two after the press, and a handle grab starts a
    /// drag only while the button is still held.
    pub(super) left_down: Vec<bool>,
    /// The camera navigation drag in flight, when one is.
    pub(super) camera_drag: Option<CameraDrag>,
    /// The keyboard modifiers held, as last reported by a modifiers event.
    pub(super) modifiers: Modifiers,
    /// The keymap preset the preferences name. It decides which mouse chords
    /// the viewport navigation uses.
    pub(super) keymap: KeymapPreset,
    /// Whether each view's published camera binding is stale. Navigation marks
    /// it, and the next frame publishes the surface camera again.
    pub(super) camera_dirty: Vec<bool>,
    /// egui's own renderer on the editor's device.
    pub(super) egui_renderer: egui_wgpu::Renderer,
    /// The surface the UI is presented through: the composition overlay, or
    /// the window surface when composition is refused. `None` until it is
    /// created, and left `None` when neither posture works.
    pub(super) overlay: Option<OverlaySurface>,
    /// `PILL_EDITOR_NO_OVERLAY` asked for the window-surface posture, so the
    /// composition overlay is never retried.
    pub(super) no_overlay: bool,
    /// The frame index of the last overlay creation attempt, so the next
    /// retry waits 120 frames from it.
    ///
    /// Only the composition overlay is retried, and it is Windows-only, so the
    /// bookkeeping is compiled there alone.
    #[cfg(windows)]
    pub(super) overlay_retry_frame: u64,
    /// The editor's fallback context, used while no shell module is loaded.
    pub(super) fallback_context: egui::Context,
    /// Whether the fallback UI was reported already.
    pub(super) reported_fallback: bool,
    /// The window events not yet pushed into the engine's input.
    pub(super) pending_events: Vec<InputEvent>,
    /// The editor's own windows' events, not yet handed to the shell.
    pub(super) shell_events: Vec<InputEvent>,
    /// The pointer's last position over the editor's windows, in the main
    /// window's client pixels. The shell input carries it, so a release that
    /// arrives without a move still has a position.
    pub(super) shell_cursor: Option<pill_core::math::Vector2f>,
    /// The notices the editor publishes for the shell: the toast events and
    /// the Problems rows.
    pub(super) notices: EditorNotices,
    /// The last pointer position each scene window reported, in its own
    /// client-area physical pixels. It is `None` until that window reports its
    /// first move.
    pub(super) scene_cursor: Vec<Option<(f32, f32)>>,
    /// The scene left press waiting for its pick answer: the view, the press
    /// position and the pick id. A handle answer starts a gizmo drag while
    /// the button is still held; an entity answer moves the selection.
    pub(super) press_pick: Option<PressPick>,
    /// The entities the renderer's outline set currently names, so the world
    /// resource is written once per selection change.
    pub(super) outlined_entities: Vec<Entity>,
    /// The overlay chips' rectangles the shell reported last frame, in
    /// logical points. A click inside one belongs to the UI, not the scene.
    pub(super) overlay_regions: Vec<ShellRect>,
    /// The scene view rectangles the shell reported last frame, in logical
    /// points, one slot per view. The published viewport state names the
    /// active view's entry.
    pub(super) scene_rects: Vec<Option<ShellRect>>,
    /// The gizmo drag in flight, when one is.
    pub(super) gizmo_drag: Option<GizmoDrag>,
    /// The gizmo geometry drawn this frame. The hover test runs against these
    /// exact handles, so the highlight cannot disagree with what is on screen.
    pub(super) gizmo_geometry: GizmoGeometry,
    /// The active tool's element handles drawn this frame, in the tool's own
    /// report order. The hover, the press and the marquee all read them, so
    /// nothing acts on an element that is no longer drawn.
    pub(super) element_handles: Vec<tools::ElementHandle>,
    /// The kinds the active tool described this frame, which the marquee reads
    /// for its box-select rule.
    pub(super) element_kinds: Vec<tools::ElementKind>,
    /// What this frame's element handles point at, by handle id.
    pub(super) element_map: gizmo::ElementHandleMap,
    /// The last element handles the log reported: the view and each element's
    /// screen position and selection. The lines appear on a change, not every
    /// frame.
    pub(super) element_logged: Option<ElementLogSignature>,
    /// The marquee drag in flight, when one is.
    pub(super) marquee: Option<MarqueeDrag>,
    /// The handle under the pointer, when one is.
    pub(super) hovered_handle: Option<u32>,
    /// How long the handles were when they were last built, in world units.
    /// The hover test's radius and the drag guides' dash length follow it.
    pub(super) gizmo_reference: f32,
    /// The handle hit radius as a fraction of the handle length, from the
    /// preferences file: the hover test and the press share it.
    pub(super) gizmo_hit_radius: f32,
    /// Whether the preferences file asks for rendered asset previews.
    pub(super) render_previews: bool,
    /// The preview images the renderer has not answered yet, keyed by the
    /// image id the preview request returned.
    pub(super) preview_in_flight: std::collections::HashMap<u64, String>,
    /// Whether the drag guides were drawn last frame. The log line appears
    /// when this changes, not every frame.
    pub(super) gizmo_guides: Option<bool>,
    /// The last logged gizmo anchor header (mode, space, x and y). The log
    /// line appears when it changes, and on the heartbeat.
    pub(super) gizmo_logged: Option<(GizmoMode, GizmoSpace, Option<String>, i32, i32)>,
    /// The pick ids a scripted gizmo grab requested. The first answer that
    /// names a handle starts the drag; an answer that misses every handle
    /// refuses it.
    pub(super) synthetic_press_ids: Vec<u64>,
    /// The pixel delta a scripted grab applies once its pick answers.
    pub(super) synthetic_gizmo_pending: Option<(f32, f32)>,
    /// Whether a scripted press keeps the drag open instead of finishing it.
    /// The `gizmo press`, `gizmo move` and `gizmo release` commands then drive
    /// the same drag a real mouse does, one command per frame.
    pub(super) synthetic_gizmo_hold: bool,
    /// The press pixel of the scripted drag in flight, in its view's physical
    /// pixels. Every scripted move measures its delta from here.
    pub(super) synthetic_gizmo_press: Option<(f32, f32)>,
    /// The view a scripted grab belongs to.
    pub(super) synthetic_gizmo_view: Option<usize>,
    /// The asset browser's rows. They refresh on a cadence and at once after
    /// any file command, and the Assets tab draws them.
    pub(super) asset_entries: Vec<AssetListing>,
    /// The rows the last walk found, before the module roots' synthetic
    /// folders are added. The next walk is compared against this to tell
    /// whether the tree moved.
    pub(super) asset_walk: Vec<AssetListing>,
    /// The live standalone asset types the create dialog offers.
    pub(super) standalone_types: Vec<StandaloneTypeInfo>,
    /// Whether a file command changed the tree. The next frame then refreshes
    /// the listing at once instead of waiting for the cadence.
    pub(super) asset_listing_dirty: bool,
    /// The file an OS drag is carrying over the editor, if one is. It is the
    /// subject of the drop preview.
    pub(super) drop_preview: Option<std::path::PathBuf>,
    /// The recovery document a previous run left behind. It is applied on the
    /// first frame where the shell's session exists.
    pub(super) recovered: Option<pill_editor_core::layout::RecoveryDocument>,
    /// The state the last recovery write captured, so an unchanged editor
    /// does not rewrite the document every frame.
    pub(super) last_recovery_key: Option<String>,
    /// Whether the main window currently takes IME input.
    pub(super) ime_allowed: bool,
    /// The caret rectangle the IME candidate window follows, in logical
    /// points.
    pub(super) ime_rect: Option<(f32, f32, f32, f32)>,
    /// The selected asset's detail for the Inspector's asset section.
    pub(super) asset_detail: Option<AssetDetail>,
    /// The path `asset_detail` was captured for, so the capture follows the
    /// selection without a disk read every frame.
    pub(super) asset_detail_path: Option<String>,
    /// The preferences file's text and stamp, read again when the file moves.
    /// On a change, the live reader applies the history limits and the
    /// snapping steps.
    pub(super) preferences: StateFileWatch,
    /// The keybindings file: a change re-applies the preset and the overrides
    /// to the command registry.
    pub(super) keybindings: StateFileWatch,
    /// Whether the keybindings watcher has applied once. The first pass
    /// applies the editor's default preset even when there is no file, and
    /// until it has applied, the file is re-read every frame.
    pub(super) keybindings_applied: bool,
    /// The telemetry capture ring the Console tab reads.
    pub(super) log_buffer: pill_core::telemetry::capture::LogBuffer,
    /// The parsed preferences document the snapshot's view and the scripted
    /// dump build on.
    pub(super) preferences_document: serde_json::Map<String, serde_json::Value>,
    /// The theme the editor's UI paints with: the code default, merged with
    /// the user's file and the project's layer.
    pub(super) theme: pill_ui_theme::Theme,
    /// The generation of the theme the layers resolved to. It moves when a
    /// resolution changes the theme, and the shell installs the new theme
    /// only when it moves.
    pub(super) theme_generation: u64,
    /// The layers [`Self::theme`] was merged from, in apply order.
    pub(super) theme_layers: Vec<ThemeLayerInfo>,
    /// The font bytes the layers resolved. The snapshot carries them beside
    /// the theme, so the shell installs both together.
    pub(super) theme_fonts: pill_editor_core::kit::style::ThemeFonts,
    /// What the last theme resolution read, so a poll only merges when a
    /// layer moved.
    pub(super) theme_watch: ThemeWatch,
    /// The theme generation the fallback context paints with, so the no-shell
    /// posture follows a theme change the same way the shell does.
    pub(super) fallback_theme_generation: u64,
    /// The project directory name the per-project preferences are keyed by.
    pub(super) gizmo_project: Option<String>,
    /// The project's own name, as its settings file spells it. The window's
    /// title and the caption strip's right end both name it.
    pub(super) project_name: std::sync::Arc<str>,
    /// The gizmo settings last written to the preferences file, so an
    /// unchanged tool writes nothing.
    pub(super) gizmo_persisted: Option<GizmoSettings>,
    /// The hidden component gizmo types last written to the preferences file,
    /// so an unchanged list writes nothing. `None` means the file has not been
    /// read yet, which is how the first frame knows not to write its empty
    /// list over the file's.
    pub(super) gizmo_hidden_persisted: Option<Vec<String>>,
    /// Whether the main window currently has focus.
    pub(super) focused: bool,
    /// The screen position and size the floating overlay window was last
    /// given, in physical pixels. Only the floating posture has one: it is a
    /// window of its own, so it has to be moved with the editor by hand.
    pub(super) overlay_placement: Option<(i32, i32, u32, u32)>,
    /// The main window's client size in physical pixels.
    pub(super) size: (u32, u32),
    /// The window's scale factor: how many physical pixels one logical point
    /// covers.
    pub(super) scale_factor: f32,
    /// The editor frames run since startup.
    pub(super) frame_index: u64,
    /// When the last frame started, for the smoothed FPS.
    pub(super) last_frame_time: Option<Instant>,
    /// When the heartbeat last printed. It fires every 210 frames or every two
    /// seconds, whichever comes first, so a slow run (the benchmark's debug
    /// posture) still reports inside its window.
    pub(super) last_heartbeat: Option<Instant>,
    /// The smoothed frame rate, logged with the heartbeat.
    pub(super) fps: f32,
    /// When the run started. The test deadline is measured from here.
    pub(super) started: Instant,
    /// Whether egui claimed the pointer in the last frame. A change is logged,
    /// so a suite can see the routing decision.
    pub(super) claimed_pointer: bool,
    /// Whether egui claimed the keyboard in the last frame.
    pub(super) claimed_keyboard: bool,
    /// The cursor egui asked for in the last frame. A change is logged once
    /// rather than every frame, which is how a suite sees a splitter's widened
    /// grab band ask for its resize cursor.
    pub(super) last_cursor: egui::CursorIcon,
    /// The scripted command channel, when the environment names one.
    pub(super) test_channel: Option<TestChannel>,
    /// A scripted left-button drag in flight, when one is.
    pub(super) scripted_drag: Option<ScriptedDrag>,
    /// The scene title last reported, so the log names each state the
    /// document went through once instead of every frame.
    pub(super) scene_title: String,
    /// Quit after this many seconds, when the environment asks for it.
    pub(super) test_seconds: Option<f64>,
    /// A quit the scripted channel or the deadline asked for.
    pub(super) quit: bool,
    /// When the loop draws next.
    pub(super) cadence: Cadence,
    /// What has asked for the next frame since the last one ran.
    pub(super) demand: FrameDemand,
    /// The decision the last frame left for the loop.
    pub(super) next_wake: FrameWake,
    /// egui's own repaint request from the last shell pass.
    pub(super) repaint_after: Option<Duration>,
    /// What asked for the last frame, kept for the cadence dump.
    pub(super) last_demand: FrameDemand,
    /// The entity badges this frame's scene views draw:
    /// every visible view's entities, projected to logical points.
    pub(super) badges: Vec<ShellBadge>,
    /// Whether the last decision left the loop polling. The
    /// `[editor] frame idle` line appears once per transition.
    pub(super) polling: bool,
    /// The most expensive frame drawn while polling since the last heartbeat,
    /// in milliseconds. It is the measured cost of an idle frame.
    pub(super) idle_frame_millis_max: f32,
    /// The snapshot field whose change last asked for a frame. The log names a
    /// churning reason once rather than every frame.
    pub(super) snapshot_changed_field: Option<String>,
    /// One liveness marker per module that registered a panel or an editor
    /// API callback. The executable holds the strong handle and drops it when
    /// the module reloads, which is what makes the module's panels read
    /// "module not loaded" and keeps its callbacks from being called. The
    /// module itself only queues requests.
    pub(super) module_markers: std::collections::HashMap<String, Arc<ModuleLiveness>>,
    /// The newest request per panel id, kept so a panel can be registered
    /// again after an unload without the module running.
    pub(super) panel_requests: Vec<PanelRequest>,
}
/// What the last theme resolution read.
///
/// The executable resolves the layers: it reads the `theme` preference, the
/// user's file, the project's `editor:` entry and the project's layer (through
/// the asset store). Keeping what each read produced lets a poll skip the merge
/// when nothing moved.
#[derive(Default)]
pub(super) struct ThemeWatch {
    /// The preference's value, as last resolved.
    pub(super) selection: UserThemeSelection,
    /// The user file's metadata when it was last read.
    pub(super) user_stamp: FileStamp,
    /// The legacy name already reported, so one value logs once.
    pub(super) legacy_logged: Option<String>,
    /// `project_settings.yaml`'s metadata when it was last read.
    pub(super) settings_stamp: FileStamp,
    /// `project_settings.local.yaml`'s metadata when it was last read.
    pub(super) local_settings_stamp: FileStamp,
    /// The settings file's last read, or the message that refused it.
    pub(super) settings: Option<Result<pill_development_host::EditorSettings, String>>,
    /// The project layer's path, as last resolved.
    pub(super) project_path: Option<String>,
    /// The project layer's file metadata when it was last read.
    pub(super) project_stamp: FileStamp,
    /// The project layer's handle in the asset store.
    pub(super) project_handle: Option<pill_engine_core::Handle<ThemeOverrides>>,
    /// The handle's content version when it was last merged.
    pub(super) project_version: Option<u64>,
    /// Where each family's font comes from, as last resolved (body, display,
    /// monospace).
    pub(super) font_sources: [Option<FontSource>; 3],
    /// The resolved source files' metadata when they were last read.
    pub(super) font_stamps: [FileStamp; 3],
    /// The font bytes the sources gave, as last installed.
    pub(super) fonts: pill_editor_core::kit::style::ThemeFonts,
    /// The refusal text last logged per input (a layer, a merge result or a
    /// font file), so a poll that re-reads an unchanged broken file stays
    /// quiet and only a different text logs again.
    pub(super) refusals_logged: std::collections::HashMap<String, String>,
}
impl ThemeWatch {
    /// Record `refusal` for `slot` and report whether it is new, which is
    /// when the caller logs it: the first refusal, or one whose text changed.
    pub(super) fn is_new_refusal(&mut self, slot: &str, refusal: &str) -> bool {
        if self.refusals_logged.get(slot).map(String::as_str) == Some(refusal) {
            return false;
        }
        self.refusals_logged
            .insert(slot.to_string(), refusal.to_string());
        true
    }

    /// Forget the refusal logged for `slot`, after the input read cleanly or
    /// its file moved, so the same refusal later logs again.
    pub(super) fn clear_refusal(&mut self, slot: &str) {
        self.refusals_logged.remove(slot);
    }
}
/// Where one font family's bytes come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FontSource {
    /// A `.ttf`/`.otf` below the project's `res/editor/`, loaded through the
    /// project's asset store - the project layer's fonts.
    Project(String),
    /// A file beside the user's theme document, read from disk.
    User(std::path::PathBuf),
}
/// A scripted left-button drag in flight: where the press lands, where the
/// pointer moves to, and how far along the drag is.
///
/// A drag needs one stage per frame (press, move, release) because egui only
/// reports a drag when the pointer moves while the button is held across
/// frames. A press and a release inside the same frame count as a click.
pub(super) struct ScriptedDrag {
    /// The press position, in the main window's client pixels.
    pub(super) from: (f32, f32),
    /// The position the single move goes to.
    pub(super) to: (f32, f32),
    /// How far along the drag is.
    pub(super) stage: ScriptedDragStage,
}

/// The stage a [`ScriptedDrag`] is waiting to inject, one per frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ScriptedDragStage {
    /// The pointer has not gone down yet.
    Press,
    /// The button is down and the pointer has not moved yet.
    Move,
    /// The move has been injected and the button is still down.
    Release,
}

/// What the element handle log last reported: the scene view the handles were
/// projected in, then each element's id, its pixels in that view and whether
/// the selection holds it. The rows are logged again only when this changes.
pub(super) type ElementLogSignature = (usize, Vec<(u64, i32, i32, bool)>);

/// A marquee drag in flight: where the press in empty space landed, where the
/// pointer is now, and whether it has travelled far enough to be a drag.
///
/// A candidate that never becomes live is a click on empty space, so the
/// release still asks the scene what is under the pixel.
pub(super) struct MarqueeDrag {
    /// The scene view the drag belongs to.
    pub(super) view: usize,
    /// The press position, in that view's own pixels.
    pub(super) from: (f32, f32),
    /// The pointer's latest position, in the same pixels.
    pub(super) to: (f32, f32),
    /// Whether the pointer has passed the drag threshold.
    pub(super) live: bool,
}

impl EditorWindow {
    /// Create the window, the editor's device, the host and the views.
    ///
    /// # Errors
    ///
    /// Returns [`EditorError::WindowCreation`] when the main window cannot be
    /// created, [`EditorError::AdapterRequest`] or
    /// [`EditorError::DeviceRequest`] when the editor's GPU device cannot be
    /// set up, and [`EditorError::Host`] or [`EditorError::Renderer`] when the
    /// project, its modules and the renderer cannot start.
    pub(super) fn open(
        event_loop: &ActiveEventLoop,
        log_buffer: pill_core::telemetry::capture::LogBuffer,
    ) -> Result<EditorWindow, EditorError> {
        // Step 1: the main window. A minimum size keeps an accidental drag of
        // the window's own edge from squeezing the dock unusable.
        //
        // The window has no system frame: the shell draws the title strip in
        // the space the frame would take, and the strip's buttons and edges ask
        // for the move, the resize, the maximize and the close. The window
        // stays resizable, because the platform's own resize loop is what an
        // edge drag starts.
        let window_attributes = Window::default_attributes()
            .with_title(pill_editor_core::brand::PILL_EDITOR_WINDOW_TITLE)
            .with_inner_size(INITIAL_WINDOW_SIZE)
            .with_min_inner_size(MINIMUM_WINDOW_SIZE)
            .with_decorations(false)
            .with_resizable(true);
        let window = Arc::new(
            event_loop
                .create_window(window_attributes)
                .map_err(|source| EditorError::WindowCreation { source })?,
        );
        let size = window.inner_size();
        let size = (size.width.max(1), size.height.max(1));
        let scale_factor = window.scale_factor() as f32;
        // Step 1b: the window's own frame. The compositor rounds the corners
        // and draws the one-pixel border, in the theme's line colour; it runs
        // again whenever the theme resolves to something new.
        window_frame::apply(
            &window,
            window_frame::border_for(&pill_editor_core::style::palette()),
        );
        // Step 2: the editor's own instance, adapter and device. The UI is
        // drawn through the overlay surface - composition, or the window
        // surface when composition is refused. Composition is DirectComposition
        // over DX12, so that posture exists on Windows alone and only there
        // does the editor's instance ask for DX12; on every other platform the
        // window surface is the posture and the instance takes the platform's
        // own backends. The project's renderer keeps its own instance inside
        // the host.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            #[cfg(windows)]
            backends: wgpu::Backends::DX12,
            #[cfg(not(windows))]
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .map_err(|source| EditorError::AdapterRequest { source })?;
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("pill editor (egui)"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .map_err(|source| EditorError::DeviceRequest { source })?;
        // Step 3: the UI's renderer and the composition overlay, best
        // effort. The overlay must exist before the project's renderer creates
        // its swapchain on this window: a DirectComposition target cannot be
        // created for a window that is already composed, and attaching the
        // renderer composes it (`DCOMPOSITION_ERROR_WINDOW_ALREADY_COMPOSED`).
        // Without any overlay the UI is not drawn at all; the refusal is
        // logged and retried every 120 frames.
        let egui_renderer = egui_wgpu::Renderer::new(
            &device,
            EDITOR_FORMAT,
            egui_wgpu::RendererOptions::default(),
        );
        let no_overlay = no_overlay_requested();
        let overlay = create_overlay(
            event_loop,
            no_overlay,
            &instance,
            &adapter,
            &device,
            Arc::clone(&window),
            size,
        );
        if let Some(overlay) = &overlay {
            info!(
                target: telemetry_target::EDITOR,
                window_surface = matches!(overlay.mode(), OverlayMode::Window),
                floating = matches!(overlay.mode(), OverlayMode::Floating),
                "[editor] overlay posture"
            );
        }
        // Step 4: the game viewport's window first - it carries the primary
        // surface, so the project renders through its own camera into a
        // child window the Game tab shows through. A refusal leaves the
        // primary on the main window and the Game tab merely mirrors what is
        // already drawn there.
        let game_surface = SurfaceWindow::open(event_loop, &window, "Pill Editor Game", "game")
            .map_err(|error| {
                warn!(
                    target: telemetry_target::EDITOR,
                    error = %error,
                    "[editor] game view window refused; the primary surface stays on the main window"
                )
            })
            .ok();
        let (primary_window, primary_size) = match &game_surface {
            Some(game) => (Arc::clone(&game.window), game.size()),
            None => (Arc::clone(&window), size),
        };
        // Step 4b: the host: the project, its extensions and the egui shell
        // module, with the renderer attached to the primary window.
        let host_configuration = HostConfig::from_environment_for_editor_frontend("egui")
            .map_err(pill_core::error::HostError::from)?;
        // The window names the project, as the project's own settings file
        // spells it; the open scene is the document's state and is reported
        // through the scene-title log instead.
        window.set_title(&format!("Pill - {}", host_configuration.name));
        let project_name: std::sync::Arc<str> =
            std::sync::Arc::from(host_configuration.name.as_str());
        let mut host = pill_development_host::setup_rendering(
            host_configuration,
            primary_window,
            primary_size.0,
            primary_size.1,
        )?;
        info!(
            target: telemetry_target::EDITOR,
            detached = game_surface.as_ref().is_some_and(|game| game.detached),
            width = primary_size.0,
            height = primary_size.1,
            "[editor] game surface attached"
        );
        // The gizmo tool's starting state: the project's own
        // remembered choice, the file's last-used values, or the defaults.
        // The project's directory name keys the per-project entry, and the
        // executable is the one place that knows it.
        let gizmo_project = std::env::var("PROJECT_PATH").ok().and_then(|path| {
            std::path::Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
        let gizmo_settings = match &gizmo_project {
            Some(project) => read_gizmo_settings(&layout_directory(), project),
            None => GizmoSettings::default(),
        };
        // The frontend owns the drags. The tool's mode, space and snapping
        // change through the shortcuts, the scripted channel and the recorded
        // `SetGizmoTool` edits; the file's settings stand until one of them
        // changes something.
        let world = host.host_mut().engine_mut().world_mut();
        world.register_resource::<GizmoState>();
        if world.get_resource::<GizmoState>().is_none() {
            world.insert_resource(GizmoState {
                settings: gizmo_settings,
                tool_frame: None,
            });
        }
        // The command resources the assets actions and the scripted channel
        // queue into: the registry, the inbox, the pending batches, the undo
        // history, the entity-pick request and the scene session. The shell
        // re-claims the same set on load.
        ensure_command_resources(world);
        // The editor's project layer loads through the project's asset store,
        // so the `.theme` type is registered here even when the project itself
        // installs no theme.
        pill_ui_theme::register(world);
        // The Console tab's history and filters, filled each frame from the
        // telemetry capture ring.
        world.register_resource::<ConsoleState>();
        if world.get_resource::<ConsoleState>().is_none() {
            world.insert_resource(ConsoleState::default());
        }
        // The module panels: the registry, the queue modules register
        // through and the commands a panel asked for. All three exist before
        // the first frame, so a request a module queued at load has somewhere
        // to be bound and the shell has something to draw from.
        world.register_resource::<PanelRegistry>();
        if world.get_resource::<PanelRegistry>().is_none() {
            world.insert_resource(PanelRegistry::default());
        }
        world.register_resource::<PanelRegistrations>();
        if world.get_resource::<PanelRegistrations>().is_none() {
            world.insert_resource(PanelRegistrations::default());
        }
        world.register_resource::<PanelCommands>();
        if world.get_resource::<PanelCommands>().is_none() {
            world.insert_resource(PanelCommands::default());
        }
        // The shell's side of the OS clipboard: the shell queues a request
        // and this executable's Win32 bridge carries it out.
        world.register_resource::<EditorClipboard>();
        if world.get_resource::<EditorClipboard>().is_none() {
            world.insert_resource(EditorClipboard::default());
        }
        // Step 5: the scene views' child windows and their renderer
        // surfaces, each with its own camera (navigation moves the active
        // viewport's). They start hidden: the first is placed at the dock's
        // default rectangle right after this, and the others show when the
        // dock shows their tabs.
        let cameras = vec![OrbitCamera::default(); DEFAULT_SCENE_VIEWS];
        let mut views: Vec<Option<SceneView>> = (0..DEFAULT_SCENE_VIEWS).map(|_| None).collect();
        for view in 0..DEFAULT_SCENE_VIEWS {
            let Some(mut scene) = SceneView::open(event_loop, &window, view) else {
                continue;
            };
            scene.attach(&mut host, &cameras[view]);
            views[view] = Some(scene);
        }
        // The editor starts in edit mode: the project's game systems and its
        // scripts stay still until the person presses Play, while the systems
        // that opted in to edit mode - the shell's pass, the renderer's frame
        // build, the physics outlines - keep running.
        host.host_mut()
            .engine_mut()
            .set_run_mode(pill_engine_core::RunMode::Editing);
        // The scene grid starts on, because the editor's view-bound viewports
        // draw it. The toolbox's grid row and the viewport menu toggle it
        // through `SetGrid`, which flips this same switch.
        {
            let world = host.host_mut().engine_mut().world_mut();
            if world.get_resource::<SurfaceCameras>().is_none() {
                world.insert_resource(SurfaceCameras::default());
            }
            if let Some(cameras) = world.get_resource_mut::<SurfaceCameras>() {
                cameras.grid.enabled = true;
            }
        }
        let mut editor = EditorWindow {
            window,
            device,
            queue,
            instance,
            adapter,
            host,
            views,
            game_surface,
            popouts: std::collections::BTreeMap::new(),
            cameras,
            play_cameras: None,
            last_run_mode: None,
            active_view: 0,
            left_down: vec![false; DEFAULT_SCENE_VIEWS],
            camera_drag: None,
            modifiers: Modifiers::default(),
            keymap: KeymapPreset::default(),
            camera_dirty: vec![true; DEFAULT_SCENE_VIEWS],
            egui_renderer,
            overlay,
            no_overlay,
            #[cfg(windows)]
            overlay_retry_frame: 0,
            fallback_context: {
                // The fallback draws the same theme as the shell, so a
                // missing shell module does not flip the look.
                let fallback_context = egui::Context::default();
                pill_editor_core::style::apply(&fallback_context);
                fallback_context
            },
            reported_fallback: false,
            pending_events: Vec::new(),
            shell_events: Vec::new(),
            shell_cursor: None,
            notices: EditorNotices::default(),
            scene_cursor: vec![None; DEFAULT_SCENE_VIEWS],
            press_pick: None,
            outlined_entities: Vec::new(),
            overlay_regions: Vec::new(),
            scene_rects: Vec::new(),
            gizmo_drag: None,
            gizmo_geometry: GizmoGeometry::default(),
            element_handles: Vec::new(),
            element_kinds: Vec::new(),
            element_map: gizmo::ElementHandleMap::new(),
            element_logged: None,
            marquee: None,
            hovered_handle: None,
            gizmo_reference: 0.0,
            gizmo_hit_radius: GIZMO_HIT_RADIUS_DEFAULT,
            render_previews: false,
            preview_in_flight: std::collections::HashMap::new(),
            badges: Vec::new(),
            gizmo_guides: None,
            gizmo_logged: None,
            synthetic_press_ids: Vec::new(),
            synthetic_gizmo_pending: None,
            synthetic_gizmo_hold: false,
            synthetic_gizmo_press: None,
            synthetic_gizmo_view: None,
            asset_entries: Vec::new(),
            asset_walk: Vec::new(),
            standalone_types: Vec::new(),
            asset_listing_dirty: true,
            drop_preview: None,
            recovered: pill_editor_core::layout::read_recovery(),
            last_recovery_key: None,
            ime_allowed: false,
            ime_rect: None,
            asset_detail: None,
            asset_detail_path: None,
            preferences: StateFileWatch::new(layout_directory().join(PREFERENCES_FILE)),
            keybindings: StateFileWatch::new(layout_directory().join(KEYBINDINGS_FILE)),
            keybindings_applied: false,
            log_buffer,
            preferences_document: serde_json::Map::new(),
            // The code default until the first poll resolves the layers. The
            // generation matches what `EguiShellState::new` installed, so a
            // project with no layers never re-applies at startup.
            theme: pill_editor_core::style::default_theme(),
            theme_generation: 1,
            theme_layers: vec![ThemeLayerInfo::code()],
            theme_fonts: pill_editor_core::kit::style::ThemeFonts::default(),
            theme_watch: ThemeWatch::default(),
            fallback_theme_generation: 1,
            gizmo_project,
            project_name,
            gizmo_persisted: None,
            gizmo_hidden_persisted: None,
            focused: true,
            overlay_placement: None,
            size,
            scale_factor,
            frame_index: 0,
            last_frame_time: None,
            last_heartbeat: None,
            fps: 0.0,
            started: Instant::now(),
            claimed_pointer: false,
            claimed_keyboard: false,
            last_cursor: egui::CursorIcon::Default,
            test_channel: TestChannel::from_environment(),
            scripted_drag: None,
            scene_title: "untitled".to_string(),
            test_seconds: test_channel::seconds_from_environment(),
            quit: false,
            cadence: {
                let cadence = Cadence::from_environment();
                if cadence.continuous() {
                    info!(
                        target: telemetry_target::EDITOR,
                        variable = pill_editor_core::cadence::CONTINUOUS_REDRAW_ENV,
                        "[editor] redraw continuous"
                    );
                } else {
                    info!(
                        target: telemetry_target::EDITOR,
                        idle_poll_ms = pill_editor_core::cadence::IDLE_POLL.as_millis() as u64,
                        "[editor] redraw on demand"
                    );
                }
                cadence
            },
            demand: FrameDemand::default(),
            next_wake: FrameWake::Immediate,
            repaint_after: None,
            last_demand: FrameDemand::default(),
            polling: false,
            idle_frame_millis_max: 0.0,
            snapshot_changed_field: None,
            module_markers: std::collections::HashMap::new(),
            panel_requests: Vec::new(),
        };
        editor.place_views();
        // A scripted run also gets the fixture tool the element framework's
        // suites drive. It is registered like any other tool, through the
        // registry, and a person's editor never sees it in the toolbox.
        if editor.test_channel.is_some() {
            let world = editor.host.host_mut().engine_mut().world_mut();
            tool_fixture::register(world);
        }
        info!(
            target: telemetry_target::EDITOR,
            width = size.0,
            height = size.1,
            "[editor] window created"
        );
        Ok(editor)
    }
}
