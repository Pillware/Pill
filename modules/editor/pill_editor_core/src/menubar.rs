//! The editor's menu bar: the strip above the dock, the mark on its left,
//! the menu titles and the rows each menu shows.
//!
//! # Responsibilities
//!
//! - The bar's vocabulary: the menu titles in their order, and the metrics
//!   the shell and the tests share (the bar's height, the mark's size, and
//!   the gaps before and after the mark).
//! - The mark's rectangle inside a bar of a given height ([`mark_rect`]).
//! - The menus' contents as plain data: one row per registered command whose
//!   menu path names the menu, and the action a click asks for
//!   ([`MenuAction`]).
//!
//! Nothing here draws. The shell walks [`Menu::ALL`] and egui lays the
//! buttons out, so the bar follows the theme and the font metrics.

// External crates
use egui::{pos2, Rect, Vec2};
use pill_engine::RunMode;

// Current crate
use crate::assets;
use crate::commands::CommandView;
use crate::editor_state::{EditorCommand, EditorSelection, EditorSnapshot};
use crate::entities;
use crate::layout::EditorTab;
use crate::systems;
use crate::toolbox::{ToolboxItem, ToolboxState};

// =============================================================================
// Constants
// =============================================================================

/// The bar's height in points, tall enough for one row of menu buttons and
/// the mark.
pub const MENU_BAR_HEIGHT: f32 = 30.0;

/// The mark's side length in points.
pub const MENU_MARK_SIDE: f32 = 18.0;

/// The gap between the bar's left edge and the mark.
pub const MENU_MARK_LEFT: f32 = 10.0;

/// The gap between the mark and the first title.
pub const MENU_MARK_GAP: f32 = 8.0;

/// How many titles the bar carries.
pub const MENU_TITLE_COUNT: usize = Menu::ALL.len();

// =============================================================================
// Menus
// =============================================================================

/// One menu of the bar. The variants are in the order the titles are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Menu {
    /// The scene rows, the `.meta` sidecar write, the Preferences tab and
    /// quitting the editor.
    File,
    /// History, the clipboard and entity deletion.
    Edit,
    /// A new entity, a new scene and a new asset.
    Create,
    /// The gizmo's tool, space and snapping, the scene grid and the
    /// simulation controls.
    View,
    /// The dock's tabs and one more scene view.
    Window,
    /// The About window.
    Help,
}

impl Menu {
    /// Every menu, in the order the titles follow the mark.
    pub const ALL: [Menu; 6] = [
        Menu::File,
        Menu::Edit,
        Menu::Create,
        Menu::View,
        Menu::Window,
        Menu::Help,
    ];

    /// Returns the label the bar draws for this menu.
    pub fn title(self) -> &'static str {
        match self {
            Menu::File => "File",
            Menu::Edit => "Edit",
            Menu::Create => "Create",
            Menu::View => "View",
            Menu::Window => "Window",
            Menu::Help => "Help",
        }
    }

    /// Returns the wire name the shell logs the menu under (`file`, `edit`,
    /// ...).
    pub fn name(self) -> &'static str {
        match self {
            Menu::File => "file",
            Menu::Edit => "edit",
            Menu::Create => "create",
            Menu::View => "view",
            Menu::Window => "window",
            Menu::Help => "help",
        }
    }
}

// =============================================================================
// Menu Model
// =============================================================================

/// The values the menus are built from: the frame's snapshot, the selection
/// and the registry's command rows.
///
/// The menu model reads nothing else, and never the world, so the shell and
/// the tests build the same rows from the same values.
pub struct MenuView<'a> {
    /// The engine's captured state: the gizmo settings, the grid, the asset
    /// rows and the history's depths.
    pub snapshot: &'a EditorSnapshot,
    /// The shared selection the enablement rules read.
    pub selection: &'a EditorSelection,
    /// The registered commands in registration order. A row exists when one
    /// of these names this menu and the model has an action for it.
    pub commands: &'a [CommandView],
}

