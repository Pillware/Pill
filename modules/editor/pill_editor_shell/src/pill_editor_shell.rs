//! The egui editor shell: the docked UI that reloads while the editor runs.
//!
//! # Responsibilities
//!
//! - Registers the shell's session state and the boundary resources, and the
//!   per-frame system that runs the dock ([`egui_shell_update`]).
//! - Re-claims every resource on every load, so a reload keeps the dock
//!   layout, the egui context and the shared selection.
//!
//! # Design
//!
//! The executable owns the window, the GPU device and the input. This crate
//! owns the dock and the egui pass, and the two exchange plain data
//! ([`EguiShellInput`], [`EguiShellFrame`]) in the `pill_editor_core` dylib
//! both link. The shell turns that input into egui's own raw input.
//!
//! This file holds the crate's hot system. To patch [`egui_shell_update`],
//! the hot-patch machinery copies it into a crate of its own and replays this
//! file's top-level `use` lines there. Everything the system calls therefore
//! has to be `pub` in [`shell`] and reachable by its package path, because a
//! helper declared in this file does not exist in the patch crate.
//!
//! An edit in `shell.rs` is patched the same way. That patch crate replays the
//! file's `use` lines and adds `use pill_editor_shell::*;` on top,
//! and the glob reaches this module's items, which is why the shell's public
//! items are re-exported below. Without the re-export, a tab body's bare-name
//! call to a sibling would not resolve, and the edit would fall back to a
//! reload.

// The crate's own name, so the `pub use` below resolves inside this crate
// too. A generated patch crate replays the same `use` lines; there the name
// is the linked crate itself.
extern crate self as pill_editor_shell;

// External crates
use pill_core::telemetry::telemetry_target;
use pill_editor_core::boundary::{EguiPopoutInputs, EguiShellFrame, EguiShellInput};
use pill_editor_core::commands::{ensure_command_resources, PendingEditorCommands};
use pill_editor_core::editor_state::{EditorSelection, EditorSnapshot};
use pill_editor_core::notices::EditorNotices;
use pill_editor_core::panels::{PanelCommands, PanelRegistrations, PanelRegistry, PanelRequest};
use pill_editor_core::session::EguiShellState;
use pill_engine::{pill_module, system, Engine, Res, ResMut};

// Current crate
// The shell's public items, re-exported so a patch crate's
// `use pill_editor_shell::*;` reaches them (see the crate header).
// `run_frame` comes through this glob as well, which is why the system below
// names it bare.
pub use pill_editor_shell::shell::*;

/// The shell's UI work: the dock, the tab bodies and the frame build.
pub mod shell;

// The build script scans this crate and writes one address entry per function
// into `function_inventory.rs`. Including that file is what makes every
// function resolvable by qualified path, although nothing in its source is
// annotated. Without it the host can patch only the `#[pill_hot]` system,
// whose address rides the engine registry, and a body edit to
// `shell::run_frame` falls back to a reload.
include!(concat!(env!("OUT_DIR"), "/function_inventory.rs"));

// =============================================================================
// Registration
// =============================================================================

/// Registers the shell against the host engine. Returns zero on success.
///
/// This is the half a reload repeats: it re-claims every resource it owns. A
/// reload releases the resources that the arriving generation does not register
/// again, and registering an existing one keeps its value instead of resetting
/// it.
pub fn register(engine: &mut Engine) -> u32 {
    let world = engine.world_mut();
    // Step 1: the command resources every editor action queues into - the
    // command registry, the inbox, the pending batches, the entity pick
    // request, the history and the scene session. The executable shares them,
    // and re-claiming them lets a reload keep them.
    ensure_command_resources(world);
    // Step 2: the session - the egui context and the dock - built on the first
    // load only. Its type lives in `pill_editor_core`, so a reload of this
    // module finds the same resource and keeps the context alive.
    world.register_resource::<EguiShellState>();
    if world.get_resource::<EguiShellState>().is_none() {
        world.insert_resource(EguiShellState::new());
    }
    // Step 3: the boundary resources, shared with the executable.
    world.register_resource::<EguiShellInput>();
    if world.get_resource::<EguiShellInput>().is_none() {
        world.insert_resource(EguiShellInput::default());
    }
    world.register_resource::<EguiShellFrame>();
    if world.get_resource::<EguiShellFrame>().is_none() {
        world.insert_resource(EguiShellFrame::default());
    }
    // The popped-out windows' input: one record per tab. The frontend fills a
    // record, and the shell reads it when it runs that tab's pass.
    world.register_resource::<EguiPopoutInputs>();
    if world.get_resource::<EguiPopoutInputs>().is_none() {
        world.insert_resource(EguiPopoutInputs::default());
    }
    // Step 4: what the tabs read and the selection they write, re-claimed so a
    // reload keeps which entity was selected. The executable refreshes the
    // snapshot every frame; without one the dock draws empty rather than
    // failing on a missing resource.
    world.register_resource::<EditorSnapshot>();
    if world.get_resource::<EditorSnapshot>().is_none() {
        world.insert_resource(EditorSnapshot::default());
    }
    world.register_resource::<EditorSelection>();
    if world.get_resource::<EditorSelection>().is_none() {
        world.insert_resource(EditorSelection::default());
    }
    // The host's notices: the executable replaces them whenever the host
    // reports a reload, a patch or a failure. The type is shared, so the shell
    // reads whatever the current generation wrote.
    world.register_resource::<EditorNotices>();
    if world.get_resource::<EditorNotices>().is_none() {
        world.insert_resource(EditorNotices::default());
    }
    // Step 5: this module's own editor panels. Their requests are queued here
    // rather than registered: the executable ties each request to a liveness
    // marker that dies with this module's image, so the draw function of an
    // unloaded module is never called.
    world.register_resource::<PanelRegistrations>();
    if world.get_resource::<PanelRegistrations>().is_none() {
        world.insert_resource(PanelRegistrations::default());
    }
    if let Some(registrations) = world.get_resource_mut::<PanelRegistrations>() {
        registrations.push(PanelRequest {
            id: "shell.demo",
            title: "Demo Panel",
            source: "pill_editor_shell",
            draw: shell::demo_panel,
        });
        registrations.push(PanelRequest {
            id: shell::SPLINE_PANEL_ID,
            title: "Spline Tool",
            source: "pill_editor_shell",
            draw: shell::spline_tool_panel,
        });
    }
    // Step 6: the per-frame system that runs the dock needs no call of its
    // own. `egui_shell_update` declares itself as a post-update system, like
    // the renderer's frame build, so it keeps running while the engine is
    // paused; the engine registers it before this function runs, on every
    // generation.
    // Note: the egui editor's end-to-end suite looks for this exact line.
    pill_core::info!(
        target: telemetry_target::EDITOR,
        "[editor] pill_editor_shell loaded"
    );
    0
}

