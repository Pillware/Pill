//! The egui editor's shared core: the UI libraries it re-exports, the dock
//! layout model and the editor's view of the engine.
//!
//! # Responsibilities
//!
//! - Re-export `egui` and `egui_dock` as the one copy the editor's frontend,
//!   its reloadable shell module and this crate all name.
//! - Carry the shell boundary ([`boundary`]) and the module panels a dock tab
//!   names ([`panels`]).
//! - Provide the bridge from the shell's window events to egui's input
//!   ([`input`]), the editor's egui style ([`style`]) and Phosphor's vendored
//!   icon fonts with the icons the chrome paints ([`icons`]).
//! - Own the dock layout: the tab set, the default layout and versioned
//!   persistence through `egui_dock`'s state ([`layout`]).
//! - Own the editor's snapshots of the engine and the commands it applies to
//!   it ([`editor_state`]).
//! - Own the clipboard documents ([`clipboard`]), the transform gizmos
//!   ([`gizmo`]) and the floating viewport toolbox ([`toolbox`]).
//! - Own the editor API's callbacks ([`hooks`]): the callbacks an `_editor`
//!   crate registers, their payloads, the context they run with - the world,
//!   the editor state ([`viewport`]), the commands they queue and their
//!   notices - and the frame-boundary dispatch that applies their answers.
//! - Own where a module's commands appear ([`placements`]): the placements, the
//!   state a module publishes for a command and target, and the draw functions
//!   it registers for a free-form place.
//! - Own the tools layer ([`tools`]): the element selection, the tool registry
//!   and the chain of extensions that add to a tool another crate owns, so a
//!   component tool reports its own elements, frames, menu rows and keys and
//!   the editor draws, picks, drags and undoes them.
//! - Declare the layout persistence failures ([`error`]).
//!
//! # Design
//!
//! Nothing here depends on a window or a GPU. The frontend owns the device
//! and draws the shell's frame with `egui-wgpu`, and the shell owns the egui
//! `Context`. `test_engine_layout.py` checks that no windowing or GPU crate is
//! reachable from this one. The crate is a shared `dylib`. The boundary types,
//! the layout and the commands cross between the editor executable and the
//! reloadable shell module, and one compiled copy per process gives them one
//! identity. The module world and the host world must resolve `egui`,
//! `egui_dock` and `epaint` identically, and the manifest pins the union of
//! their features.
//!
//! # The reload-safety contract
//!
//! egui's `Context` keeps arbitrary typed values and callbacks alive past the
//! frame that stored them. Reloadable crates (`extensions/**`,
//! `examples/**`) must never store their own types or closures in it. After a
//! reload, the stored value's drop code and vtable point into the image that
//! was unloaded, so touching the value runs code that is no longer there.
//!
//! The APIs that keep boxed `Any`/`Fn` values inside the context are
//! `Context::data` and `Context::data_mut` (the `IdTypeMap` behind them),
//! `Context::set_request_repaint_callback`, `Context::on_begin_pass` and
//! `Context::on_end_pass` (plugins), and any shape carrying
//! `egui::PaintCallback`. `test_coding_standards.py` flags those calls in
//! reloadable crates. A line that deliberately touches the map carries the
//! `egui-state-ok` marker; only a test may use it, and only to plant a value
//! that a previous generation left behind.
//!
//! Long-lived egui state therefore lives in a world resource defined by this
//! crate: the shell's [`session::EguiShellState`], which a newly loaded shell
//! generation finds in the world and keeps using. The shell clears egui's data
//! map on the first frame of every generation
//! (`pill_editor_shell::shell::claim`), because a value left there
//! by a previous generation would be read through the vtable of an image that
//! is no longer loaded.

pub use egui;
pub use egui_dock;

/// The UI kit: the theme-to-egui translator, the font installer, the scoped
/// style variants and the themed widgets stock egui cannot draw. Re-exported
/// so the shell and the executable never name the rlib directly.
pub use pill_ui_kit as kit;