/// One row of a menu: a separator or an item a click can act on.
pub enum MenuEntry {
    /// A horizontal rule between groups.
    Separator,
    /// An item the reader can click.
    Action {
        /// The row's label. It is the command's title, except on a toolbox
        /// row, whose label shows the current state.
        label: String,
        /// What a click asks the editor to do.
        action: MenuAction,
        /// Whether the item can act. A disabled row is drawn greyed out.
        enabled: bool,
        /// `Some(on)` marks a toggle or radio row, which the theme
        /// highlights. `None` is a plain item.
        checked: Option<bool>,
    },
}

/// What a menu item asks the editor to do, as plain data.
///
/// The shell turns an action into the same calls the tabs, the toolbox and
/// the scripted channel make. Nothing here touches the world.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    /// Nothing. The row is disabled, so its action does not apply.
    Inert,
    /// Queue a recorded step: the label it records under and the commands it
    /// carries.
    Queue {
        /// The history label the step records under.
        label: String,
        /// The commands the step carries.
        commands: Vec<EditorCommand>,
    },
    /// Open the create-asset dialog.
    CreateAsset,
    /// Open the same dialog to create a scene, with the scene type chosen.
    CreateScene,
    /// Open the same dialog to write the current world as a scene.
    SaveSceneAs,
    /// Write the selected asset's missing `.meta` sidecar.
    WriteAssetMeta,
    /// Undo the newest recorded step.
    Undo,
    /// Redo the newest undone step.
    Redo,
    /// Bring a dock tab to the front.
    ActivateTab(EditorTab),
    /// Dock one more scene view, with its own camera and toolbox.
    NewSceneView,
    /// Start a play session, or stop the one that is running. The toolbar's
    /// first button and the View menu's Play row both carry it.
    TogglePlay,
    /// Open the About window.
    About,
    /// Quit the editor.
    Quit,
    /// Copy, cut or paste. The shell queues the request, and the executable's
    /// OS clipboard bridge carries it out.
    Clipboard(crate::clipboard::ClipboardAction),
}

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the mark's rectangle in a bar `bar_height` points tall. The
/// rectangle is a square, inset by [`MENU_MARK_LEFT`] on the left and
/// centred vertically.
pub fn mark_rect(bar_height: f32) -> Rect {
    let top = ((bar_height - MENU_MARK_SIDE) * 0.5).max(0.0);
    Rect::from_min_size(pos2(MENU_MARK_LEFT, top), Vec2::splat(MENU_MARK_SIDE))
}

/// Returns the entries of `menu`: one row per registered command whose menu
/// path starts with this menu's title, in registration order.
///
/// No row is hard-coded here. A command that stops being registered loses its
/// row, and a renamed command renames its row. The rules between groups come
/// from the specs marked with
/// [`crate::commands::CommandSpec::separator`].
///
/// A command with no action here gets no row. A greyed-out item would say
/// "not available right now", which is false for a command nothing
/// implements.
pub fn entries(menu: Menu, view: &MenuView<'_>) -> Vec<MenuEntry> {
    let mut menu_entries = Vec::new();
    for command in view.commands {
        if command.menu_path.first().map(String::as_str) != Some(menu.title()) {
            continue;
        }
        let Some(menu_entry) = menu_entry_for_command(command, view) else {
            continue;
        };
        if command.separator_before && !menu_entries.is_empty() {
            menu_entries.push(MenuEntry::Separator);
        }
        menu_entries.push(menu_entry);
    }
    menu_entries
}

/// Returns what running one registered command asks the editor to do.
///
/// The palette calls this instead of building its own menu row, so a command
/// run from the palette and the same command run from its menu entry reach one
/// action. `None` means this build registers the command but has no action for
/// it. The palette then drops the row rather than offering a no-op.
pub fn action_for(
    command: &CommandView,
    snapshot: &EditorSnapshot,
    selection: &EditorSelection,
) -> Option<MenuAction> {
    let view = MenuView {
        snapshot,
        selection,
        commands: std::slice::from_ref(command),
    };
    match menu_entry_for_command(command, &view)? {
        MenuEntry::Action { action, .. } => Some(action),
        MenuEntry::Separator => None,
    }
}

