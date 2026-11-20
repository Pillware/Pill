//! Tests for the play toolbar over the menu bar.
//!
//! # Responsibilities
//!
//! - Show that the toolbar draws its three controls, that its group rectangle
//!   joins the frame's overlay regions so a press on it is never a scene pick,
//!   and that it sits inside the menu bar's strip.
//! - Show that each control queues the command it means, unrecorded: Play while
//!   editing, Stop while a session runs, Pause or Resume depending on the pause
//!   state, and Step.
//!
//! # Design
//!
//! The group's rectangle is what the shell reports, so one pass through
//! `play_toolbar` is the whole geometry subject. For the presses, the controls
//! call [`run_play_control`] with the command the model derives
//! (`play_toggle_command` for the run mode, `pause_toggle_command` for the
//! pause state), and the model's own table tests pin those two, so what is left
//! to the shell is the queue: one unrecorded batch per press. A click itself
//! cannot be posed here - egui's `Area` does not receive synthetic pointer
//! events in this harness - so the suite presses the controls in a real window
//! instead.

// External crates
use pill_engine::Engine;

// Current crate
use pill_editor_shell::shell::prelude::*;
use pill_editor_shell::shell::*;

// =============================================================================
// Helpers
// =============================================================================

/// Runs one toolbar pass over a fixed 1280x720 screen and returns the group
/// rectangle it claimed.
///
/// `#[cfg(test)]` keeps it out of the hot-patch function inventory, which names
/// every module-level function and cannot name one the library build leaves out.
#[cfg(test)]
fn toolbar_pass(
    context: &egui::Context,
    snapshot: &EditorSnapshot,
    pending: &mut PendingEditorCommands,
    session: &mut MenubarSession,
) -> Option<ShellRect> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 720.0),
        )),
        events: Vec::new(),
        ..Default::default()
    };
    let mut group = None;
    let mut frame_output = context.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            let mut toolbox = ToolboxSession::default();
            let mut inspector = InspectorSession::default();
            let mut browser = ProjectBrowser::default();
            let mut dialog: Option<AssetDialog> = None;
            let mut draft: Option<preferences::PreferencesDraft> = None;
            let mut deferred = FrameRequests::default();
            let mut console_filters = ConsoleFilterSettings::default();
            let mut requests = ShellRequests::default();
            let mut stats = StatisticsSession::default();
            let panels = PanelRegistry::default();
            let mut budget = PanelBudget::default();
            let mut panel_commands = Vec::new();
            let mut panel_windows = Vec::new();
            let mut clipboard = pill_editor_core::clipboard::EditorClipboard::default();
            let mut previews = pill_editor_core::session::PreviewStates::default();
            let mut selection = EditorSelection::default();
            let mut viewer = EditorTabViewer {
                snapshot,
                selection: &mut selection,
                asset_dialog: &mut dialog,
                pending,
                preferences_draft: &mut draft,
                toolbox: &mut toolbox,
                browser: &mut browser,
                inspector: &mut inspector,
                scene_rects: Vec::new(),
                game_rect: None,
                overlay_regions: Vec::new(),
                frames_per_second: 60.0,
                active_tabs: &[],
                deferred: &mut deferred,
                console_filters: &mut console_filters,
                requests: &mut requests,
                stats: &mut stats,
                panels: &panels,
                panel_budget: &mut budget,
                panel_commands: &mut panel_commands,
                panel_entities: &[],
                panel_detail: None,
                panel_answer: None,
                panel_window_requests: &mut panel_windows,
                pending_panel_window: None,
                pending_panel_window_used: false,
                clipboard: &mut clipboard,
                previews: &mut previews,
                badges: &[],
                marquee: None,
            };
            play_toolbar(ui, session, &mut viewer);
            group = viewer.overlay_regions.last().copied();
        });
    });
    frame_output.textures_delta.clear();
    group
}

// =============================================================================
// Tests
// =============================================================================

/// The toolbar draws, claims one rectangle as an overlay region, and sits in
/// the menu bar's strip.
#[test]
fn the_play_toolbar_claims_its_rectangle_over_the_bar() {
    let engine = Engine::new();
    let snapshot = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    let mut pending = PendingEditorCommands::default();
    let mut session = MenubarSession::default();
    let context = egui::Context::default();
    style::apply(&context);

    let group = toolbar_pass(&context, &snapshot, &mut pending, &mut session)
        .expect("the toolbar claims its rectangle");

    assert!(group.width > 0.0 && group.height > 0.0, "the bar drew");
    assert!(
        group.y < pill_editor_core::menubar::MENU_BAR_HEIGHT,
        "the toolbar sits in the menu bar, but it starts at {}",
        group.y
    );
    assert!(
        group.width < 600.0,
        "three controls, not the whole strip: the group is {} wide",
        group.width
    );
    assert!(pending.batches.is_empty(), "an idle pass queues nothing");
}

