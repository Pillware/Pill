//! The shell's UI work: the dock, the tabs and the frame build.
//!
//! # Responsibilities
//!
//! - [`run_frame`]: one egui pass, from the boundary input to the boundary
//!   frame the executable draws, including the scene and game rectangles the
//!   frontend places their child windows at.
//! - The tab bodies ([`entities_tab`], [`systems_tab`], [`inspector_tab`],
//!   [`assets_tab`], [`preferences_tab`], all `#[pill_hot_fn]` so a body edit
//!   patches live). The scene and game tabs draw nothing: their bodies stay
//!   holes so the child windows under the overlay show through.
//! - The module's declarations: one `pub mod` per file under `shell/`, and the
//!   re-exports a patch crate reaches those items through.
//!
//! # Design
//!
//! The dock is the session's `egui_dock` state, so a layout edit changes a
//! resource that survives into the next generation. Nothing here stores
//! anything in egui's `Context` (the reload-safety contract `pill_editor_core`
//! documents); the tabs read the snapshot and write the shared selection.
//!
//! # Patchability
//!
//! A patch crate holds one function body and reaches this module only through
//! the crate root's re-export. Every item an edited body can name is therefore
//! `pub` here, and every module is too, because the build script's function
//! inventory names each addressable function by its module path. A body that
//! names a private item fails to compile as a patch, and the host reloads
//! instead.
//!
//! That rule has a shape too: the scanner refuses an `impl` header containing
//! `<` or `where`, so a method in `impl TabViewer for EditorTabViewer<'_>` is
//! never addressable and no body inside it can be patched. Such a method
//! forwards to a `#[pill_hot_fn] pub fn` beside the impl, and the body lives
//! there.

// External crates
use pill_core::telemetry::telemetry_target;

// Current crate
use pill_editor_shell::shell::prelude::*;

/// The shared names every shell module globs.
pub mod prelude;

/// The [`claim`] module: the generation's claim on the shared egui context.
pub mod claim;

pub use claim::claim_context_once;
pub use claim::claim_shell_context;

/// The [`tab_systems`] module: the Systems tab's pause and step controls and
/// its recorded system toggles.
pub mod tab_systems;

pub use tab_systems::systems_tab;

/// The [`toasts`] module: the host's notices drawn as a short-lived stack.
pub mod toasts;

pub use toasts::{draw_toasts, push_notices};

/// The [`bar`] module: the menu bar's rows and the actions they run.
pub mod bar;

pub use bar::about_window;
pub use bar::menu_bar;
pub use bar::menu_body;
pub use bar::play_button;
pub use bar::play_toolbar;
pub use bar::run_menu_action;
pub use bar::run_play_control;
pub use bar::MENU_TITLE_GAP;
pub use bar::PLAY_BUTTON_GAP;
pub use bar::PLAY_TOOLBAR_TOP;

/// The [`palette`] module: the searchable command popup.
pub mod palette;

pub use palette::palette_window;

/// The [`tab_console`] module: the Console tab's filters and rows.
pub mod tab_console;

pub use tab_console::console_tab;

/// The [`tab_problems`] module: build errors and warnings, grouped by module.
pub mod tab_problems;

pub use tab_problems::problems_tab;

/// The [`tab_statistics`] module: frame, pass and per-tab timings.
pub mod tab_statistics;

pub use tab_statistics::statistics_tab;

/// The [`tab_demo`] module: the shell module's own editor panel, the reference
/// a module's panel is written against.
pub mod tab_demo;

pub use tab_demo::demo_panel;

/// The [`panel_windows`] module: the windows and modals module panels ask the
/// shell for.
pub mod panel_windows;

pub use panel_windows::{
    panel_windows, PanelFeed, PANEL_MODAL_APPLY_BUTTON, PANEL_MODAL_BUTTONS,
    PANEL_MODAL_CANCEL_BUTTON,
};

/// The [`prompt`] module: the modal prompt every editor prompt is built from -
/// a title, a body the caller draws, and a row of answers.
pub mod prompt;

pub use prompt::{prompt_buttons, prompt_window, Prompt, PromptButton};

/// The [`quit_prompt`] module: the prompt shown while unsaved module work
/// holds a quit back.
pub mod quit_prompt;

pub use quit_prompt::{
    quit_prompt_window, unsaved_items_body, QUIT_PROMPT_ANSWERS, QUIT_PROMPT_BUTTONS,
    QUIT_PROMPT_ENTER_BUTTON, QUIT_PROMPT_ESCAPE_BUTTON, QUIT_PROMPT_ITEMS_SHOWN,
    QUIT_PROMPT_WIDTH,
};

/// The [`spline_tool`] module: the Spline Tool panel and its re-seed modal.
pub mod spline_tool;

pub use spline_tool::{
    reseed_command, reseed_window, spline_tool_panel, Reseed, RESEED_POINTS, RESEED_WINDOW,
    SPLINE_COMPONENT, SPLINE_PANEL_ID,
};

/// The [`resource_section`] module: the Inspector's rows for one editable
/// world resource.
pub mod resource_section;

pub use resource_section::{queue_edit as queue_resource_edit, resource_section};

/// The [`views`] module: the scene and game viewports, their geometry and
/// their overlay chips.
pub mod views;

pub use views::frame_counter_rect;
pub use views::scene_badges;
pub use views::scene_body_rect;
pub use views::scene_marquee;
pub use views::scene_overlay;
pub use views::shell_rect;
pub use views::SCENE_COUNTER_HEIGHT;
pub use views::SCENE_COUNTER_MARGIN;
pub use views::SCENE_COUNTER_WIDTH;
pub use views::SLOW_UPDATE_MICROS;

/// The [`chrome`] module: the chrome the tabs share: icons, selection moves,
/// context menus and the rectangles the frame reports as the UI's.
pub mod chrome;

pub use chrome::apply_tab_redock;
pub use chrome::claim_pointer;
pub use chrome::click_from_modifiers;
pub use chrome::context_menu;
pub use chrome::empty_area;
pub use chrome::engine_badge;
pub use chrome::floating_popup;
pub use chrome::gizmo_mode_label;
pub use chrome::gizmo_space_label;
pub use chrome::log_context_menu;
pub use chrome::mark_menu_region;
pub use chrome::paint_active_tab_underline;
pub use chrome::remember_moved_rect;
pub use chrome::select_asset;
pub use chrome::select_entity;
pub use chrome::select_entity_click;
pub use chrome::tab_icon;
pub use chrome::SelectionClick;