/// Returns the `.scene` asset the Assets tab has selected, as the path Open
/// Scene loads. `None` means the selection is not a scene file.
fn selected_scene_path(selection: &EditorSelection) -> Option<String> {
    selection
        .asset
        .as_deref()
        .filter(|path| path.to_ascii_lowercase().ends_with(".scene"))
        .map(str::to_string)
}

/// Returns one row for one registered command: its label, and what a click
/// asks the editor to do.
///
/// The label is the command's title, except where the row shows the current
/// state (the gizmo's space). Returns `None` when the command's id names no
/// action here and no dock tab. That is a gap in this model, not a disabled
/// row, and [`entries`] and [`action_for`] both treat it that way.
fn menu_entry_for_command(command: &CommandView, view: &MenuView<'_>) -> Option<MenuEntry> {
    let toolbox_state = ToolboxState {
        gizmo: view.snapshot.gizmo,
        grid: view.snapshot.grid_enabled,
        gizmos: view.snapshot.gizmos_enabled,
        frame: view.snapshot.gizmo_frame.as_deref(),
    };
    let mut enabled = true;
    let mut checked = None;
    let mut label = command.title.clone();
    let action = match command.id.as_str() {
        "editor.undo" => {
            enabled = view.snapshot.undo_depth > 0;
            MenuAction::Undo
        }
        "editor.redo" => {
            enabled = view.snapshot.redo_depth > 0;
            MenuAction::Redo
        }
        "editor.entity_delete" => match view.selection.primary() {
            Some(entity) => {
                let (commands, label) =
                    entities::entity_delete(entity, view.selection.asset.clone());
                MenuAction::Queue { label, commands }
            }
            None => {
                enabled = false;
                MenuAction::Inert
            }
        },
        "editor.entity_create" => {
            let (step, label) = entities::entity_create();
            MenuAction::Queue {
                label,
                commands: vec![step],
            }
        }
        // The clipboard rows. Paste is always offered, because only the
        // executable can tell whether the clipboard holds a payload; a refusal
        // there shows a toast instead of a greyed row nobody can explain. Copy
        // and Cut ask the same question the copy itself asks, so an offered
        // row always has something to carry.
        "editor.copy" | "editor.cut" => {
            let action = if command.id == "editor.copy" {
                crate::clipboard::ClipboardAction::Copy
            } else {
                crate::clipboard::ClipboardAction::Cut
            };
            enabled = crate::clipboard::can_copy(view.selection, view.snapshot);
            MenuAction::Clipboard(action)
        }
        "editor.paste" => MenuAction::Clipboard(crate::clipboard::ClipboardAction::Paste),
        // The scene document's rows. Open acts on the scene selected in the
        // Assets tab, so it greys out until one is selected. Save needs an open
        // scene file. Save As asks for a name and a folder in the create
        // dialog, the same window an asset is created in.
        "editor.scene_new" => MenuAction::CreateScene,
        "editor.scene_open" => match selected_scene_path(view.selection) {
            Some(path) => MenuAction::Queue {
                label: format!("open scene {path}"),
                commands: vec![EditorCommand::SceneOpen { path }],
            },
            None => {
                enabled = false;
                MenuAction::Inert
            }
        },
        "editor.scene_save" => {
            enabled = view.snapshot.scene_open;
            MenuAction::Queue {
                label: "save scene".to_string(),
                commands: vec![EditorCommand::SceneSave],
            }
        }
        "editor.scene_save_as" => MenuAction::SaveSceneAs,
        "editor.asset.create" => MenuAction::CreateAsset,
        "editor.asset_write_meta" => {
            enabled = assets::can_write_meta(view.selection, &view.snapshot.assets);
            MenuAction::WriteAssetMeta
        }
        "editor.preferences_open" => MenuAction::ActivateTab(EditorTab::Preferences),
        "editor.scene_new_view" => MenuAction::NewSceneView,
        "editor.quit" => MenuAction::Quit,
        "editor.about" => MenuAction::About,
        "editor.tool_translate" => {
            let menu_row = menu_row_for_toolbox_item(ToolboxItem::Translate, &toolbox_state);
            label = menu_row.0;
            checked = menu_row.2;
            menu_row.1
        }
        "editor.tool_rotate" => {
            let menu_row = menu_row_for_toolbox_item(ToolboxItem::Rotate, &toolbox_state);
            label = menu_row.0;
            checked = menu_row.2;
            menu_row.1
        }
        "editor.tool_scale" => {
            let menu_row = menu_row_for_toolbox_item(ToolboxItem::Scale, &toolbox_state);
            label = menu_row.0;
            checked = menu_row.2;
            menu_row.1
        }
        "editor.tool_space_toggle" => {
            let menu_row = menu_row_for_toolbox_item(ToolboxItem::Space, &toolbox_state);
            label = menu_row.0;
            checked = menu_row.2;
            menu_row.1
        }
        "editor.tool_snap_toggle" => {
            let menu_row = menu_row_for_toolbox_item(ToolboxItem::Snap, &toolbox_state);
            label = menu_row.0;
            checked = menu_row.2;
            menu_row.1
        }
        "editor.grid_toggle" => {
            let menu_row = menu_row_for_toolbox_item(ToolboxItem::Grid, &toolbox_state);
            label = menu_row.0;
            checked = menu_row.2;
            menu_row.1
        }
        // The play row shows the action a press takes, so it reads `Play`
        // while the editor is being edited and `Stop` while a game runs.
        "editor.play_toggle" => {
            label = play_title(view.snapshot.run_mode).to_string();
            MenuAction::TogglePlay
        }
        "editor.pause_toggle" => {
            let paused = view.snapshot.paused;
            MenuAction::Queue {
                label: "pause toggle".to_string(),
                commands: vec![systems::pause_toggle_command(paused)],
            }
        }
        "editor.step" => MenuAction::Queue {
            label: "step one frame".to_string(),
            commands: vec![EditorCommand::StepOnce],
        },
        // The Window menu's rows: one per tab, found by the name in
        // `editor.window_<name>`.
        id => match id
            .strip_prefix("editor.window_")
            .and_then(|name| EditorTab::ALL.into_iter().find(|tab| tab.name() == name))
        {
            Some(tab) => MenuAction::ActivateTab(tab),
            None => return None,
        },
    };
    Some(MenuEntry::Action {
        label,
        action,
        enabled,
        checked,
    })
}