/// Queues one control's command through the path its button calls, and returns
/// what the queue holds.
#[cfg(test)]
fn run_control(
    snapshot: &EditorSnapshot,
    control: &str,
    command: EditorCommand,
) -> Vec<EditorCommand> {
    let mut pending = PendingEditorCommands::default();
    let mut toolbox = ToolboxSession::default();
    let mut inspector = InspectorSession::default();
    let mut browser = ProjectBrowser::default();
    let mut dialog: Option<AssetDialog> = None;
    let mut draft: Option<preferences::PreferencesDraft> = None;
    let mut deferred = FrameRequests::default();
    let mut console_filters = ConsoleFilterSettings::default();
    let mut requests = ShellRequests::default();
    let mut stats = StatisticsSession::default();
    let panels = PanelRegistry::default();
    let mut budget = PanelBudget::default();
    let mut panel_commands = Vec::new();
    let mut panel_windows = Vec::new();
    let mut clipboard = pill_editor_core::clipboard::EditorClipboard::default();
    let mut previews = pill_editor_core::session::PreviewStates::default();
    let mut selection = EditorSelection::default();
    let mut viewer = EditorTabViewer {
        snapshot,
        selection: &mut selection,
        asset_dialog: &mut dialog,
        pending: &mut pending,
        preferences_draft: &mut draft,
        toolbox: &mut toolbox,
        browser: &mut browser,
        inspector: &mut inspector,
        scene_rects: Vec::new(),
        game_rect: None,
        overlay_regions: Vec::new(),
        frames_per_second: 60.0,
        active_tabs: &[],
        deferred: &mut deferred,
        console_filters: &mut console_filters,
        requests: &mut requests,
        stats: &mut stats,
        panels: &panels,
        panel_budget: &mut budget,
        panel_commands: &mut panel_commands,
        panel_entities: &[],
        panel_detail: None,
        panel_answer: None,
        panel_window_requests: &mut panel_windows,
        pending_panel_window: None,
        pending_panel_window_used: false,
        clipboard: &mut clipboard,
        previews: &mut previews,
        badges: &[],
        marquee: None,
    };
    run_play_control(control, command, &mut viewer);
    assert_eq!(pending.batches.len(), 1, "one press queues one batch");
    let batch = &pending.batches[0];
    assert!(!batch.record, "a session is not an undo step");
    batch.commands.clone()
}

/// Each control queues what its label says, in both run modes and both pause
/// states.
#[test]
fn the_play_controls_queue_their_commands_unrecorded() {
    let mut engine = Engine::new();
    engine.set_run_mode(pill_engine::RunMode::Editing);
    let editing = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    assert_eq!(
        run_control(
            &editing,
            "play",
            pill_editor_core::menubar::play_toggle_command(pill_engine::RunMode::Editing),
        ),
        vec![EditorCommand::EnterPlay],
        "the first control starts a session while the editor is editing"
    );

    engine.set_run_mode(pill_engine::RunMode::Playing);
    let mut playing = EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new());
    assert_eq!(
        run_control(
            &playing,
            "play",
            pill_editor_core::menubar::play_toggle_command(pill_engine::RunMode::Playing),
        ),
        vec![EditorCommand::ExitPlay],
        "the first control ends a running session"
    );

    assert_eq!(
        run_control(
            &playing,
            "pause",
            pill_editor_core::systems::pause_toggle_command(false)
        ),
        vec![EditorCommand::SetPaused { paused: true }],
        "a running session pauses"
    );
    playing.paused = true;
    assert_eq!(
        run_control(
            &playing,
            "pause",
            pill_editor_core::systems::pause_toggle_command(true)
        ),
        vec![EditorCommand::SetPaused { paused: false }],
        "a paused session resumes"
    );
    assert_eq!(
        run_control(&playing, "step", EditorCommand::StepOnce),
        vec![EditorCommand::StepOnce],
        "the third control steps one frame"
    );
}