/// The [`menus`] module: the context menus the tabs open.
pub mod menus;
pub use menus::component_menu_items;
pub use menus::scene_context_menu;

/// The button every place draws a placed command with.
pub mod placed_commands;
pub use placed_commands::hover_with;
pub use placed_commands::paint_state_dots;
pub use placed_commands::remember_command_rect;
pub use placed_commands::PLACED_COMMAND_SIZE;
pub use placed_commands::{
    asset_menu_rows, component_menu_rows, draw_placed_command, draw_placed_menu_row,
    entity_menu_rows, rows_for_slot, toolbox_rows, PlacementHost,
};

/// The [`toolbox_panel`] module: the floating toolbox over a scene viewport.
pub mod toolbox_panel;

pub use toolbox_panel::toolbox_overlay;

/// The [`rows`] module: the one label/control row every tab draws its fields
/// with.
pub mod rows;

pub use rows::control_column;
pub use rows::label_cell;
pub use rows::label_cell_in;
pub use rows::label_column_width;
pub use rows::label_gap;
pub use rows::row;
pub use rows::row_in;
pub use rows::value_row;
pub use rows::value_row_in;
pub use rows::CONTROL_MIN_WIDTH;
pub use rows::LABEL_COLUMN_FRACTION;
pub use rows::LABEL_COLUMN_MAX_WIDTH;
pub use rows::LABEL_COLUMN_MIN_WIDTH;
pub use rows::ROW_HEIGHT;
pub use rows::ROW_LABEL_GAP;
pub use rows::ROW_LABEL_WIDTH;
pub use rows::WIDE_ROW_LABEL_WIDTH;

/// The [`settle`] module: the stage after a shell pass - the requests the
/// pass deferred and the frame it produced.
pub mod settle;

pub use settle::settle_pass;
pub use settle::FrameRequests;

/// The [`tab_entities`] module: the entities tab: the entity rows and their menu.
pub mod tab_entities;

pub use tab_entities::entities_tab;
pub use tab_entities::entity_menu_items;
pub use tab_entities::entity_rows;
pub use tab_entities::remember_hierarchy_row_rect;
pub use tab_entities::DraggedEntity;

/// The [`tab_preferences`] module: the preferences tab: the editor-wide rows and the project's gizmo state.
pub mod tab_preferences;

pub use tab_preferences::preferences_tab;

/// The [`tab_inspector`] module: the inspector tab: the selection's components and their rows.
pub mod tab_inspector;

pub use tab_inspector::addable_components;
pub use tab_inspector::entity_inspector;
pub use tab_inspector::inspector_add_popup;
pub use tab_inspector::inspector_header;
pub use tab_inspector::inspector_section_commands;
pub use tab_inspector::inspector_section_header;
pub use tab_inspector::inspector_tab;
pub use tab_inspector::INSPECTOR_VECTOR_FIELD_HEIGHT;
pub use tab_inspector::INSPECTOR_VECTOR_FIELD_WIDTH;
pub use tab_inspector::INSPECTOR_VECTOR_LABEL_WIDTH;
pub use tab_inspector::SECTION_LINE_GAP;
pub use tab_inspector::VECTOR_OWN_LINE_INDENT;
pub use tab_inspector::VECTOR_SLOT_GAP;
pub use tab_inspector::VECTOR_SLOT_MIN_WIDTH;

/// The [`inspector_fields`] module: the inspector's field rows: the decorators
/// a component declares, the row model and the control each row draws.
pub mod inspector_fields;

pub use inspector_fields::field_widget_name;
pub use inspector_fields::has_field_editor;
pub use inspector_fields::inspector_entity_row;
pub use inspector_fields::inspector_field_row;
pub use inspector_fields::inspector_row;
pub use inspector_fields::is_scalar_field;
pub use inspector_fields::reference_button_width;
pub use inspector_fields::remember_entity_row_rect;
pub use inspector_fields::remember_entity_row_text;
pub use inspector_fields::side_button_size;
pub use inspector_fields::slot_width_within;
pub use inspector_fields::vector_row;
pub use inspector_fields::vector_slot_width;
pub use inspector_fields::vector_slots;
pub use inspector_fields::EntityRowInput;

/// The [`inspector_widgets`] module: the inspector's field controls.
pub mod inspector_widgets;

pub use inspector_widgets::ascending;
pub use inspector_widgets::axis_slot;
pub use inspector_widgets::component_rows;
pub use inspector_widgets::enum_edit;
pub use inspector_widgets::fill_slider_width;
pub use inspector_widgets::fixed_width_field;
pub use inspector_widgets::format_field_value;
pub use inspector_widgets::integer_edit;
pub use inspector_widgets::read_only_row;
pub use inspector_widgets::scalar_edit;
pub use inspector_widgets::scalar_edit_decorated;
pub use inspector_widgets::stretched_drag_value;
pub use inspector_widgets::FieldEditResponse;
pub use inspector_widgets::Number;

/// The [`inspector_settings`] module: the inspector's asset import settings.
pub mod inspector_settings;

pub use inspector_settings::asset_settings_row;
pub use inspector_settings::asset_settings_rows;
pub use inspector_settings::settings_commit;
pub use inspector_settings::SettingsWalk;

/// The [`tab_assets`] module: the assets tab: the folder tree and the open folder's grid.
pub mod tab_assets;

pub use tab_assets::assets_grid;
pub use tab_assets::assets_path_bar;
pub use tab_assets::assets_tab;
pub use tab_assets::assets_tree;
pub use tab_assets::assets_zoom_bar;
pub use tab_assets::folder_tree_rows;
pub use tab_assets::ASSETS_TREE_CARET_SIZE;
pub use tab_assets::ASSETS_TREE_CARET_WIDTH;
pub use tab_assets::ASSETS_TREE_INDENT;
pub use tab_assets::ASSETS_TREE_WIDTH;
pub use tab_assets::ASSETS_TREE_WIDTH_RANGE;