/// The play control's label for a run mode: what pressing it does.
///
/// The toolbar's first button and the View menu's row share it, so both read
/// `Play` while the editor is being edited and `Stop` while a game runs.
pub fn play_title(run_mode: RunMode) -> &'static str {
    match run_mode {
        RunMode::Editing => "Play",
        RunMode::Playing => "Stop",
    }
}

/// The command the play control queues: start a session or stop the one that
/// runs. Neither is recorded, so a session never becomes an undo step.
pub fn play_toggle_command(run_mode: RunMode) -> EditorCommand {
    match run_mode {
        RunMode::Editing => EditorCommand::EnterPlay,
        RunMode::Playing => EditorCommand::ExitPlay,
    }
}

/// Returns a toolbox row as menu data: the label it shows for the current
/// state, the recorded command a click queues, and whether it reads as active.
fn menu_row_for_toolbox_item(
    item: ToolboxItem,
    toolbox_state: &ToolboxState,
) -> (String, MenuAction, Option<bool>) {
    let label = item.label(toolbox_state).to_string();
    let action = MenuAction::Queue {
        label: label.clone(),
        commands: vec![item.command(toolbox_state)],
    };
    (label, action, Some(item.active(toolbox_state)))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_titles_keep_unity_order_after_the_mark() {
        let titles: Vec<&str> = Menu::ALL.iter().map(|menu| menu.title()).collect();
        assert_eq!(
            titles,
            vec!["File", "Edit", "Create", "View", "Window", "Help"]
        );
    }

    #[test]
    fn titles_and_wire_names_are_distinct() {
        for menu in Menu::ALL {
            assert!(!menu.title().is_empty());
            assert_eq!(menu.name(), menu.title().to_lowercase());
        }
    }

    #[test]
    fn the_mark_sits_left_and_centres_in_the_bar() {
        let rect = mark_rect(MENU_BAR_HEIGHT);
        assert_eq!(rect.width(), MENU_MARK_SIDE);
        assert_eq!(rect.height(), MENU_MARK_SIDE);
        assert_eq!(rect.min.x, MENU_MARK_LEFT);
        let bar_center = MENU_BAR_HEIGHT * 0.5;
        assert!((rect.center().y - bar_center).abs() < 0.01);
    }

    #[test]
    fn a_bar_shorter_than_the_mark_keeps_the_mark_inside() {
        let rect = mark_rect(10.0);
        assert_eq!(rect.min.y, 0.0);
    }

    /// Returns the frame's view over one snapshot, selection and command list.
    fn view_over<'a>(
        snapshot: &'a EditorSnapshot,
        selection: &'a EditorSelection,
        commands: &'a [CommandView],
    ) -> MenuView<'a> {
        MenuView {
            snapshot,
            selection,
            commands,
        }
    }

    /// Returns the registered commands, as the frame's snapshot carries them.
    fn commands_fixture() -> Vec<CommandView> {
        crate::commands::CommandRegistry::with_builtins().views()
    }

    /// Returns the label, the enabled flag and the check state of every action
    /// row.
    fn action_states(menu_entries: &[MenuEntry]) -> Vec<(String, bool, Option<bool>)> {
        menu_entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Separator => None,
                MenuEntry::Action {
                    label,
                    enabled,
                    checked,
                    ..
                } => Some((label.clone(), *enabled, *checked)),
            })
            .collect()
    }

    /// Returns the labels only, so a test can compare a menu's rows by text.
    fn labels(menu_entries: &[MenuEntry]) -> Vec<String> {
        action_states(menu_entries)
            .into_iter()
            .map(|row| row.0)
            .collect()
    }

    fn separator_count(menu_entries: &[MenuEntry]) -> usize {
        menu_entries
            .iter()
            .filter(|entry| matches!(entry, MenuEntry::Separator))
            .count()
    }

    #[test]
    fn the_file_menu_offers_the_scene_rows_the_preferences_tab_and_quitting() {
        let commands = commands_fixture();
        let snapshot = snapshot_fixture();
        let selection = EditorSelection::default();
        let menu_entries = entries(Menu::File, &view_over(&snapshot, &selection, &commands));
        // Three rules split the rows into four groups: Open and Save, Save As
        // with the sidecar write, the Preferences row and Quit.
        assert_eq!(
            labels(&menu_entries),
            vec![
                "Open Scene",
                "Save Scene",
                "Save Scene As",
                "Write Asset Meta",
                "Editor preferences",
                "Quit Editor",
            ]
        );
        // With nothing selected, the sidecar write is greyed out.
        assert!(!action_states(&menu_entries)[3].1);
        assert!(
            action_states(&menu_entries)[4].1,
            "the preferences tab always opens"
        );
        assert_eq!(separator_count(&menu_entries), 3);
        assert!(matches!(menu_entries[2], MenuEntry::Separator));
        assert!(matches!(menu_entries[5], MenuEntry::Separator));
        assert!(matches!(menu_entries[7], MenuEntry::Separator));
    }

    #[test]
    fn the_edit_menu_follows_the_history_depths_and_the_selection() {
        let commands = commands_fixture();
        let mut snapshot = snapshot_fixture();
        let mut selection = EditorSelection::default();
        let disabled_entries = entries(Menu::Edit, &view_over(&snapshot, &selection, &commands));
        assert_eq!(
            labels(&disabled_entries),
            vec!["Undo", "Redo", "Copy", "Cut", "Paste", "Delete Entity"]
        );
        let action_rows = action_states(&disabled_entries);
        assert!(!action_rows[0].1, "no undo history yet");
        assert!(!action_rows[1].1, "no redo history yet");
        assert!(!action_rows[5].1, "nothing selected to delete");
        // Copy and Cut need something to carry. Paste is always offered,
        // because only the executable can tell whether the clipboard holds a
        // payload.
        assert_eq!((action_rows[2].1, action_rows[3].1), (false, false));
        assert!(action_rows[4].1, "paste is offered with nothing selected");

        snapshot.undo_depth = 2;
        snapshot.redo_depth = 1;
        let mut engine = pill_engine::Engine::new();
        let entity = engine
            .world_mut()
            .create_entity()
            .build()
            .expect("entity builds");
        selection.select_only(entity);
        let enabled_entries = entries(Menu::Edit, &view_over(&snapshot, &selection, &commands));
        let action_rows = action_states(&enabled_entries);
        assert!(action_rows[0].1);
        assert!(action_rows[1].1);
        assert!(action_rows[5].1);
        // The selection names an entity the snapshot has not captured yet, so
        // there is nothing to carry and Copy and Cut stay grey. Offering them
        // would copy the asset the selection replaced, or nothing.
        assert_eq!((action_rows[2].1, action_rows[3].1), (false, false));
        snapshot.detail = Some(crate::editor_state::EntityDetail {
            entity,
            components: Vec::new(),
        });
        let captured_entries = entries(Menu::Edit, &view_over(&snapshot, &selection, &commands));
        let action_rows = action_states(&captured_entries);
        // A captured detail gives Copy and Cut something to carry.
        assert_eq!((action_rows[2].1, action_rows[3].1), (true, true));
        match &enabled_entries[6] {
            MenuEntry::Action {
                action: MenuAction::Queue { label, commands },
                ..
            } => {
                assert_eq!(label, "delete entity");
                assert_eq!(commands.len(), 2);
            }
            _ => panic!("the delete row must queue its recorded step"),
        }
    }

    #[test]
    fn the_create_menu_builds_one_empty_entity() {
        let commands = commands_fixture();
        let snapshot = snapshot_fixture();
        let selection = EditorSelection::default();
        let menu_entries = entries(Menu::Create, &view_over(&snapshot, &selection, &commands));
        // The entity create row, then one row for a scene and one for a
        // standalone asset.
        assert_eq!(
            labels(&menu_entries),
            vec!["New Entity", "Create Scene", "Create Asset..."]
        );
        match &menu_entries[0] {
            MenuEntry::Action {
                action: MenuAction::Queue { label, commands },
                ..
            } => {
                assert_eq!(label, "new entity");
                assert_eq!(
                    commands,
                    &vec![EditorCommand::CreateEntity {
                        components: Vec::new()
                    }]
                );
            }
            _ => panic!("the create row must queue the recorded step"),
        }
    }

    #[test]
    fn the_view_menu_mirrors_the_toolbox_rows_and_checks_the_active_ones() {
        let commands = commands_fixture();
        let snapshot = snapshot_fixture();
        let selection = EditorSelection::default();
        let menu_entries = entries(Menu::View, &view_over(&snapshot, &selection, &commands));
        // The toolbox's six rows in the panel's order, then the simulation
        // controls after the last rule. The play row shows what a press does,
        // so it reads `Play` while the editor is being edited.
        assert_eq!(
            labels(&menu_entries),
            vec![
                "Translate",
                "Rotate",
                "Scale",
                "World",
                "Snap",
                "Grid",
                "Play",
                "Pause / Resume Simulation",
                "Step One Frame",
            ]
        );
        let action_rows = action_states(&menu_entries);
        // A fresh engine starts on the translate tool, in world space, with
        // snapping off.
        assert_eq!(action_rows[0].2, Some(true));
        assert_eq!(action_rows[1].2, Some(false));
        assert_eq!(action_rows[3].2, Some(false));
        // The grid row mirrors the renderer's switch, which is off on a fresh
        // engine.
        assert_eq!(action_rows[5].2, Some(snapshot.grid_enabled));
        // Three rules split the rows: before the space row, before the grid
        // row, and before the simulation controls.
        assert_eq!(separator_count(&menu_entries), 3);
        // The six rows that toggle carry a check state. The three simulation
        // rows are plain actions.
        assert!(action_rows[..6].iter().all(|row| row.2.is_some()));
        assert!(action_rows[6..].iter().all(|row| row.2.is_none()));
    }

    #[test]
    fn the_window_menu_lists_every_tab_in_strip_order() {
        let commands = commands_fixture();
        let snapshot = snapshot_fixture();
        let selection = EditorSelection::default();
        let menu_entries = entries(Menu::Window, &view_over(&snapshot, &selection, &commands));
        // The new-view row comes first, then one row per tab in the tab
        // strip's order.
        let mut expected: Vec<String> = vec!["New Scene View".to_string()];
        expected.extend(EditorTab::ALL.iter().map(|tab| tab.title()));
        assert_eq!(labels(&menu_entries), expected);
        assert!(matches!(
            menu_entries[0],
            MenuEntry::Action {
                action: MenuAction::NewSceneView,
                ..
            }
        ));
        for (entry, tab) in menu_entries.iter().skip(1).zip(EditorTab::ALL.iter()) {
            match entry {
                MenuEntry::Action {
                    action: MenuAction::ActivateTab(activated),
                    ..
                } => assert_eq!(activated, tab),
                _ => panic!("a window row must activate its tab"),
            }
        }
    }

    #[test]
    fn the_help_menu_opens_about() {
        let commands = commands_fixture();
        let snapshot = snapshot_fixture();
        let selection = EditorSelection::default();
        let menu_entries = entries(Menu::Help, &view_over(&snapshot, &selection, &commands));
        assert_eq!(labels(&menu_entries), vec!["About Pill Editor"]);
        assert!(matches!(
            menu_entries[0],
            MenuEntry::Action {
                action: MenuAction::About,
                ..
            }
        ));
    }

    #[test]
    fn every_command_a_drawn_menu_lists_has_an_action() {
        let commands = commands_fixture();
        let snapshot = snapshot_fixture();
        let selection = EditorSelection::default();
        let titles: Vec<&str> = Menu::ALL.iter().map(|menu| menu.title()).collect();
        for command in &commands {
            let Some(top) = command.menu_path.first() else {
                continue;
            };
            if !titles.contains(&top.as_str()) {
                continue;
            }
            assert!(
                action_for(command, &snapshot, &selection).is_some(),
                "{} is registered under the {} menu, which the bar draws, but \
                 no action answers it",
                command.id,
                top
            );
        }
    }

    #[test]
    fn no_registered_menu_path_names_a_menu_the_bar_cannot_draw() {
        let titles: Vec<&str> = Menu::ALL.iter().map(|menu| menu.title()).collect();
        for command in commands_fixture() {
            if let Some(top) = command.menu_path.first() {
                assert!(
                    titles.contains(&top.as_str()),
                    "{} names the {} menu, which Menu::ALL does not carry",
                    command.id,
                    top
                );
            }
        }
    }

    #[test]
    fn a_command_without_an_action_draws_no_row_and_runs_nothing() {
        let mut commands = commands_fixture();
        commands.push(CommandView {
            id: "editor.unimplemented".to_string(),
            title: "Unimplemented".to_string(),
            menu_path: vec!["Edit".to_string(), "Unimplemented".to_string()],
            separator_before: false,
        });
        let snapshot = snapshot_fixture();
        let selection = EditorSelection::default();
        let menu_entries = entries(Menu::Edit, &view_over(&snapshot, &selection, &commands));
        assert!(
            !labels(&menu_entries)
                .iter()
                .any(|label| label == "Unimplemented"),
            "a command with no action must not draw as a row that does nothing"
        );
        let unknown_command = commands
            .iter()
            .find(|command| command.id == "editor.unimplemented")
            .expect("the fixture pushed it");
        assert_eq!(action_for(unknown_command, &snapshot, &selection), None);
    }

    #[test]
    fn dropping_a_command_from_the_registry_drops_its_row() {
        let kept_commands: Vec<CommandView> = commands_fixture()
            .into_iter()
            .filter(|command| command.id != "editor.undo")
            .collect();
        let snapshot = snapshot_fixture();
        let selection = EditorSelection::default();
        let menu_entries = entries(
            Menu::Edit,
            &view_over(&snapshot, &selection, &kept_commands),
        );
        assert!(
            !labels(&menu_entries).iter().any(|label| label == "Undo"),
            "the row must follow the registry, not a table in the shell"
        );
    }

    #[test]
    fn a_renamed_command_renames_its_row() {
        let mut commands = commands_fixture();
        let undo_command = commands
            .iter_mut()
            .find(|command| command.id == "editor.undo")
            .expect("the built-ins register undo");
        undo_command.title = "Step Back".to_string();
        let snapshot = snapshot_fixture();
        let selection = EditorSelection::default();
        let menu_entries = entries(Menu::Edit, &view_over(&snapshot, &selection, &commands));
        assert_eq!(labels(&menu_entries)[0], "Step Back");
    }

    /// The play control reads what a press does, and queues that: `Play` while
    /// the editor is being edited, `Stop` while a session runs.
    #[test]
    fn the_play_control_follows_the_run_mode() {
        assert_eq!(play_title(RunMode::Editing), "Play");
        assert_eq!(play_title(RunMode::Playing), "Stop");
        assert_eq!(
            play_toggle_command(RunMode::Editing),
            EditorCommand::EnterPlay
        );
        assert_eq!(
            play_toggle_command(RunMode::Playing),
            EditorCommand::ExitPlay
        );
        // A session is not an undo step, so neither command records.
        assert!(!EditorCommand::EnterPlay.recordable());
        assert!(!EditorCommand::ExitPlay.recordable());
    }

    /// The View menu's play row greys nothing out and shows the same label the
    /// toolbar's first button does.
    #[test]
    fn the_view_menu_shows_the_play_row_for_both_modes() {
        let commands = commands_fixture();
        let selection = EditorSelection::default();
        let mut snapshot = snapshot_fixture();
        let editing = entries(Menu::View, &view_over(&snapshot, &selection, &commands));
        assert!(labels(&editing).contains(&"Play".to_string()));
        snapshot.run_mode = RunMode::Playing;
        let playing = entries(Menu::View, &view_over(&snapshot, &selection, &commands));
        assert!(labels(&playing).contains(&"Stop".to_string()));
        let rows = action_states(&playing);
        let play_row = rows
            .iter()
            .position(|row| row.0 == "Stop")
            .expect("the play row is in the View menu");
        assert!(rows[play_row].1, "the play row is never greyed out");
    }

    /// Returns a snapshot over a fresh engine, which the entry tests take
    /// views of.
    ///
    /// The engine is put in edit mode first, because that is the mode an
    /// editor's own engine is in: a fresh engine defaults to playing, which is
    /// what a standalone build does.
    fn snapshot_fixture() -> EditorSnapshot {
        let mut engine = pill_engine::Engine::new();
        engine.set_run_mode(RunMode::Editing);
        EditorSnapshot::capture_engine_tiers(&engine, 0, &[], Vec::new())
    }
}