/// The Assets pipeline: the dialogs' state and the file-command batches the
/// toolbar, the dialogs and the scripted channel queue.
pub mod assets;
/// The plain data crossing the frontend/shell boundary.
pub mod boundary;
/// The editor's brand mark: the pill logotype stored as a precomputed coverage
/// mask.
pub mod brand;
/// The Assets tab's project browser: the open folder and the listing rules the
/// tree and the grid share.
pub mod browser;
/// The redraw cadence: when the editor needs to draw a frame.
pub mod cadence;
/// The scene viewports' orbit cameras and their navigation gestures.
pub mod camera;
/// The clipboard documents that the copy and paste commands share.
pub mod clipboard;
/// The command registry: named actions, menus and rebindable shortcuts.
pub mod commands;
/// The Console tab's history, channels and filters.
pub mod console;
/// The frame boundary: the pending-command drain, the undo/redo walk and the
/// history prune a reload needs.
pub mod drain;
/// The editor's own state documents on disk, all written the same safe way and
/// reported through one error type.
pub mod editor_files;
/// The editor's snapshots of the engine and the commands it applies to it.
pub mod editor_state;
/// The Entities tab's shared actions: the create and delete commands, queued
/// for undo.
pub mod entities;
/// Layout persistence failures.
pub mod error;
/// Undoable edits whose state only an editor extension understands, and the
/// inbox that hands replayed states back to that extension.
pub mod extension_edits;
/// Transform gizmos: handle geometry, the drag calculations and snapping.
pub mod gizmo;
/// The undo and redo history of editor edits.
pub mod history;
/// The editor API an `_editor` crate registers against: callbacks for the
/// editor's edits, their context and the drain's dispatch.
pub mod hooks;
/// Phosphor's icons: the fonts, the glyphs and the duotone painting.
pub mod icons;
/// The bridge from the shell's own window events to egui's input.
pub mod input;
/// The Inspector's editing helpers: field edits and asset settings rows.
pub mod inspector;
/// The `egui_dock` layout: tab set, default layout and persistence.
pub mod layout;
/// The editor's menu bar: its titles, metrics and the items each menu holds.
pub mod menubar;
/// What the host and the editor did, as plain data: toast events and Problems
/// rows.
pub mod notices;
/// The editor panels a module registers: their ids, draw functions, a marker
/// that says whether their module is still loaded, and the dock tab slot they
/// fill.
pub mod panels;
/// Where a module's commands appear and what they look like there: the
/// placements, the per-target state modules publish and the draw functions
/// they register.
pub mod placements;
/// The editor state a play session keeps aside and puts back at Stop.
pub mod play_session;
/// The Preferences tab's shared vocabulary: the file's editable keys and their
/// parsing.
pub mod preferences;
/// Asset previews: the preference that turns them on and the tiles worth
/// drawing one for.
pub mod previews;
/// Build diagnostics: the problems the Problems panel shows.
pub mod problems;
/// The Project Settings tab: every declared setting with where its value comes
/// from, and the file writes behind an edit and a reset.
pub mod project_settings;
/// The comment-preserving line editor the settings tab writes with.
pub mod project_settings_file;
/// The requests one shell pass raises for the editor executable.
pub mod requests;
/// The world resources the Inspector can show and edit.
pub mod resources;
/// The open scene: its path, guid and unsaved marker, and the file operations
/// behind New, Open, Save and Save As.
pub mod scene_workflow;
/// The scripted test channel's grammar: one typed parse of a command line.
pub mod scripted;
/// The egui shell's long-lived session state.
pub mod session;
/// Polling the editor's state files for changes.
pub mod state_watch;
/// The editor's egui theme.
pub mod style;
/// The Systems tab's shared logic: row labels, system toggles and the pause
/// control.
pub mod systems;
/// The floating viewport toolbox: rows, geometry and the edits its rows queue.
pub mod toolbox;
/// The tool layer: the modes a scene view can be in, the registry that holds
/// them and which one is active.
pub mod tools;
/// The work modules hold unsaved in memory, and the quit prompt about it.
pub mod unsaved_work;
/// The viewport state a callback reads.
pub mod viewport;

// =============================================================================
// Test Support
// =============================================================================

/// The one lock the crate's asset-mounting tests share.
#[cfg(test)]
pub(crate) mod test_support {
    /// The mounted asset directory is process-wide, so the tests that mount one
    /// take this lock in turn. They live in several modules, which is why the
    /// lock sits here and not in one module's private test module.
    pub(crate) fn asset_mount_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