/// The [`assets_tiles`] module: the assets tab's tiles, their menu and the asset dialog.
pub mod assets_tiles;

pub use assets_tiles::asset_container_menu;
pub use assets_tiles::asset_dialog_window;
pub use assets_tiles::asset_icon;
pub use assets_tiles::asset_menu_items;
pub use assets_tiles::asset_tile;
pub use assets_tiles::asset_type_icon;
pub use assets_tiles::remember_pane_rect;
pub use assets_tiles::remember_tree_row_rect;
pub use assets_tiles::tile_name_job;
pub use assets_tiles::AssetTileWrites;
pub use assets_tiles::ASSETS_TILE_CORNER_RADIUS;
pub use assets_tiles::ASSETS_TILE_GAP;
pub use assets_tiles::ASSETS_TILE_ICON_INSET;
pub use assets_tiles::ASSETS_TILE_LABEL_GAP;
pub use assets_tiles::ASSETS_TILE_LABEL_HEIGHT;
pub use assets_tiles::ASSETS_TILE_LABEL_ROWS;

/// The [`tests`] module: the shell's unit tests; the parent compiles it into
/// the test build only.
#[cfg(test)]
pub mod tests;

/// The play toolbar's own tests: the rectangle it claims over the bar, and the
/// commands its controls queue.
#[cfg(test)]
pub mod play_toolbar_geometry_tests;

// =============================================================================
// Types
// =============================================================================

/// Draws the dock's tabs; the tab bodies come from the tab functions.
///
/// The type and every field are public because a hot patch for [`run_frame`]
/// is compiled as its own crate, and the generated source builds this value
/// through the package path. The rule is the crate header's: everything a hot
/// function's body names must be reachable from outside.
pub struct EditorTabViewer<'a> {
    /// The engine data the tabs read.
    pub snapshot: &'a EditorSnapshot,
    /// The shared selection the tabs read and write.
    pub selection: &'a mut EditorSelection,
    /// The asset dialog that is open, if one is; the Assets tab and the
    /// dialog window both read and write it.
    pub asset_dialog: &'a mut Option<AssetDialog>,
    /// The queue the assets actions push their recorded batches into.
    pub pending: &'a mut PendingEditorCommands,
    /// The Preferences tab's unsaved field buffers; the tab seeds them
    /// from the snapshot and a save clears them.
    pub preferences_draft: &'a mut Option<preferences::PreferencesDraft>,
    /// The floating viewport toolboxes' session state: where each scene
    /// view's toolbox floats and what the log already reported.
    pub toolbox: &'a mut ToolboxSession,
    /// The Inspector's collapsed sections and its logged geometry.
    pub inspector: &'a mut InspectorSession,
    /// The Assets tab's project browser: the folder its grid lists, the
    /// tree's open folders and the tile size.
    pub browser: &'a mut ProjectBrowser,
    /// The scene viewports' body rectangles this pass, in logical points, one
    /// slot per view index. The frontend places each viewport's child window
    /// at its slot. The list is sized from the dock before the pass, so a view
    /// the person just added already has a slot.
    pub scene_rects: Vec<Option<ShellRect>>,
    /// The game viewport's body rectangle this pass, in logical points; the
    /// frontend places the primary surface's child window at it.
    pub game_rect: Option<ShellRect>,
    /// The regions drawn over the viewport this pass, in logical points; a
    /// click inside one belongs to the UI, not to a scene pick. The overlay
    /// chips, the menus, the toasts and the dialogs all add their rectangles
    /// here.
    pub overlay_regions: Vec<ShellRect>,
    /// The editor's smoothed frame rate, for the scene's counter.
    pub frames_per_second: f32,
    /// The active tab of each leaf this pass, one entry per leaf. The tab bar
    /// draws its accent underline under these tabs only, so an unselected tab
    /// keeps the strip's neutral line.
    pub active_tabs: &'a [EditorTab],
    /// What the pass could not do while the dock and its tabs were borrowed:
    /// pop a tab out or re-dock one, open a new scene view, hand a dragged
    /// tile to the OS, or quit. Each request is written where it is raised and
    /// applied by `settle_pass` after the pass.
    pub deferred: &'a mut FrameRequests,
    /// The Console tab's filters; the executable applies them
    /// to the history on its next frame.
    pub console_filters: &'a mut ConsoleFilterSettings,
    /// The requests this pass raises for the executable: the Problems row a
    /// click asked to open, the menu's undo and redo steps, and the dock tab it
    /// asked to show. `settle_pass` moves the value onto the frame the
    /// executable drains.
    pub requests: &'a mut ShellRequests,
    /// The timings the Statistics tab reads: the shell writes
    /// them around each tab body, the tab only formats them.
    pub stats: &'a mut StatisticsSession,
    /// The panels the loaded modules registered: a module
    /// tab's title, body and liveness all come from here.
    pub panels: &'a PanelRegistry,
    /// Every module panel's build time, written around each panel body and
    /// read by the tab's badge and by the budget toast.
    pub panel_budget: &'a mut PanelBudget,
    /// The commands the module panels asked for this pass, applied by the
    /// executable at the frame boundary.
    pub panel_commands: &'a mut Vec<PanelCommand>,
    /// The world's entities, as module panels see them.
    pub panel_entities: &'a [PanelEntityRow],
    /// The selected entity's components, as module panels see them.
    pub panel_detail: Option<&'a PanelEntityDetail>,
    /// The answer a modal window took, read during the following pass. It is
    /// owned here so the viewer never borrows the panel-window state that the
    /// settle stage clears.
    pub panel_answer: Option<pill_editor_core::panels::PanelAnswer>,
    /// The windows the panels asked to open or close this pass, with the slot
    /// that asked: applied after the dock, before the windows draw.
    pub panel_window_requests: &'a mut Vec<(u16, PanelCommand)>,
    /// The slot a scripted window request named, and the window id it wants
    /// that panel to open.
    pub pending_panel_window: Option<(u16, &'a str)>,
    /// Whether the pending window's panel drew this pass, so the request is
    /// spent.
    pub pending_panel_window_used: bool,
    /// The shell's side of the OS clipboard: the request
    /// the executable carries out, and the text it read back.
    pub clipboard: &'a mut pill_editor_core::clipboard::EditorClipboard,
    /// The asset tiles' previews: the textures to draw and the tiles this pass
    /// drew, so the executable asks only for what is on screen.
    pub previews: &'a mut pill_editor_core::session::PreviewStates,
    /// The entity badges the executable projected into this frame's scene
    /// views.
    pub badges: &'a [ShellBadge],
    /// The marquee the executable is holding open over a scene viewport, when
    /// it is.
    pub marquee: Option<ShellMarquee>,
    /// The states the modules published for their commands.
    pub command_states: &'a CommandStates,
    /// The draw functions the modules registered for a free-form place.
    pub command_draws: &'a CommandDraws,
    /// The asset files (relative to `res`) whose module work is unsaved.
    pub unsaved_asset_paths: &'a [String],
}