/// The artifact's one-time entry point: nothing is one-time here.
///
/// Every resource and system the shell owns is re-claimed by [`register`] on
/// each generation, which is exactly what a reload has to repeat. This function
/// exists because a statically linked build loads the module once and calls
/// this entry point alone, and because the generated
/// `host_module_pill_editor_shell` wrapper carries it as
/// `pill_module_init`.
#[pill_module(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    register(engine)
}

// =============================================================================
// egui_shell_update
// =============================================================================

/// Runs one egui pass: the dock and its tabs, then the frame the executable
/// draws.
///
/// The system runs inside the host's frame, between the executable queueing
/// its input and the presentation. Before the frame, the executable writes
/// [`EguiShellInput`]: geometry, focus and the events of the editor's own
/// windows. After it, it drains [`EguiShellFrame`].
#[allow(clippy::type_complexity)]
#[system(
    name = "egui_shell",
    run_phase = "post_project",
    run_in_edit_mode = true
)]
pub fn egui_shell_update(
    resources: (
        ResMut<EguiShellState>,
        ResMut<EguiShellInput>,
        ResMut<EguiShellFrame>,
        // The nested tuples keep this system within the engine's limit of
        // seven system parameters.
        (
            Res<EditorSnapshot>,
            ResMut<EditorSelection>,
            Res<EditorNotices>,
        ),
        (
            ResMut<PendingEditorCommands>,
            // The panels the loaded modules registered, and the commands those
            // panels asked for this pass.
            Res<PanelRegistry>,
            ResMut<PanelCommands>,
            // The shell's side of the OS clipboard.
            ResMut<pill_editor_core::clipboard::EditorClipboard>,
        ),
        // Each popped-out window's input.
        Res<EguiPopoutInputs>,
        // What the modules publish for the shell to draw: their Inspector
        // header actions' states, the state of the commands they placed, the
        // draw functions they registered, and the unsaved work the quit prompt
        // and the Assets tab show.
        (
            Res<pill_editor_core::placements::CommandStates>,
            Res<pill_editor_core::placements::CommandDraws>,
            ResMut<pill_editor_core::unsaved_work::UnsavedWork>,
        ),
    ),
) {
    // The resources are this crate's own registrations. A world without them
    // is not an editor's world, so there is nothing to run.
    let (
        mut shell_state,
        mut shell_input,
        mut shell_frame,
        (editor_snapshot, mut editor_selection, editor_notices),
        (mut pending_commands, panel_registry, mut panel_commands, mut editor_clipboard),
        popout_window_inputs,
        (command_states, command_draws, mut unsaved_work),
    ) = resources;
    let (
        Some(mut shell_state),
        Some(shell_input),
        Some(mut shell_frame),
        (Some(editor_snapshot), Some(mut editor_selection), Some(editor_notices)),
        (
            Some(mut pending_commands),
            Some(panel_registry),
            Some(mut panel_commands),
            Some(mut editor_clipboard),
        ),
        Some(popout_window_inputs),
        (Some(command_states), Some(command_draws), Some(mut unsaved_work)),
    ) = (
        shell_state.get_mut(),
        shell_input.get_mut(),
        shell_frame.get_mut(),
        (
            editor_snapshot.get(),
            editor_selection.get_mut(),
            editor_notices.get(),
        ),
        (
            pending_commands.get_mut(),
            panel_registry.get(),
            panel_commands.get_mut(),
            editor_clipboard.get_mut(),
        ),
        popout_window_inputs.get(),
        (
            command_states.get(),
            command_draws.get(),
            unsaved_work.get_mut(),
        ),
    )
    else {
        return;
    };
    run_frame(
        &mut shell_state,
        &shell_input,
        &mut shell_frame,
        editor_snapshot,
        &mut editor_selection,
        &mut pending_commands,
        editor_notices,
        panel_registry,
        &mut panel_commands.commands,
        &mut editor_clipboard,
        popout_window_inputs,
        command_states,
        command_draws,
        &mut unsaved_work,
    );
}