// The dock's tab methods, forwarded to the free functions below.
//
// The patch scanner refuses an `impl` header that contains `<` or `where`, and
// this trait impl's receiver has a lifetime. So no body in this block can be
// addressed, and an edit to one reloads the shell. Each method is therefore one
// call, and the body lives in the `#[pill_hot_fn]` function below it, which a
// patch crate can compile.
impl TabViewer for EditorTabViewer<'_> {
    type Tab = EditorTab;

    /// The dock needs one stable id per tab to key its widgets on.
    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(("pill_editor_shell.tab", *tab))
    }

    /// The tab's title in the dock; the work happens in [`tab_title`].
    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        tab_title(self, *tab)
    }

    /// The tab's body; the work happens in [`tab_body`].
    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        tab_body(self, ui, *tab);
    }

    /// The dock tab's menu content. After it, the dock adds its own Eject
    /// (into a window) and Close rows; a floating window's tab gets a Redock
    /// row here, which brings the surface's tabs back into the dock.
    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab, path: NodePath) {
        tab_context_menu(self, ui, *tab, path);
    }

    /// The tab button hook. The active tab's accent underline is painted here,
    /// because the dock knows each tab's rectangle but has one line colour for
    /// all of them. A right-click that opens the tab menu is logged here, once
    /// per click: the menu content itself runs every frame it is open.
    fn on_tab_button(&mut self, tab: &mut Self::Tab, response: &egui::Response) {
        tab_button(self, *tab, response);
    }

    /// The scene and game tabs keep their bodies transparent: the scene and
    /// game child windows show through them from under the overlay.
    fn clear_background(&self, tab: &Self::Tab) -> bool {
        !matches!(tab, EditorTab::Scene(_) | EditorTab::Game)
    }

    /// The scene and game tabs' bodies are viewports; they never scroll. Every
    /// other tab scrolls vertically only: it is laid out to the tab's width,
    /// squashing its fields and shortening its text rather than growing past
    /// the right edge.
    fn scroll_bars(&self, tab: &Self::Tab) -> [bool; 2] {
        match tab {
            EditorTab::Scene(_) | EditorTab::Game => [false, false],
            _ => [false, true],
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Runs one shell frame: the boundary input becomes egui input, the dock and
/// its tabs draw, and the result becomes the frame the executable draws. The
/// asset dialog, the menus, the toasts and the command palette are drawn over
/// the dock.
// Every argument is another part of the editor state the pass reads or writes.
// Grouping them would mean building a new struct on every frame.
#[allow(clippy::too_many_arguments)]
pub fn run_frame(
    state: &mut EguiShellState,
    shell_input: &EguiShellInput,
    frame: &mut EguiShellFrame,
    snapshot: &EditorSnapshot,
    selection: &mut EditorSelection,
    pending: &mut PendingEditorCommands,
    notices: &EditorNotices,
    panels: &PanelRegistry,
    panel_commands: &mut Vec<PanelCommand>,
    clipboard: &mut pill_editor_core::clipboard::EditorClipboard,
    popout_inputs: &pill_editor_core::boundary::EguiPopoutInputs,
    command_states: &CommandStates,
    command_draws: &CommandDraws,
    unsaved_work: &mut pill_editor_core::unsaved_work::UnsavedWork,
) {
    state.frame_index = state.frame_index.wrapping_add(1);
    let frame_index = state.frame_index;
    // The executable merges the theme's layers and publishes the result on the
    // snapshot. The shell installs it on the main context and on every
    // popped-out one whenever the generation number changes, so an edit to a
    // preference or to the project restyles the editor without a restart.
    if state.applied_theme_generation != snapshot.theme_generation {
        state.applied_theme_generation = snapshot.theme_generation;
        pill_editor_core::style::apply_theme(
            &state.context,
            &snapshot.theme,
            &snapshot.theme_layers,
            snapshot.theme_generation,
            &snapshot.theme_fonts,
        );
        for context in state.popout_contexts.values() {
            pill_editor_core::style::apply_theme(
                context,
                &snapshot.theme,
                &snapshot.theme_layers,
                snapshot.theme_generation,
                &snapshot.theme_fonts,
            );
        }
    }
    // The saved layout names module panel slots from the run that wrote the
    // layout file. The registry maps each saved id to today's slot, once per
    // session, so a shell reload never undoes a dock the user arranged.
    if !state.panels_remapped {
        let ids: Vec<String> = panels
            .entries()
            .iter()
            .map(|entry| entry.id.clone())
            .collect();
        state.dock.remap_panel_slots(&ids);
        state.panels_remapped = true;
    }
    // The host's notices become toasts before the dock draws, so a reload that
    // happened this frame is on screen in this frame.
    push_notices(state, notices);
    // The row heartbeat. The editor suite reads this line to prove the shell
    // sees the project's data, that the frame counter survives a reload or a
    // patch (which shows the session resource did too), and which entity the
    // snapshot's detail describes.
    if frame_index.is_multiple_of(212) {
        let detail_entity = snapshot
            .detail
            .as_ref()
            .map_or_else(|| "-".to_string(), |detail| detail.entity.to_string());
        let detail_components = snapshot
            .detail
            .as_ref()
            .map_or(0, |detail| detail.components.len());
        pill_core::info!(
            target: telemetry_target::EDITOR,
            frames = frame_index,
            entities = snapshot.entities.len(),
            systems = snapshot.systems.len(),
            paused = snapshot.paused,
            detail = detail_entity.as_str(),
            detail_components,
            "[editor] egui shell rows"
        );
    }
    // The first frame of a shell generation claims every egui context the
    // session holds, the main one and each popped-out window's. Anything the
    // previous generation left in their temporary data would be a dangling
    // read once this one draws (claim.rs explains why). This runs before
    // anything borrows the dock or the popout map.
    claim_shell_context(state);
    // The UI heartbeat: how long one full shell update took in microseconds,
    // from input conversion through the dock and its tabs to the frame
    // conversion. The shell benchmark takes its UI-redraw case from this
    // number. Drawing on the GPU happens outside it, because presenting is
    // limited by vsync.
    let ui_started = pill_core::platform::Instant::now();
    let raw_input = pill_editor_core::input::raw_input(shell_input);
    // The brand mark is uploaded once per session: the context owns the
    // texture, so a shell reload finds it already there.
    let mark = state.mark_texture();
    // The context is a cheap clone of a refcounted handle. The dock and the
    // asset dialog are separate fields, so they are borrowed apart: the dock's
    // layout borrow and the dialog's state borrow can then exist at the same
    // time.
    let context = state.context.clone();
    // What the module panels read this pass. The rows are copied only when a
    // panel is registered, so an editor with no module panels pays nothing for
    // them. The panels see this frame's rate, the same number the scene chips
    // show.
    let mut panel_feed = panel_feed(snapshot, panels);
    panel_feed.frames_per_second = shell_input.frames_per_second;
    // The window a scripted `panel window` line asked for. The panel itself
    // decides what the id means. The request stays set until that panel draws,
    // so a frame where its tab is not on screen does not drop it.
    let pending_panel_window = state.pending_panel_window.clone();
    let panel_answer =
        state
            .panel_windows
            .answer
            .as_ref()
            .map(|answer| pill_editor_core::panels::PanelAnswer {
                window: answer.window.clone(),
                accepted: answer.accepted,
            });
    // The play session's tab focus, decided before the dock draws. Entering
    // Play remembers the tab that was in front and brings the Game view
    // forward, so the person sees what the session shows; Stop puts that tab
    // back. The switch in the project's `editor_settings.yaml` can turn the
    // focus off, and then nothing moves.
    if state.last_run_mode != Some(snapshot.run_mode) {
        let entering_play = snapshot.run_mode == pill_engine::RunMode::Playing;
        state.last_run_mode = Some(snapshot.run_mode);
        if entering_play {
            if snapshot.focus_game_on_play {
                let in_front = state
                    .dock
                    .state()
                    .iter_leaves()
                    .find_map(|(_path, leaf)| leaf.tabs.get(leaf.active.0).copied());
                state.tab_before_play = in_front.filter(|tab| *tab != EditorTab::Game);
                state.requests.pending_tab = Some(EditorTab::Game);
                pill_core::info!(
                    target: telemetry_target::EDITOR,
                    tab = "game",
                    "[editor] play focused the game tab"
                );
            }
        } else if let Some(tab) = state.tab_before_play.take() {
            state.requests.pending_tab = Some(tab);
            pill_core::info!(
                target: telemetry_target::EDITOR,
                tab = tab.name(),
                "[editor] play restored the tab"
            );
        }
    }
    // The tint the bar and the dock take while a session runs: nothing while
    // editing, the theme's play colour while playing, and the stronger one
    // while paused.
    let play_tint =
        pill_editor_core::style::play_tint(&snapshot.theme, snapshot.run_mode, snapshot.paused);
    // What the pass could not do while the dock and its tabs were borrowed.
    // Each tab and menu writes its request into this value where it is raised,
    // and `settle_pass` applies it after the pass.
    let mut deferred = pill_editor_shell::shell::FrameRequests::default();
    let mut panel_window_requests: Vec<(u16, PanelCommand)> = Vec::new();
    let EguiShellState {
        dock,
        asset_dialog,
        preferences_draft,
        toolbox,
        browser,
        menubar,
        inspector,
        toasts,
        palette,
        console_filters,
        requests: shell_requests,
        stats,
        panel_budget,
        panel_windows: windows,
        pending_panel_window: window_ping,
        popout_contexts,
        previews,
        ..
    } = state;
    // The grid reports what it drew, so the executable asks for previews of
    // what is on screen only.
    previews.begin_pass();
    // The rows record their rectangles either way - the shell's own tests read
    // them - but only a scripted run asks for them in the log.
    inspector.log_rects = shell_input.scripted;
    // Only a leaf's active tab gets the accent underline, and the dock draws
    // its own line under every tab in one colour. The pass therefore reads
    // which tabs are active before the dock paints them.
    let active_tabs: Vec<EditorTab> = dock
        .state()
        .iter_leaves()
        .filter_map(|(_path, leaf)| leaf.tabs.get(leaf.active.0).copied())
        .collect();
    // The asset files with unsaved module work, which the Assets tab marks.
    let unsaved_asset_paths: Vec<String> = unsaved_work
        .items()
        .iter()
        .filter_map(|item| item.asset_path.clone())
        .collect();
    // Named through the package path rather than as a file-local item: a hot
    // patch compiles this body in a crate of its own, where only the package's
    // public items exist (the crate header has the details).
    let mut viewer = pill_editor_shell::shell::EditorTabViewer {
        snapshot,
        selection,
        asset_dialog,
        pending,
        preferences_draft,
        toolbox,
        browser,
        inspector,
        scene_rects: vec![None; dock.scene_view_count()],
        game_rect: None,
        overlay_regions: Vec::new(),
        frames_per_second: shell_input.frames_per_second,
        active_tabs: &active_tabs,
        console_filters,
        requests: shell_requests,
        stats,
        panels,
        panel_budget,
        panel_commands,
        panel_entities: panel_feed.entities.as_slice(),
        panel_detail: panel_feed.detail.as_ref(),
        panel_answer,
        panel_window_requests: &mut panel_window_requests,
        pending_panel_window: pending_panel_window
            .as_ref()
            .map(|(slot, window)| (*slot, window.as_str())),
        pending_panel_window_used: false,
        clipboard,
        deferred: &mut deferred,
        previews,
        badges: &shell_input.badges,
        marquee: shell_input.marquee,
        command_states,
        command_draws,
        unsaved_asset_paths: &unsaved_asset_paths,
    };
    // The whole egui pass - layout, widgets and the dock - as one Tracy zone.
    // It closes before the settle stage below, so the span is the pass itself.
    let output = {
        let _zone = pill_core::profile_scope!("egui pass");
        context.run_ui(raw_input, |ui| {
            // The tabs and badges a pass draws are the ones their rectangle caches
            // keep for this frame. Opening the window here means a row that left
            // the list stops occupying the session.
            viewer.inspector.begin_geometry_pass();
            // The menu strip comes first, so the dock and every viewport rectangle
            // it reports start below it.
            menu_bar(ui, menubar, &mark, &mut viewer);
            // Tab bars carry the tabs and their close buttons only. With no
            // collapse chevron and no panel-level close-all button, a panel closes
            // exactly when its last tab closes.
            DockArea::new(dock.state_mut())
                .style({
                    let mut dock_style =
                        style::dock_style(ui.ctx().global_style().as_ref(), &style::palette());
                    // While a session runs the tab strips carry the play tint, so
                    // the whole frame reads as the game's.
                    if let Some(tint) = play_tint {
                        dock_style.tab_bar.bg_fill = tint;
                    }
                    dock_style
                })
                .show_leaf_collapse_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_inside(ui, &mut viewer);
            // The create/rename dialog floats over the dock while it is open.
            pill_editor_shell::shell::asset_dialog_window(
                ui.ctx(),
                viewer.asset_dialog,
                viewer.pending,
                viewer.snapshot,
                &mut viewer.overlay_regions,
            );
            pill_editor_shell::shell::about_window(
                ui.ctx(),
                &mark,
                &mut menubar.about_open,
                &mut viewer.overlay_regions,
            );
            // The toast stack floats over the dock and every viewport it covers,
            // so its rows join the frame's overlay regions.
            pill_editor_shell::shell::draw_toasts(
                toasts,
                frame_index,
                ui.ctx(),
                &mut viewer.overlay_regions,
            );
            // The command palette draws last so it sits above every other surface:
            // it is driven from the keyboard, not from the pointer.
            pill_editor_shell::shell::palette_window(ui.ctx(), palette, menubar, &mut viewer);
            // The windows the module panels asked for, applied before the windows
            // pass draws them, so one opened this frame is on screen this frame.
            for (slot, request) in viewer.panel_window_requests.drain(..) {
                let source = panels
                    .get(slot)
                    .map(|entry| entry.source.clone())
                    .unwrap_or_default();
                match request {
                    PanelCommand::OpenWindow(request) => {
                        pill_core::info!(
                            target: telemetry_target::EDITOR,
                            slot,
                            window = request.id.as_str(),
                            modal = request.modal,
                            "[editor] panel window opened"
                        );
                        windows.open(&source, slot, &request)
                    }
                    PanelCommand::CloseWindow(id) => {
                        pill_core::info!(
                            target: telemetry_target::EDITOR,
                            slot,
                            window = id.as_str(),
                            "[editor] panel window closed"
                        );
                        windows.close(&source, &id)
                    }
                    _ => {}
                }
            }
            pill_editor_shell::shell::panel_windows(
                ui.ctx(),
                windows,
                panels,
                &mut *viewer.panel_budget,
                &panel_feed,
                &mut *viewer.panel_commands,
                &mut viewer.overlay_regions,
            );
            // The quit prompt sits above everything while the executable holds
            // a quit back for unsaved work.
            pill_editor_shell::shell::quit_prompt_window(ui.ctx(), unsaved_work);
        })
    };
    let mut shell_frame = frame_from_output(&context, output);
    shell_frame.scene_rects = std::mem::take(&mut viewer.scene_rects);
    shell_frame.game_rect = viewer.game_rect.take();
    shell_frame.overlay_regions = std::mem::take(&mut viewer.overlay_regions);
    // The pass's deferred requests live in `viewer.deferred`. Each tab and menu
    // wrote its request where it was raised, and the settle stage applies them
    // in the order the pass needs (its module docs say why each request sits
    // where it does). Passing the viewer whole means the value needs no second
    // argument and cannot disagree with itself.
    //
    // A pass can replace a frame the executable never drew: a reload runs a
    // pass while the previous frame is still queued. epaint panics when a
    // texture update is dropped unapplied, so the queued frame's updates go
    // before the assignment drops it.
    frame.discard_unapplied_textures();
    *frame = pill_editor_shell::shell::settle_pass(
        dock,
        popout_contexts,
        popout_inputs,
        windows,
        window_ping,
        toasts,
        panels,
        frame_index,
        &mut viewer,
        shell_frame,
    );
    let ui_micros = ui_started.elapsed().as_micros() as u64;
    // The Statistics tab's top row: the time this frame's pass actually took.
    // It is recorded before the frame's own log line below, so the tab and the
    // log never disagree.
    stats.record_shell(ui_micros);
    stats.frames_per_second = shell_input.frames_per_second;
    // The periodic heartbeat, plus any update slower than `SLOW_UPDATE_MICROS`.
    // The suites read it to prove the counter advances, and the benchmark
    // still gets samples when 210 frames take longer than its measurement
    // window.
    if frame_index.is_multiple_of(210) || ui_micros >= SLOW_UPDATE_MICROS {
        pill_core::debug!(
            target: telemetry_target::EDITOR,
            frames = frame_index,
            ui_us = ui_micros,
            "[editor] egui frame"
        );
    }
}

/// Runs one egui pass for each popped-out tab.
///
/// Every popped-out window has its own context, so it draws on its own, after
/// the dock pass in the main window. The data and the viewer code are the same;
/// the widget state and the input record are the window's own. The tab body is
/// the same `TabViewer::ui` implementation the dock calls, which is what keeps
/// a tab looking and behaving the same in a window and in the dock.
///
/// `dock.popouts()` holds the tabs that are *not* in the dock tree, so nothing
/// is drawn twice. The viewer's viewport rectangles are reset for each pass: a
/// popped-out Scene tab reports its rectangle in its own window's coordinates,
/// and the frontend places that view's surface there.
pub fn run_popout_passes(
    dock: &mut pill_editor_core::layout::EditorDock,
    contexts: &mut std::collections::BTreeMap<EditorTab, egui::Context>,
    inputs: &pill_editor_core::boundary::EguiPopoutInputs,
    viewer: &mut EditorTabViewer<'_>,
) -> Vec<pill_editor_core::boundary::PopoutFrame> {
    let mut frames = Vec::new();
    // Redock requests are applied after the loop: the dock cannot be borrowed
    // mutably while it is read for each pass's tab list.
    let mut redock = Vec::new();
    for tab in dock.popouts().to_vec() {
        let name = tab.name();
        let context = contexts
            .entry(tab)
            .or_insert_with(|| {
                let context = egui::Context::default();
                pill_editor_core::style::apply(&context);
                context
            })
            .clone();
        // A window the frontend has not reported yet gets a default record:
        // an empty event list, so the pass draws without interaction.
        let input = inputs.by_tab.get(&name).cloned().unwrap_or_default();
        let raw_input = pill_editor_core::input::raw_input(&input);
        viewer.scene_rects = vec![None; dock.scene_view_count()];
        let mut output = context.run_ui(raw_input, |ui| {
            // The window has no dock around it, so it draws the one control
            // the dock would give it: the Redock button.
            let mut send_back = false;
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(tab.title()).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    send_back = ui
                        .add(icons::IconButton::new(icons::Icon::ArrowSquareIn, "Redock"))
                        .clicked();
                });
            });
            ui.separator();
            if send_back {
                redock.push(tab);
            }
            let mut popout_tab = tab;
            <EditorTabViewer<'_> as TabViewer>::ui(viewer, ui, &mut popout_tab);
        });
        if redock.contains(&tab) {
            // Its window closes the moment the dock stops listing it, so no
            // frame is built for this pass. Its texture updates are never
            // applied, and epaint panics when a delta is dropped unapplied.
            output.textures_delta.clear();
            continue;
        }
        let mut frame = pill_editor_core::boundary::frame_from_output(&context, output);
        frame.scene_rects = std::mem::take(&mut viewer.scene_rects);
        frame.overlay_regions = std::mem::take(&mut viewer.overlay_regions);
        frames.push(pill_editor_core::boundary::PopoutFrame {
            tab: name,
            title: format!("Pill Editor - {}", tab.title()),
            frame: Box::new(frame),
        });
    }
    for tab in redock {
        if dock.redock(tab) {
            pill_core::info!(
                target: telemetry_target::EDITOR,
                tab = tab.name().as_str(),
                "[editor] popout returned"
            );
        }
    }
    frames
}

/// What module panels read this pass.
///
/// The rows are copied only when a panel is registered: they duplicate what
/// the tabs already read, and an editor with no module panels should not pay
/// for them.
pub fn panel_feed(snapshot: &EditorSnapshot, panels: &PanelRegistry) -> PanelFeed {
    let mut feed = PanelFeed {
        entities: Vec::new(),
        detail: None,
        frames_per_second: 0.0,
    };
    if panels.is_empty() {
        return feed;
    }
    feed.entities = snapshot
        .entities
        .iter()
        .map(|entry| PanelEntityRow {
            entity: entry.entity,
            display: entry.display.clone(),
            archetype: entry.archetype_label.clone(),
        })
        .collect();
    feed.detail = snapshot.detail.as_ref().map(|detail| PanelEntityDetail {
        entity: detail.entity,
        components: detail
            .components
            .iter()
            .map(|component| PanelComponentValues {
                component: component.type_name.clone(),
                fields: component
                    .fields
                    .iter()
                    .zip(component.values.iter())
                    .map(|(field, value)| PanelFieldValue {
                        name: field.name.to_string(),
                        value: inspector::value_text(value),
                    })
                    .collect(),
            })
            .collect(),
    });
    feed
}

/// Returns one tab's dock title.
///
/// A module panel's title comes from the registry, so it changes when the
/// module unloads. A panel that went over its frame budget gets a `(slow)` mark
/// that no other tab carries.
#[pill_hot_fn]
pub fn tab_title(viewer: &mut EditorTabViewer<'_>, tab: EditorTab) -> egui::WidgetText {
    if let Some(slot) = tab.module_slot() {
        let title = viewer.panels.tab_title(slot);
        let label = if viewer.panel_budget.is_slow(slot) {
            format!("{} (slow)", title)
        } else {
            title
        };
        return icons::rich(tab_icon(tab), label.as_str()).into();
    }
    icons::rich(tab_icon(tab), tab.title().as_str()).into()
}

/// Draws one active tab's body and records how long it took.
///
/// Every tab is timed here because the Statistics tab exists to show
/// measurements, not estimates.
#[pill_hot_fn]
pub fn tab_body(viewer: &mut EditorTabViewer<'_>, ui: &mut egui::Ui, tab: EditorTab) {
    // One Tracy zone per tab, named for it: the same closure the Statistics tab
    // times, on the profiler's timeline. The name formats only when a profiler
    // is connected.
    let _zone = pill_core::profile_scope!("tab: {}", tab.name());
    let started = pill_core::platform::Instant::now();
    // A tab has no horizontal scrolling, so text too long for the room left
    // ends in an ellipsis instead of running past the tab's edge.
    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
    match tab {
        EditorTab::Scene(view) => {
            let view = view as usize;
            if let Some(slot) = viewer.scene_rects.get_mut(view) {
                *slot = Some(scene_body_rect(ui));
            }
            scene_context_menu(viewer, ui);
            scene_overlay(ui, viewer.frames_per_second, &mut viewer.overlay_regions);
            toolbox_overlay(viewer, ui, view);
            scene_badges(viewer, ui, view);
            scene_marquee(ui, viewer.marquee.as_ref(), view);
        }
        EditorTab::Game => {
            // The game viewport is the primary surface's child window: a hole,
            // measured like a scene tab, with no menu and no overlay chips - it
            // is the project's own view.
            viewer.game_rect = Some(scene_body_rect(ui));
        }
        EditorTab::Entities => entities_tab(
            ui,
            viewer.snapshot,
            viewer.selection,
            viewer.pending,
            &mut viewer.overlay_regions,
            viewer.inspector,
            &mut PlacementHost {
                commands: &viewer.snapshot.commands,
                states: viewer.command_states,
                draws: viewer.command_draws,
                invocations: &mut viewer.requests.invocations,
            },
        ),
        EditorTab::Systems => systems_tab(ui, viewer.snapshot, viewer.pending),
        EditorTab::Inspector => inspector_tab(
            ui,
            viewer.snapshot,
            &*viewer.selection,
            viewer.pending,
            &mut viewer.overlay_regions,
            viewer.inspector,
            &mut PlacementHost {
                commands: &viewer.snapshot.commands,
                states: viewer.command_states,
                draws: viewer.command_draws,
                invocations: &mut viewer.requests.invocations,
            },
        ),
        EditorTab::Assets => {
            // One bundle of the handles the Assets tab writes through, the same
            // shape a grid tile gets (see `AssetTileWrites`).
            let mut writes = AssetTileWrites {
                selection: viewer.selection,
                dialog: viewer.asset_dialog,
                pending: viewer.pending,
                overlay_regions: &mut viewer.overlay_regions,
                drag_out: &mut viewer.deferred.drag_out,
                previews: viewer.previews,
                unsaved_paths: viewer.unsaved_asset_paths,
                placements: &mut PlacementHost {
                    commands: &viewer.snapshot.commands,
                    states: viewer.command_states,
                    draws: viewer.command_draws,
                    invocations: &mut viewer.requests.invocations,
                },
            };
            assets_tab(ui, viewer.snapshot, viewer.browser, &mut writes);
        }
        EditorTab::Preferences => preferences_tab(
            ui,
            viewer.snapshot,
            viewer.preferences_draft,
            viewer.pending,
        ),
        EditorTab::Console => console_tab(ui, viewer.snapshot, viewer.console_filters),
        EditorTab::Problems => problems_tab(ui, viewer.snapshot, &mut viewer.requests.problem_open),
        EditorTab::Statistics => {
            statistics_tab(ui, viewer.snapshot, viewer.stats, viewer.frames_per_second)
        }
        EditorTab::Module(slot) => draw_module_panel(viewer, ui, slot),
    }
    viewer
        .stats
        .record(tab, started.elapsed().as_micros() as f64);
}

/// Runs one module panel's pass: check that its module is alive, draw the
/// panel, collect the window requests it made, and charge its draw time to the
/// panel budget.
///
/// The panel belongs to a module. Its draw function runs only while that
/// module's liveness marker is alive; when the marker is gone the tab says so
/// instead.
#[pill_hot_fn]
pub fn draw_module_panel(viewer: &mut EditorTabViewer<'_>, ui: &mut egui::Ui, slot: u16) {
    let Some(draw) = viewer.panels.draw_of(slot) else {
        ui.vertical_centered(|ui| {
            ui.add_space(12.0);
            ui.strong("Module not loaded");
            ui.label(
                viewer
                    .panels
                    .get(slot)
                    .map(|entry| entry.source.clone())
                    .unwrap_or_else(|| "unknown panel".to_string()),
            );
        });
        return;
    };

    let requested_window = viewer
        .pending_panel_window
        .filter(|(pinged, _)| *pinged == slot)
        .map(|(_, window)| window);
    if requested_window.is_some() {
        viewer.pending_panel_window_used = true;
    }
    let mut context = PanelContext {
        frames_per_second: viewer.frames_per_second,
        selected_entity: viewer.selection.primary(),
        entities: viewer.panel_entities,
        detail: viewer.panel_detail,
        window: None,
        requested_window,
        answer: viewer.panel_answer.as_ref(),
        commands: viewer.panel_commands,
    };
    // Timed here rather than reusing `tab_body`'s clock: the panel budget is a
    // per-slot measurement over its own draw, not the tab's total.
    let started = pill_core::platform::Instant::now();
    draw(ui, &mut context);
    let micros = started.elapsed().as_micros() as f32;
    // The window requests belong to this panel, and only this function knows
    // which slot asked.
    let mut kept = Vec::with_capacity(context.commands.len());
    for command in context.commands.drain(..) {
        match command {
            PanelCommand::OpenWindow(_) | PanelCommand::CloseWindow(_) => {
                viewer.panel_window_requests.push((slot, command))
            }
            other => kept.push(other),
        }
    }
    *context.commands = kept;
    let over_budget = viewer.panel_budget.record(slot, micros);
    if over_budget {
        pill_core::warn!(
            target: telemetry_target::EDITOR,
            slot,
            micros,
            budget = PANEL_BUDGET_MICROS,
            frames = PANEL_BUDGET_FRAMES,
            "[editor] module panel over budget"
        );
    }
}

/// The dock tab's menu rows: Pop Out wherever a tab is, Redock on a floating
/// surface.
#[pill_hot_fn]
pub fn tab_context_menu(
    viewer: &mut EditorTabViewer<'_>,
    ui: &mut egui::Ui,
    tab: EditorTab,
    path: NodePath,
) {
    // Popping a tab out is offered wherever a tab is, docked or floating; the
    // window carries the way back.
    if ui
        .add(icons::IconButton::new(
            icons::Icon::ArrowSquareOut,
            "Pop Out",
        ))
        .clicked()
    {
        viewer.deferred.tab_popout = Some(tab);
        ui.close();
    }
    if !path.surface.is_main()
        && ui
            .add(icons::IconButton::new(icons::Icon::ArrowSquareIn, "Redock"))
            .clicked()
    {
        viewer.deferred.tab_redock = Some(path.surface);
        ui.close();
    }
    // This menu's frame belongs to the dock, so only the content drawn here is
    // reported. With its rectangle in the overlay regions, a click on a menu
    // item over the viewport never becomes a scene pick.
    let rect = ui.min_rect();
    viewer.overlay_regions.push(ShellRect {
        x: rect.min.x,
        y: rect.min.y,
        width: rect.width(),
        height: rect.height(),
    });
}

/// The tab button hook: the active tab's accent underline, and the tab menu's
/// once-per-click log.
#[pill_hot_fn]
pub fn tab_button(viewer: &mut EditorTabViewer<'_>, tab: EditorTab, response: &egui::Response) {
    if viewer.active_tabs.contains(&tab) {
        paint_active_tab_underline(response);
    }
    if response.secondary_clicked() {
        log_context_menu("tab");
    }
}
