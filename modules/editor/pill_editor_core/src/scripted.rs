//! The scripted test channel's grammar: one typed parse of a command line.
//!
//! # Responsibilities
//!
//! - Parse one scripted line into a [`ScriptedCommand`], or explain why it
//!   cannot run.
//! - Keep the grammar - verbs, arguments and value forms - in one file, so
//!   it can be table-tested without launching an editor.
//!
//! # Design
//!
//! The vocabulary is the suites' API. `test_editor_egui.py` writes these
//! lines, so a verb name or an argument order cannot change without the suite
//! noticing. Both editors parse with this module and run only what their
//! posture can. A verb a posture cannot run is refused with the same
//! `[editor] scripted command rejected` line the Python side raises on. The
//! module names model types only, so the browser build can link it.
//!
//! Arguments are whitespace-separated words. A trailing free-text value
//! (`edit set`, a path, a name) takes the rest of the line and keeps its
//! spaces. One surrounding pair of double quotes is removed, and a backslash
//! escapes the character after it. A path that is not the last argument
//! (`edit asset move <from> <to>`, `edit asset create <type> <folder> <name>`)
//! keeps its spaces only when it is quoted, because the cursor's `path`
//! reader stops a bare path at the first space.
//!
//! `key <chord>` joins `ctrl`, `shift`, `alt` or `super` (`control`, `win`
//! and `cmd` work too) with `+`, then names a letter, a digit, `f1`..`f24` or
//! a named editing key from [`key_code_name`].

// External crates
use pill_engine::KeyCode;

// Current crate
use crate::camera::CameraNavigation;
use crate::gizmo::GizmoMode;
use crate::layout::EditorTab;

// =============================================================================
// The command
// =============================================================================

/// What a scripted `command` line names as the subject of the invocation,
/// before the executable resolves it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptedTarget {
    /// No subject: the line ran a command the way a menu row does.
    None,
    /// An entity, by the display handle the Entities list shows.
    Entity(String),
    /// An asset, by its project-relative key.
    Asset(String),
}

/// One parsed scripted line.
///
/// Every variant names a side effect the runner performs; the values are
/// owned, so a parsed command outlives the line it came from.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptedCommand {
    /// `quit` - leave the editor through its own quit path.
    Quit,
    /// `palette [<query>]`, `palette run`, `palette close` - open the command
    /// palette, run its selected row, or close it.
    Palette(PaletteRequest),
    /// `console filter <level>` - the lowest level the Console tab shows.
    ConsoleFilter(String),
    /// `console search <text>` - the Console tab's substring filter.
    ConsoleSearch(String),
    /// `problem open <index>` - open a Problems row, as a click would.
    ProblemOpen(usize),
    /// `safe_mode retry` - press the safe-mode panel's Retry button.
    SafeModeRetry,
    /// `command <id> [entity <name> | asset <path>]` - run a command a module
    /// registered, with the subject the line names. The editor's own ids keep
    /// their concrete verbs below.
    Command {
        /// The id the command registered under.
        id: String,
        /// What the command is asked about.
        target: ScriptedTarget,
    },
    /// `panel unload <source>` - drop one module's liveness marker (the value
    /// that says the module is loaded), so its panels read "module not
    /// loaded" as they do after an unload.
    PanelUnload(String),
    /// `panel restore <source>` - bind that module's panels to a fresh
    /// liveness marker, as a reload's re-registration does.
    PanelRestore(String),
    /// `panel window <slot> <id>` - ask the panel in that slot to open the
    /// window it calls `id`, as clicking its own button would.
    PanelWindow {
        /// The panel's registry slot.
        slot: u16,
        /// The window id the panel chooses.
        id: String,
    },
    /// `undock <tab>` - float a dock tab into its own window.
    Undock(EditorTab),
    /// `tab <tab>` - bring a dock tab to the front.
    ActivateTab(EditorTab),
    /// `click <x> <y>` - a left press and release at a point.
    Click {
        /// The point's x in physical pixels.
        x: f32,
        /// The point's y in physical pixels.
        y: f32,
    },
    /// `rclick <x> <y>` - a right click, with no scene pick.
    RightClick {
        /// The point's x in physical pixels.
        x: f32,
        /// The point's y in physical pixels.
        y: f32,
    },
    /// `drag <x1> <y1> <x2> <y2>` - a left-button drag. The press, the move
    /// and the release each arrive in their own frame.
    Drag {
        /// The point the press starts at.
        from: (f32, f32),
        /// The point the release happens at.
        to: (f32, f32),
    },
    /// `gizmo drag <axis|plane> <dx> <dy>` - press a handle and drag it.
    GizmoDrag {
        /// The axis (`x`, `y`, `z`) or plane (`xy`, `xz`, `yz`) name.
        target: String,
        /// The drag's pixel delta.
        delta: (f32, f32),
    },
    /// `gizmo press <axis|plane>` - press a handle without moving it.
    GizmoPress {
        /// The axis (`x`, `y`, `z`) or plane (`xy`, `xz`, `yz`) name.
        target: String,
    },
    /// `gizmo release` - release a synthetic gizmo drag.
    GizmoRelease,
    /// `gizmo <translate|rotate|scale>` - switch the active tool.
    GizmoTool(GizmoMode),
    /// `extensions <on|off>` - register or drop the tool extensions the
    /// executable carries for the suites, so an area can drive the plain tool
    /// and the extended one in one session.
    Extensions {
        /// Whether they are registered afterwards.
        enabled: bool,
    },
    /// `drag element <id> <dx> <dy> <dz>` - nudge one element of the active
    /// tool by a world delta, as a direct drag would.
    DragElement {
        /// The tool's own id for the element.
        id: u64,
        /// The world delta the element moves by.
        delta: [f32; 3],
    },
    /// `element menu <item>` - run one row of the active tool's element
    /// context menu by its id, as a click on the row does.
    ElementMenu {
        /// The row's id, as the tool reported it.
        item: String,
    },
    /// `tool <name>` / `tool none` - activate a registered tool, or leave every
    /// tool with none active.
    Tool(Option<String>),
    /// `key <chord>` - inject a keyboard chord.
    Key(KeyChord),
    /// `dump <target>` - log one piece of editor state.
    Dump(Dump),
    /// `browser <folder>` - open one folder in the Assets grid.
    Browser(String),
    /// `camera orbit|pan|dolly <delta>` - navigate the active view.
    Camera {
        /// The gesture to apply.
        navigation: CameraNavigation,
        /// The gesture's pixel delta.
        delta: (f32, f32),
    },
    /// `camera zoom <lines>` - wheel the active view.
    CameraZoom(f32),
    /// `hover <view> <x> <y>` - move a view's scripted pointer.
    Hover {
        /// The scene view index.
        view: usize,
        /// The pointer's x in physical pixels.
        x: f32,
        /// The pointer's y in physical pixels.
        y: f32,
    },
    /// `hover ui <x> <y>` - move the shell's own pointer, as a real move would.
    ///
    /// The hover state belongs to egui, and egui learns a position only from
    /// an event. This line is how a suite makes a tooltip or a cursor appear
    /// without a mouse.
    HoverUi {
        /// The pointer's x in physical pixels.
        x: f32,
        /// The pointer's y in physical pixels.
        y: f32,
    },
    /// `select asset <path>` - move the shared selection onto an asset.
    SelectAsset(String),
    /// `select entity <handle>` - move the shared selection onto an entity.
    SelectEntity(String),
    /// `select add <handle>` - extend the selection as a Ctrl-click does.
    SelectAdd(String),
    /// `select range <handle>` - extend the selection over the Entities list's
    /// range from the primary, as a Shift-click does.
    ///
    /// Reserved: the native runner answers it, the browser editor refuses it
    /// by name, and no suite drives it - the grammar table is its only pin.
    SelectRange(String),
    /// `select resource <name>` - move the shared selection onto an editable
    /// world resource.
    SelectResource(String),
    /// `select element <id>[,<id>…]` / `select element add <id>` - select
    /// elements of the primary entity, replacing the element selection or
    /// extending it.
    ///
    /// The ids are the tool's own. Until the tool layer reports a kind (the
    /// tools stage), the editor records them under the `element` kind, so the
    /// "one kind per selection" rule has something to hold.
    SelectElement {
        /// The element ids to select, in selection order.
        ids: Vec<u64>,
        /// Whether to extend the current element selection instead of
        /// replacing it.
        add: bool,
    },
    /// `panel field <component> <field> <value>` - queue the component-field
    /// write a module panel's own row would queue, against the selected
    /// entity.
    PanelField {
        /// The component's registered name, or its `::Name` suffix.
        component: String,
        /// The field's name.
        field: String,
        /// The new value, as the field's kind parses it.
        value: String,
    },
    /// `edit resource <field> <value>` - one recorded resource field edit.
    EditResource {
        /// The resource's stable name.
        resource: String,
        /// The field's name within the resource.
        field: String,
        /// The new value, as the field's kind parses it.
        value: String,
    },
    /// `toolbar <create|rename|delete|meta>` - press one Assets toolbar button.
    Toolbar(ToolbarAction),
    /// `scene new` - start an empty scene.
    SceneNew,
    /// `scene open <path>` - open a `.scene` file, replacing the world.
    SceneOpen(String),
    /// `scene save [<path>]` - write the world to the open scene, or to
    /// `path` when one is given.
    SceneSave(Option<String>),
    /// `dialog set name <text>` - type into the open asset dialog's name field.
    DialogSetName(String),
    /// `dialog submit` - press the open asset dialog's confirm button.
    DialogSubmit,
    /// `edit asset <action> ...` - queue one recorded asset-file edit.
    EditAsset(AssetEdit),
    /// `pick entity <entity> <component> <field>` - arm an entity reference
    /// row, so the next scene press names its reference.
    ///
    /// Reserved: no suite drives it, so the grammar table is its only pin.
    PickEntity {
        /// The entity the field belongs to.
        entity: String,
        /// The component's registered name, or its suffix.
        component: String,
        /// The field's name.
        field: String,
    },
    /// `edit set <entity> <component> <field> <value>` - one recorded field edit.
    EditSet {
        /// The entity's display handle (`3v0`).
        entity: String,
        /// The component's registered name, or its `::Name` suffix.
        component: String,
        /// The field's name.
        field: String,
        /// The new value, as the field's kind parses it.
        value: String,
    },
    /// `edit array <entity> <component> <field> <index> <value>` - one element edit.
    EditArray {
        /// The entity's display handle.
        entity: String,
        /// The component's registered name, or its suffix.
        component: String,
        /// The array field's name.
        field: String,
        /// The element's index.
        index: usize,
        /// The new element value.
        value: String,
    },
    /// `edit selection <component> <field> <value>` - one recorded field edit
    /// written to every selected entity that carries the component, as the
    /// Inspector's row queues it.
    EditSelection {
        /// The component's registered name, or its suffix.
        component: String,
        /// The field's name.
        field: String,
        /// The new value, as the field's kind parses it.
        value: String,
    },
    /// `edit asset-settings <path> <key> <value>` - one import-settings edit.
    EditAssetSettings {
        /// The asset's project-relative path.
        path: String,
        /// The `a.b[0]` settings key path.
        key: String,
        /// The new value, as the leaf's JSON kind parses it.
        value: String,
    },
    /// `edit system <index> <on|off>` - the recorded system toggle.
    EditSystem {
        /// The system's index in the Systems tab.
        index: usize,
        /// Whether the system should end up enabled.
        enabled: bool,
    },
    /// `edit preference <key> <value>` - one preferences row.
    EditPreference {
        /// The preference's dotted key.
        key: String,
        /// The new value, written as text.
        value: String,
    },
    /// `edit add|remove <entity> <component>` - attach or detach a component.
    EditComponent {
        /// The entity's display handle.
        entity: String,
        /// The component's registered name, or its suffix.
        component: String,
        /// Whether to attach (`add`) or detach (`remove`).
        add: bool,
    },
    /// `command editor.undo` - undo one step.
    Undo,
    /// `command editor.redo` - redo one step.
    Redo,
    /// `command editor.scene_new_view` - dock one more scene view.
    NewSceneView,
    /// `reveal <path>` - ask the file manager to show one asset's file, the way
    /// the Assets panel's row menu does. The path is the row's key, so a
    /// module's resource carries its `extensions/<module>/` prefix.
    Reveal(String),
    /// `pick reference <asset> <key> <candidate>` - choose an asset in the
    /// picker of one settings row, by the candidate's path, and queue the same
    /// recorded edit the row's click queues.
    PickReference {
        /// The owning asset's path below the asset directory.
        asset: String,
        /// The settings key the row edits, such as `textures.color`.
        key: String,
        /// The candidate's path, as the Assets panel lists it.
        candidate: String,
    },
    /// `popout <tab>` - move a tab into its own OS window. Add `redock` to put
    /// the tab back in the dock instead.
    Popout {
        /// The tab to move.
        tab: EditorTab,
        /// Whether to put the tab back in the dock instead of floating it.
        redock: bool,
    },
    /// `command editor.pause_toggle` - flip the engine's pause flag.
    PauseToggle,
    /// `command editor.play` - start a play session.
    Play,
    /// `command editor.stop` - stop the session and restore the edit world.
    Stop,
    /// `command editor.play_toggle` - Play when editing, Stop when playing.
    PlayToggle,
    /// `command editor.step` - run one frame while paused.
    Step,
    /// `command editor.entity_create` - the recorded New Entity edit.
    EntityCreate,
    /// `command editor.entity_delete` - the recorded delete of the selection.
    EntityDelete,
    /// `command editor.tool_snap_toggle` - flip the gizmo's snapping.
    SnapToggle,
    /// `command editor.tool_space_toggle` - flip world/local space.
    SpaceToggle,
    /// `command editor.copy` - put the selection on the OS clipboard.
    Copy,
    /// `command editor.cut` - copy the selection and delete it.
    Cut,
    /// `command editor.paste` - read the OS clipboard and apply it.
    Paste,
    /// `drop <path>` - hand one path to the drop handler as the OS would.
    Drop(String),
    /// `hover file <path>` - the same path as a drag preview.
    HoverFile(String),
    /// `drag out <path>` - hand one file to the OS drag.
    /// With no mouse button held the drag ends at once, which is what makes it
    /// safe to smoke-test from the scripted channel.
    DragOut(String),
}

/// What a `palette` line asks of the command palette.
///
/// The command changes the palette's state, not its pixels. The popup's own
/// keys (arrows, Enter, Escape) make the same changes that a person's presses
/// make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteRequest {
    /// Open the palette with this query in the field.
    Query(String),
    /// Run the selected row and close.
    Run,
    /// Close without running.
    Close,
}

/// Which piece of editor state a `dump` logs.
#[derive(Clone, Debug, PartialEq)]
pub enum Dump {
    /// `dump tabs` - the dock's surfaces and their tabs.
    Tabs,
    /// `dump scenes` - the scene view rectangles and the game view's.
    Scenes,
    /// `dump viewport` - the active view, the interaction state, the hovered
    /// handle and every view's camera.
    Viewport,
    /// `dump grid` - the scene grid's switch and the surfaces it draws on.
    Grid,
    /// `dump gizmos` - every component gizmo the world registered, whether an
    /// editor hid its type, and how many entities it drew last frame.
    Gizmos,
    /// `dump play` - the run mode, the pause flag and the world's restore
    /// epoch.
    Play,
    /// `dump assets` - the asset listing's rows.
    Assets,
    /// `dump engine-assets` - the assets the store holds that no file backs,
    /// which a picker offers beside the listing's rows.
    EngineAssets,
    /// `dump browser` - the project browser's open folder and open rows.
    Browser,
    /// `dump standalone-types` - the types the create dialog offers.
    StandaloneTypes,
    /// `dump systems` - the registered systems and their state.
    Systems,
    /// `dump tools` - the registered tools and the active one.
    Tools,
    /// `dump elements` - the active tool's report for the selected entity, as
    /// the editor draws and picks it.
    Elements,
    /// `dump preferences` - the preferences document and the live halves.
    Preferences,
    /// `dump theme` - the theme the editor paints with, the layers it was
    /// merged from, and the generation number that moves when they resolve
    /// again.
    Theme,
    /// `dump console` - the Console tab's filters and the rows they leave.
    Console,
    /// `dump problems` - the Problems rows and their locations.
    Problems,
    /// `dump stats` - the Statistics tab's frame and per-tab timings.
    Stats,
    /// `dump cadence` - the redraw counters.
    Cadence,
    /// `dump panels` - the module panels and their timings.
    Panels,
    /// `dump windows` - the panel windows on screen.
    Windows,
    /// `dump entities` - the entity list with each row's component names,
    /// which is what a collection test needs to find its subject.
    Entities,
    /// `dump selection` - the selected entities, primary last.
    Selection,
    /// `dump resources` - the editable world resources and their values.
    Resources,
    /// `dump field <entity> <component> <field>` - one component field's value.
    Field {
        /// The entity's display handle.
        entity: String,
        /// The component's registered name, or its suffix.
        component: String,
        /// The field's name.
        field: String,
    },
}

/// One Assets toolbar button a `toolbar` line presses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarAction {
    /// Open the create dialog.
    Create,
    /// Open the rename dialog for the selected file.
    Rename,
    /// Delete the selected file.
    Delete,
    /// Write the selected file's `.meta` sidecar.
    Meta,
}

/// One recorded asset-file edit an `edit asset` line queues.
#[derive(Clone, Debug, PartialEq)]
pub enum AssetEdit {
    /// `edit asset create <type> <folder|-> <name>` - create a standalone asset.
    Create {
        /// The standalone type's registered name.
        type_name: String,
        /// The destination folder, project-relative; `-` means the root.
        folder: String,
        /// The new file's stem.
        name: String,
    },
    /// `edit asset delete <path>` - delete one file.
    Delete {
        /// The file's project-relative path.
        path: String,
    },
    /// `edit asset move <from> <to>` - move one file.
    Move {
        /// The file's current project-relative path.
        from: String,
        /// The project-relative path the file moves to.
        to: String,
    },
    /// `edit asset meta <path>` - write one file's `.meta` sidecar.
    Meta {
        /// The file's project-relative path.
        path: String,
    },
}

/// One keyboard chord a `key` line injects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyChord {
    /// The key itself.
    pub key: KeyCode,
    /// Whether Control is held.
    pub control: bool,
    /// Whether Shift is held.
    pub shift: bool,
    /// Whether Alt is held.
    pub alt: bool,
    /// Whether the platform's Super key (Windows, Command) is held.
    pub super_key: bool,
}

impl KeyChord {
    /// The chord written out, as the log line and a suite read it:
    /// `ctrl+shift+z`, `w`.
    pub fn label(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.control {
            parts.push("ctrl");
        }
        if self.shift {
            parts.push("shift");
        }
        if self.alt {
            parts.push("alt");
        }
        if self.super_key {
            parts.push("super");
        }
        parts.push(key_code_name(self.key));
        parts.join("+")
    }

    /// The character the chord types into a text field, when it types one.
    ///
    /// A letter, a digit or the space bar types its character. Shift types
    /// the capital of a letter, as the real key would. A chord that holds
    /// Control, Alt or Super is a command and types nothing, and the named
    /// keys (Enter, Tab, Delete, the arrows) act instead of typing. A shifted
    /// digit also types nothing: the number row's symbols are not in the
    /// key-name table, and the unshifted digit would be the wrong character.
    pub fn typed_character(&self) -> Option<char> {
        if self.control || self.alt || self.super_key {
            return None;
        }
        let name = key_code_name(self.key);
        if let Some(character) = name.chars().next().filter(|_| name.len() == 1) {
            if character.is_ascii_digit() {
                return if self.shift { None } else { Some(character) };
            }
            return Some(if self.shift {
                character.to_ascii_uppercase()
            } else {
                character
            });
        }
        (self.key == KeyCode::Space).then_some(' ')
    }
}

/// Why a scripted line could not be parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// The sentence the runner logs beside the line, which says what the verb
    /// needs.
    pub reason: String,
}

impl ParseError {
    /// One error carrying `reason`.
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

// =============================================================================
// Parsing
// =============================================================================

impl ScriptedCommand {
    /// Parse one line of the scripted command file.
    ///
    /// The caller logs the line and the returned reason, so a suite sees a
    /// refusal instead of waiting out its timeout.
    ///
    /// # Errors
    ///
    /// Returns a [`ParseError`] when the line is empty, when the verb or one
    /// of its targets is unknown, and when an argument the verb needs is
    /// missing or malformed. Its reason says what the verb expects.
    pub fn parse(line: &str) -> Result<ScriptedCommand, ParseError> {
        let mut cursor = Cursor::new(line);
        let Some(verb) = cursor.word() else {
            return Err(ParseError::new("the line is empty"));
        };
        match verb {
            "quit" => {
                cursor.finish(verb)?;
                Ok(ScriptedCommand::Quit)
            }
            "undock" => {
                let tab = tab(&mut cursor, "undock needs a tab")?;
                cursor.finish(verb)?;
                Ok(ScriptedCommand::Undock(tab))
            }
            "tab" => {
                let tab = tab(&mut cursor, "tab needs a tab name")?;
                cursor.finish(verb)?;
                Ok(ScriptedCommand::ActivateTab(tab))
            }
            // `popout <tab>` moves a tab into its own OS window, and
            // `popout <tab> redock` moves it back into the dock.
            "popout" => {
                let tab = tab(&mut cursor, "popout needs a tab name")?;
                let redock = match cursor.word() {
                    Some("redock") => true,
                    Some(other) => {
                        return Err(ParseError::new(format!(
                            "popout takes a tab name and optionally 'redock', not '{other}'"
                        )))
                    }
                    None => false,
                };
                cursor.finish(verb)?;
                Ok(ScriptedCommand::Popout { tab, redock })
            }
            "click" => {
                let x = number(&mut cursor, "click needs an x and a y")?;
                let y = number(&mut cursor, "click needs an x and a y")?;
                cursor.finish(verb)?;
                Ok(ScriptedCommand::Click { x, y })
            }
            "rclick" => {
                let x = number(&mut cursor, "rclick needs an x and a y")?;
                let y = number(&mut cursor, "rclick needs an x and a y")?;
                cursor.finish(verb)?;
                Ok(ScriptedCommand::RightClick { x, y })
            }
            // `drag x1 y1 x2 y2` is a pointer gesture. `drag out <path>` hands
            // one file to the OS drag. No mouse button is held there, so the
            // drag ends at once and is safe to drive from the file.
            // `drag element <id> <dx> <dy> <dz>` nudges one element instead.
            "drag" => {
                let is_drag_out = {
                    let rest = cursor.rest();
                    rest == "out" || rest.starts_with("out ")
                };
                let is_element_drag = {
                    let rest = cursor.rest();
                    rest == "element" || rest.starts_with("element ")
                };
                if is_drag_out {
                    cursor.word();
                    let path = cursor.text("drag out needs a path")?;
                    Ok(ScriptedCommand::DragOut(path))
                } else if is_element_drag {
                    cursor.word();
                    let id = number(&mut cursor, "drag element needs an id and three deltas")?;
                    let x = number(&mut cursor, "drag element needs three deltas")?;
                    let y = number(&mut cursor, "drag element needs three deltas")?;
                    let z = number(&mut cursor, "drag element needs three deltas")?;
                    cursor.finish(verb)?;
                    Ok(ScriptedCommand::DragElement { id, delta: [x, y, z] })
                } else {
                    let from = point(&mut cursor, "drag needs x1 y1 x2 y2")?;
                    let to = point(&mut cursor, "drag needs x1 y1 x2 y2")?;
                    cursor.finish(verb)?;
                    Ok(ScriptedCommand::Drag { from, to })
                }
            }
            // `element menu <item>` runs one row of the active tool's element
            // context menu by its id.
            "element" => match cursor.word() {
                Some("menu") => {
                    let item = cursor
                        .word()
                        .ok_or_else(|| ParseError::new("element menu needs an item id"))?;
                    cursor.finish("element menu")?;
                    Ok(ScriptedCommand::ElementMenu {
                        item: item.to_string(),
                    })
                }
                _ => Err(ParseError::new("element needs `menu <item>`")),
            },
            "gizmo" => gizmo(&mut cursor),
            // `console filter <level>` and `console search <text>` set the
            // Console tab's filters. The executable applies them next frame.
            "console" => match cursor.word() {
                Some("filter") => {
                    let level = cursor
                        .word()
                        .ok_or_else(|| ParseError::new("console filter needs a level"))?;
                    cursor.finish("console filter")?;
                    Ok(ScriptedCommand::ConsoleFilter(level.to_string()))
                }
                Some("search") => {
                    let text = cursor.text("console search needs text")?;
                    Ok(ScriptedCommand::ConsoleSearch(text))
                }
                _ => Err(ParseError::new(
                    "console needs `filter <level>` or `search <text>`",
                )),
            },
            // `problem open <index>` opens a Problems row the way a click
            // does, which makes the executable spawn the configured editor.
            // `safe_mode retry` presses the safe-mode panel's Retry button,
            // which the executable draws only while no shell is loaded.
            "safe_mode" => match cursor.word() {
                Some("retry") => {
                    cursor.finish("safe_mode retry")?;
                    Ok(ScriptedCommand::SafeModeRetry)
                }
                _ => Err(ParseError::new("safe_mode needs `retry`")),
            },
            "problem" => match cursor.word() {
                Some("open") => {
                    let index = number(&mut cursor, "problem open needs a row index")?;
                    cursor.finish("problem open")?;
                    Ok(ScriptedCommand::ProblemOpen(index))
                }
                _ => Err(ParseError::new("problem needs `open <index>`")),
            },
            // `reveal <path>` is the row menu's show-in-file-manager action,
            // driven from the channel. The executable looks the file up and
            // opens the file manager on it.
            "reveal" => {
                let path = cursor.text("reveal needs a path")?;
                Ok(ScriptedCommand::Reveal(path))
            }
            "panel" => match cursor.word() {
                Some("unload") => {
                    let source = cursor
                        .word()
                        .ok_or_else(|| ParseError::new("panel unload needs a module name"))?;
                    cursor.finish("panel unload")?;
                    Ok(ScriptedCommand::PanelUnload(source.to_string()))
                }
                Some("restore") => {
                    let source = cursor
                        .word()
                        .ok_or_else(|| ParseError::new("panel restore needs a module name"))?;
                    cursor.finish("panel restore")?;
                    Ok(ScriptedCommand::PanelRestore(source.to_string()))
                }
                Some("window") => {
                    let slot = number(&mut cursor, "panel window needs a slot and a window id")?;
                    let id = cursor
                        .word()
                        .ok_or_else(|| ParseError::new("panel window needs a window id"))?;
                    cursor.finish("panel window")?;
                    Ok(ScriptedCommand::PanelWindow {
                        slot,
                        id: id.to_string(),
                    })
                }
                // The write one of a panel's own rows queues, driven from the
                // test channel instead of from the pointer.
                Some("field") => {
                    let reason = "panel field needs a component, a field and a value";
                    let component = cursor.word().ok_or_else(|| ParseError::new(reason))?;
                    let field = cursor.word().ok_or_else(|| ParseError::new(reason))?;
                    let value = cursor.text("panel field needs a value")?;
                    Ok(ScriptedCommand::PanelField {
                        component: component.to_string(),
                        field: field.to_string(),
                        value,
                    })
                }
                _ => Err(ParseError::new(
                    "panel needs `unload <module>`, `restore <module>`, `window <slot> <id>` or `field <component> <field> <value>`",
                )),
            },
            // `palette <query>` opens the palette with the query typed in,
            // and `palette run` and `palette close` drive the open palette.
            "palette" => {
                let first = cursor.word();
                let tail = cursor.rest();
                match (first, tail) {
                    (Some("run"), "") => Ok(ScriptedCommand::Palette(PaletteRequest::Run)),
                    (Some("close"), "") => Ok(ScriptedCommand::Palette(PaletteRequest::Close)),
                    (None, _) => Ok(ScriptedCommand::Palette(PaletteRequest::Query(
                        String::new(),
                    ))),
                    (Some(first), "") => Ok(ScriptedCommand::Palette(PaletteRequest::Query(
                        first.to_string(),
                    ))),
                    (Some(first), tail) => Ok(ScriptedCommand::Palette(PaletteRequest::Query(
                        format!("{first} {tail}"),
                    ))),
                }
            }
            "key" => {
                let chord = chord(&mut cursor)?;
                cursor.finish(verb)?;
                Ok(ScriptedCommand::Key(chord))
            }
            // The OS drop, driven from the file. winit's own events cannot be
            // synthesised from here, so this calls the handler they call.
            "drop" => {
                let path = cursor.text("drop needs a path")?;
                Ok(ScriptedCommand::Drop(path))
            }
            "hover" => match cursor.word() {
                Some("file") => {
                    let path = cursor.text("hover file needs a path")?;
                    Ok(ScriptedCommand::HoverFile(path))
                }
                // The shell's own pointer, which sets the UI's hover state.
                // The scene hover below never touches that state.
                Some("ui") => {
                    let x = number(&mut cursor, "hover ui needs an x")?;
                    let y = number(&mut cursor, "hover ui needs a y")?;
                    cursor.finish(verb)?;
                    Ok(ScriptedCommand::HoverUi { x, y })
                }
                // The word just read was the view index, so the numbers that
                // follow it are the pointer position.
                other => {
                    let view = match other {
                        Some(text) => text.parse::<usize>().map_err(|_| {
                            ParseError::new(format!(
                                "hover needs a view index, an x and a y, got '{text}'"
                            ))
                        })?,
                        None => {
                            return Err(ParseError::new("hover needs a view, an x and a y"));
                        }
                    };
                    let x = number(&mut cursor, "hover needs an x")?;
                    let y = number(&mut cursor, "hover needs a y")?;
                    cursor.finish(verb)?;
                    Ok(ScriptedCommand::Hover { view, x, y })
                }
            },
            "dump" => {
                let dump = dump(&mut cursor)?;
                Ok(ScriptedCommand::Dump(dump))
            }
            "browser" => {
                let folder = cursor.text("browser needs a folder path or root")?;
                Ok(ScriptedCommand::Browser(folder))
            }
            "camera" => camera(&mut cursor),
            "select" => match cursor.word() {
                Some("asset") => {
                    let path = cursor.text("select asset needs a path")?;
                    Ok(ScriptedCommand::SelectAsset(path))
                }
                Some("entity") => {
                    let handle = cursor
                        .word()
                        .ok_or_else(|| ParseError::new("select entity needs a handle"))?;
                    cursor.finish(verb)?;
                    Ok(ScriptedCommand::SelectEntity(handle.to_string()))
                }
                Some("add") => {
                    let handle = cursor
                        .word()
                        .ok_or_else(|| ParseError::new("select add needs a handle"))?;
                    cursor.finish(verb)?;
                    Ok(ScriptedCommand::SelectAdd(handle.to_string()))
                }
                Some("range") => {
                    let handle = cursor
                        .word()
                        .ok_or_else(|| ParseError::new("select range needs a handle"))?;
                    cursor.finish(verb)?;
                    Ok(ScriptedCommand::SelectRange(handle.to_string()))
                }
                Some("resource") => {
                    let name = cursor
                        .word()
                        .ok_or_else(|| ParseError::new("select resource needs a resource name"))?;
                    cursor.finish(verb)?;
                    Ok(ScriptedCommand::SelectResource(name.to_string()))
                }
                Some("element") => {
                    let next = cursor
                        .word()
                        .ok_or_else(|| ParseError::new("select element needs ids"))?;
                    let (add, list) = if next == "add" {
                        (
                            true,
                            cursor.word().ok_or_else(|| {
                                ParseError::new("select element add needs an id")
                            })?,
                        )
                    } else {
                        (false, next)
                    };
                    cursor.finish(verb)?;
                    let ids = list
                        .split(',')
                        .map(|id| {
                            id.trim().parse::<u64>().map_err(|_| {
                                ParseError::new("select element takes element ids, as numbers")
                            })
                        })
                        .collect::<Result<Vec<u64>, ParseError>>()?;
                    Ok(ScriptedCommand::SelectElement { ids, add })
                }
                _ => Err(ParseError::new("unknown select target")),
            },
            "toolbar" => {
                let action = toolbar_action(&mut cursor)?;
                cursor.finish(verb)?;
                Ok(ScriptedCommand::Toolbar(action))
            }
            "tool" => {
                let name = cursor.word().ok_or_else(|| {
                    ParseError::new("tool needs a name, or `none` to clear it")
                })?;
                cursor.finish(verb)?;
                let target = if name.eq_ignore_ascii_case("none") {
                    None
                } else {
                    Some(name.to_string())
                };
                Ok(ScriptedCommand::Tool(target))
            }
            // `extensions on|off` registers or drops the tool extensions the
            // executable carries for the suites.
            "extensions" => {
                let enabled = match cursor.word() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => {
                        return Err(ParseError::new(
                            "extensions needs `on` or `off`",
                        ));
                    }
                };
                cursor.finish("extensions")?;
                Ok(ScriptedCommand::Extensions { enabled })
            }
            "dialog" => dialog(&mut cursor),
            "edit" => edit(&mut cursor),
            "pick" => pick(&mut cursor),
            "scene" => scene_action(&mut cursor),
            "command" => command(&mut cursor),
            other => Err(ParseError::new(format!("unknown verb '{other}'"))),
        }
    }
}

/// `scene new|open <path>|save [<path>]`.
///
/// # Errors
///
/// Returns an error when the action is missing or unknown, and when `open`
/// names no path.
fn scene_action(cursor: &mut Cursor<'_>) -> Result<ScriptedCommand, ParseError> {
    let command = match cursor.word() {
        Some("new") => ScriptedCommand::SceneNew,
        Some("open") => {
            // Take the rest of the line, so a path with spaces in it parses.
            // The header promises as much, and `save` below reads a path the
            // same way.
            let path = cursor.text("scene open needs a .scene path")?;
            ScriptedCommand::SceneOpen(path)
        }
        // Save with no path writes the open scene; one argument is a save-as.
        Some("save") => {
            let path = if cursor.rest().is_empty() {
                None
            } else {
                Some(cursor.text("scene save takes at most one path")?)
            };
            ScriptedCommand::SceneSave(path)
        }
        Some(other) => {
            return Err(ParseError::new(format!(
                "unknown scene action '{other}' (new, open, save)"
            )))
        }
        None => return Err(ParseError::new("scene needs an action (new, open, save)")),
    };
    cursor.finish("scene")?;
    Ok(command)
}

/// `gizmo drag|press|release|<tool>`.
///
/// # Errors
///
/// Returns an error when the action or the target is missing, and when the
/// target is not a tool, an axis or a plane. A drag also fails when its two
/// deltas are missing or not numbers.
fn gizmo(cursor: &mut Cursor<'_>) -> Result<ScriptedCommand, ParseError> {
    match cursor.word() {
        Some("drag") => {
            let target = cursor
                .word()
                .ok_or_else(|| {
                    ParseError::new("gizmo drag needs an axis or plane and two pixel deltas")
                })?
                .to_string();
            let delta = point(
                cursor,
                "gizmo drag needs an axis or plane and two pixel deltas",
            )?;
            cursor.finish("gizmo drag")?;
            Ok(ScriptedCommand::GizmoDrag { target, delta })
        }
        Some("press") => {
            let target = cursor
                .word()
                .ok_or_else(|| ParseError::new("gizmo press needs an axis or plane"))?
                .to_string();
            cursor.finish("gizmo press")?;
            Ok(ScriptedCommand::GizmoPress { target })
        }
        Some("release") => {
            cursor.finish("gizmo release")?;
            Ok(ScriptedCommand::GizmoRelease)
        }
        Some(other) => match gizmo_mode(other) {
            Some(mode) => {
                cursor.finish("gizmo")?;
                Ok(ScriptedCommand::GizmoTool(mode))
            }
            None => Err(ParseError::new("unknown gizmo target")),
        },
        None => Err(ParseError::new("gizmo needs a tool or a drag")),
    }
}

/// `camera orbit|pan <dx> <dy>`, `camera dolly <dy>`, `camera zoom <lines>`.
///
/// # Errors
///
/// Returns an error when the gesture is missing or unknown, and when the
/// numbers it reads are missing or not numbers.
fn camera(cursor: &mut Cursor<'_>) -> Result<ScriptedCommand, ParseError> {
    let action = cursor
        .word()
        .ok_or_else(|| ParseError::new("camera needs orbit, pan, dolly or zoom"))?;
    match action {
        "orbit" | "pan" => {
            let delta = point(cursor, "camera orbit and pan need two pixel deltas")?;
            cursor.finish("camera")?;
            Ok(ScriptedCommand::Camera {
                navigation: if action == "orbit" {
                    CameraNavigation::Orbit
                } else {
                    CameraNavigation::Pan
                },
                delta,
            })
        }
        "dolly" => {
            let delta_y = number(cursor, "camera dolly needs a pixel delta")?;
            cursor.finish("camera dolly")?;
            Ok(ScriptedCommand::Camera {
                navigation: CameraNavigation::Dolly,
                delta: (0.0, delta_y),
            })
        }
        "zoom" => {
            let lines = number(cursor, "camera zoom needs a line count")?;
            cursor.finish("camera zoom")?;
            Ok(ScriptedCommand::CameraZoom(lines))
        }
        _ => Err(ParseError::new("camera needs orbit, pan, dolly or zoom")),
    }
}

/// `dump <target>` and `dump field <entity> <component> <field>`.
///
/// # Errors
///
/// Returns an error when the target is missing or unknown, and one naming
/// the missing arguments when a `field` line is short of them.
fn dump(cursor: &mut Cursor<'_>) -> Result<Dump, ParseError> {
    let target = cursor
        .word()
        .ok_or_else(|| ParseError::new("unknown dump target"))?;
    let dump = match target {
        "tabs" => Dump::Tabs,
        "scenes" => Dump::Scenes,
        "viewport" => Dump::Viewport,
        "grid" => Dump::Grid,
        "gizmos" => Dump::Gizmos,
        "play" => Dump::Play,
        "assets" => Dump::Assets,
        "engine-assets" => Dump::EngineAssets,
        "browser" => Dump::Browser,
        "standalone-types" => Dump::StandaloneTypes,
        "systems" => Dump::Systems,
        "tools" => Dump::Tools,
        "elements" => Dump::Elements,
        "preferences" => Dump::Preferences,
        "theme" => Dump::Theme,
        "console" => Dump::Console,
        "problems" => Dump::Problems,
        "stats" => Dump::Stats,
        "cadence" => Dump::Cadence,
        "panels" => Dump::Panels,
        "windows" => Dump::Windows,
        "resources" => Dump::Resources,
        "entities" => Dump::Entities,
        "selection" => Dump::Selection,
        "field" => {
            let reason = "dump field needs an entity, a component and a field";
            let entity = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let component = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let field = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let dump = Dump::Field {
                entity: entity.to_string(),
                component: component.to_string(),
                field: field.to_string(),
            };
            cursor.finish("dump field")?;
            return Ok(dump);
        }
        _ => return Err(ParseError::new("unknown dump target")),
    };
    cursor.finish("dump")?;
    Ok(dump)
}

/// `dialog set name <text>` and `dialog submit`.
///
/// # Errors
///
/// Returns an error when the action is neither `set name` nor `submit`, and
/// when the name text is missing.
fn dialog(cursor: &mut Cursor<'_>) -> Result<ScriptedCommand, ParseError> {
    match cursor.word() {
        Some("set") => match cursor.word() {
            Some("name") => {
                let text = cursor.text("dialog set name needs the text")?;
                Ok(ScriptedCommand::DialogSetName(text))
            }
            _ => Err(ParseError::new("dialog set needs name or type")),
        },
        Some("submit") => {
            cursor.finish("dialog submit")?;
            Ok(ScriptedCommand::DialogSubmit)
        }
        _ => Err(ParseError::new("dialog needs open, set, submit or cancel")),
    }
}

/// `pick entity <entity> <component> <field>`: arm an entity reference row, so
/// the next scene click names its reference.
///
/// # Errors
///
/// Returns an error when the target is neither `entity` nor `reference`, and
/// when one of the three arguments that follow is missing.
fn pick(cursor: &mut Cursor<'_>) -> Result<ScriptedCommand, ParseError> {
    match cursor.word() {
        Some("entity") => {
            let reason = "pick entity needs an entity, a component and a field";
            let entity = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let component = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let field = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            cursor.finish("pick entity")?;
            Ok(ScriptedCommand::PickEntity {
                entity: entity.to_string(),
                component: component.to_string(),
                field: field.to_string(),
            })
        }
        // `pick reference <asset> <key> <candidate>` - the choice a picker row
        // makes, by candidate path instead of by click.
        Some("reference") => {
            let reason = "pick reference needs an asset, a key and a candidate";
            let asset = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let key = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let candidate = cursor.text("pick reference needs a candidate")?;
            Ok(ScriptedCommand::PickReference {
                asset: asset.to_string(),
                key: key.to_string(),
                candidate,
            })
        }
        _ => Err(ParseError::new("pick needs entity or reference")),
    }
}

/// `edit <target>` and its sub-grammars.
///
/// # Errors
///
/// Returns an error when the target is missing or unknown, when one of its
/// arguments is missing, and when an index is not a number.
fn edit(cursor: &mut Cursor<'_>) -> Result<ScriptedCommand, ParseError> {
    match cursor.word() {
        Some("asset") => {
            let edit = asset_edit(cursor)?;
            Ok(ScriptedCommand::EditAsset(edit))
        }
        Some("set") => {
            let reason = "edit set needs an entity, a component, a field and a value";
            let entity = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let component = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let field = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let value = cursor.text("edit set needs a value")?;
            Ok(ScriptedCommand::EditSet {
                entity: entity.to_string(),
                component: component.to_string(),
                field: field.to_string(),
                value,
            })
        }
        Some("array") => {
            let reason = "edit array needs an entity, a component, a field, an index and a value";
            let entity = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let component = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let field = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let index_text = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let index = index_text.parse::<usize>().map_err(|_| {
                ParseError::new(format!(
                    "edit array needs a numeric index, got '{index_text}'"
                ))
            })?;
            let value = cursor.text("edit array needs a value")?;
            Ok(ScriptedCommand::EditArray {
                entity: entity.to_string(),
                component: component.to_string(),
                field: field.to_string(),
                index,
                value,
            })
        }
        Some("selection") => {
            let reason = "edit selection needs a component, a field and a value";
            let component = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let field = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let value = cursor.text("edit selection needs a value")?;
            Ok(ScriptedCommand::EditSelection {
                component: component.to_string(),
                field: field.to_string(),
                value,
            })
        }
        Some("asset-settings") => {
            let reason = "edit asset-settings needs a path, a key and a value";
            let path = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let key = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let value = cursor.text("edit asset-settings needs a value")?;
            Ok(ScriptedCommand::EditAssetSettings {
                path: path.to_string(),
                key: key.to_string(),
                value,
            })
        }
        Some("system") => {
            let reason = "edit system needs an index and on or off";
            let index_text = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let index = index_text
                .parse::<usize>()
                .map_err(|_| ParseError::new(reason))?;
            let enabled = match cursor.word() {
                Some("on") => true,
                Some("off") => false,
                _ => return Err(ParseError::new(reason)),
            };
            cursor.finish("edit system")?;
            Ok(ScriptedCommand::EditSystem { index, enabled })
        }
        Some("preference") => {
            let reason = "edit preference needs a key and a value";
            let key = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let value = cursor.text("edit preference needs a key and a value")?;
            Ok(ScriptedCommand::EditPreference {
                key: key.to_string(),
                value,
            })
        }
        Some("resource") => {
            let resource = cursor
                .word()
                .ok_or_else(|| ParseError::new("edit resource needs a resource name"))?;
            let field = cursor
                .word()
                .ok_or_else(|| ParseError::new("edit resource needs a field"))?;
            let value = cursor.text("edit resource needs a value")?;
            Ok(ScriptedCommand::EditResource {
                resource: resource.to_string(),
                field: field.to_string(),
                value,
            })
        }
        Some(action @ ("add" | "remove")) => {
            let reason = "edit add or remove needs an entity and a component";
            let entity = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let component = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            cursor.finish("edit add or remove")?;
            Ok(ScriptedCommand::EditComponent {
                entity: entity.to_string(),
                component: component.to_string(),
                add: action == "add",
            })
        }
        _ => Err(ParseError::new("unknown edit target")),
    }
}

/// `edit asset create|delete|move|meta`.
///
/// # Errors
///
/// Returns an error when the action is missing or unknown, when its type,
/// folder, name or destination is missing, and when a quoted path is never
/// closed.
fn asset_edit(cursor: &mut Cursor<'_>) -> Result<AssetEdit, ParseError> {
    let reason = "edit asset needs create, delete, move or meta with its arguments";
    let action = cursor
        .word()
        .ok_or_else(|| ParseError::new(reason))?
        .to_string();
    match action.as_str() {
        "create" => {
            let type_name = cursor.word().ok_or_else(|| ParseError::new(reason))?;
            let folder = cursor.path("edit asset create needs a folder")?;
            let name = cursor.text("edit asset create needs a name")?;
            Ok(AssetEdit::Create {
                type_name: type_name.to_string(),
                folder,
                name,
            })
        }
        "delete" => {
            let path = cursor.text("edit asset delete needs a path")?;
            Ok(AssetEdit::Delete { path })
        }
        "move" => {
            let from = cursor.path("edit asset move needs a source")?;
            let to = cursor.text("edit asset move needs a destination")?;
            Ok(AssetEdit::Move { from, to })
        }
        "meta" => {
            let path = cursor.text("edit asset meta needs a path")?;
            Ok(AssetEdit::Meta { path })
        }
        _ => Err(ParseError::new(reason)),
    }
}

/// `command <id> [entity <name> | asset <path>]` - the recorded editor actions
/// the suites drive, and the commands a module registered.
///
/// # Errors
///
/// Returns an error when the id is missing, when a built-in id carries any
/// argument, or when the target is neither `entity <name>` nor `asset <path>`.
/// An id this build does not know is not an error here: the executable rejects
/// it when nothing has registered it, so a module's command reads the same
/// whether this parser predates it or not.
fn command(cursor: &mut Cursor<'_>) -> Result<ScriptedCommand, ParseError> {
    let id = cursor
        .word()
        .ok_or_else(|| ParseError::new("command needs an id"))?;
    let command = match id {
        "editor.undo" => ScriptedCommand::Undo,
        "editor.redo" => ScriptedCommand::Redo,
        "editor.pause_toggle" => ScriptedCommand::PauseToggle,
        "editor.play" => ScriptedCommand::Play,
        "editor.stop" => ScriptedCommand::Stop,
        "editor.play_toggle" => ScriptedCommand::PlayToggle,
        "editor.step" => ScriptedCommand::Step,
        "editor.entity_create" => ScriptedCommand::EntityCreate,
        "editor.entity_delete" => ScriptedCommand::EntityDelete,
        "editor.tool_translate" => ScriptedCommand::GizmoTool(GizmoMode::Translate),
        "editor.tool_rotate" => ScriptedCommand::GizmoTool(GizmoMode::Rotate),
        "editor.tool_scale" => ScriptedCommand::GizmoTool(GizmoMode::Scale),
        "editor.tool_snap_toggle" => ScriptedCommand::SnapToggle,
        "editor.tool_space_toggle" => ScriptedCommand::SpaceToggle,
        "editor.copy" => ScriptedCommand::Copy,
        "editor.cut" => ScriptedCommand::Cut,
        "editor.paste" => ScriptedCommand::Paste,
        // One more scene view, the Window menu's own row.
        "editor.scene_new_view" => ScriptedCommand::NewSceneView,
        other => ScriptedCommand::Command {
            id: other.to_string(),
            target: scripted_target(cursor)?,
        },
    };
    cursor.finish("command")?;
    Ok(command)
}

/// The optional subject a scripted `command` line names.
///
/// # Errors
///
/// Returns an error when the target word is not `entity` or `asset`, or when
/// the name or path after it is missing.
fn scripted_target(cursor: &mut Cursor<'_>) -> Result<ScriptedTarget, ParseError> {
    match cursor.word() {
        None => Ok(ScriptedTarget::None),
        Some("entity") => Ok(ScriptedTarget::Entity(
            cursor.text("command entity needs an entity name")?,
        )),
        Some("asset") => Ok(ScriptedTarget::Asset(
            cursor.text("command asset needs a path")?,
        )),
        Some(other) => Err(ParseError::new(format!(
            "command takes `entity <name>` or `asset <path>`, got '{other}'"
        ))),
    }
}

/// `toolbar <action>`.
///
/// # Errors
///
/// Returns an error when the word is missing or is not one of the four
/// toolbar buttons.
fn toolbar_action(cursor: &mut Cursor<'_>) -> Result<ToolbarAction, ParseError> {
    let reason = "toolbar needs create, rename, delete or meta";
    match cursor.word().ok_or_else(|| ParseError::new(reason))? {
        "create" => Ok(ToolbarAction::Create),
        "rename" => Ok(ToolbarAction::Rename),
        "delete" => Ok(ToolbarAction::Delete),
        "meta" => Ok(ToolbarAction::Meta),
        _ => Err(ParseError::new(reason)),
    }
}

/// `key <chord>`, the modifiers joined to the key with `+`.
///
/// # Errors
///
/// Returns an error when the chord is missing, when a part before the key is
/// not a modifier, and when the last part is not a key name. A chord that
/// ends in `+` is malformed rather than unknown, and gets the general reason.
fn chord(cursor: &mut Cursor<'_>) -> Result<KeyChord, ParseError> {
    let reason = "key needs a chord, like ctrl+z or w";
    // Step 1: read the chord word.
    let text = cursor.word().ok_or_else(|| ParseError::new(reason))?;

    // Step 2: start a chord with no modifiers held, then read the
    // `+`-separated parts. Every part except the last is a modifier, and the
    // last one names the key.
    let mut chord = KeyChord {
        key: KeyCode::Escape,
        control: false,
        shift: false,
        alt: false,
        super_key: false,
    };
    let mut parts = text.split('+').peekable();
    let mut key = None;
    while let Some(part) = parts.next() {
        let is_last = parts.peek().is_none();
        if is_last {
            // A trailing `+` leaves no key part. That is a malformed chord
            // rather than an unknown key name, so it gets the general reason.
            if part.is_empty() {
                return Err(ParseError::new(reason));
            }
            key = Some(key_code(part).ok_or_else(|| {
                ParseError::new(format!(
                    "key '{part}' is not a key name; use a letter, a digit, f1..f24 \
                     or a named editing key"
                ))
            })?);
            break;
        }
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => chord.control = true,
            "shift" => chord.shift = true,
            "alt" => chord.alt = true,
            "super" | "win" | "cmd" => chord.super_key = true,
            _ => {
                return Err(ParseError::new(format!(
                    "key chord modifier '{part}' is not ctrl, shift, alt or super"
                )));
            }
        }
    }

    // Step 3: a chord always names a key.
    chord.key = key.ok_or_else(|| ParseError::new(reason))?;
    Ok(chord)
}

/// One key name: a single letter or digit, `f1`..`f24`, or a named key.
fn key_code(name: &str) -> Option<KeyCode> {
    let lowered = name.to_ascii_lowercase();
    if lowered.len() == 1 {
        let character = lowered.chars().next()?;
        return match character {
            'a'..='z' => letter_key(character),
            '0'..='9' => digit_key(character),
            _ => None,
        };
    }
    match lowered.as_str() {
        "escape" | "esc" => Some(KeyCode::Escape),
        "enter" | "return" => Some(KeyCode::Enter),
        "tab" => Some(KeyCode::Tab),
        "space" => Some(KeyCode::Space),
        "backspace" => Some(KeyCode::Backspace),
        "delete" | "del" => Some(KeyCode::Delete),
        "insert" | "ins" => Some(KeyCode::Insert),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "pageup" => Some(KeyCode::PageUp),
        "pagedown" => Some(KeyCode::PageDown),
        "left" => Some(KeyCode::ArrowLeft),
        "right" => Some(KeyCode::ArrowRight),
        "up" => Some(KeyCode::ArrowUp),
        "down" => Some(KeyCode::ArrowDown),
        "f1" => Some(KeyCode::F1),
        "f2" => Some(KeyCode::F2),
        "f3" => Some(KeyCode::F3),
        "f4" => Some(KeyCode::F4),
        "f5" => Some(KeyCode::F5),
        "f6" => Some(KeyCode::F6),
        "f7" => Some(KeyCode::F7),
        "f8" => Some(KeyCode::F8),
        "f9" => Some(KeyCode::F9),
        "f10" => Some(KeyCode::F10),
        "f11" => Some(KeyCode::F11),
        "f12" => Some(KeyCode::F12),
        "f13" => Some(KeyCode::F13),
        "f14" => Some(KeyCode::F14),
        "f15" => Some(KeyCode::F15),
        "f16" => Some(KeyCode::F16),
        "f17" => Some(KeyCode::F17),
        "f18" => Some(KeyCode::F18),
        "f19" => Some(KeyCode::F19),
        "f20" => Some(KeyCode::F20),
        "f21" => Some(KeyCode::F21),
        "f22" => Some(KeyCode::F22),
        "f23" => Some(KeyCode::F23),
        "f24" => Some(KeyCode::F24),
        _ => None,
    }
}

/// The name a key prints as, for [`KeyChord::label`].
///
/// The same names are what a tool's `on_key` hook is told, so a tool that
/// names a key and a scripted line that spells one agree.
pub fn key_code_name(key: KeyCode) -> &'static str {
    match key {
        KeyCode::KeyA => "a",
        KeyCode::KeyB => "b",
        KeyCode::KeyC => "c",
        KeyCode::KeyD => "d",
        KeyCode::KeyE => "e",
        KeyCode::KeyF => "f",
        KeyCode::KeyG => "g",
        KeyCode::KeyH => "h",
        KeyCode::KeyI => "i",
        KeyCode::KeyJ => "j",
        KeyCode::KeyK => "k",
        KeyCode::KeyL => "l",
        KeyCode::KeyM => "m",
        KeyCode::KeyN => "n",
        KeyCode::KeyO => "o",
        KeyCode::KeyP => "p",
        KeyCode::KeyQ => "q",
        KeyCode::KeyR => "r",
        KeyCode::KeyS => "s",
        KeyCode::KeyT => "t",
        KeyCode::KeyU => "u",
        KeyCode::KeyV => "v",
        KeyCode::KeyW => "w",
        KeyCode::KeyX => "x",
        KeyCode::KeyY => "y",
        KeyCode::KeyZ => "z",
        KeyCode::Digit0 => "0",
        KeyCode::Digit1 => "1",
        KeyCode::Digit2 => "2",
        KeyCode::Digit3 => "3",
        KeyCode::Digit4 => "4",
        KeyCode::Digit5 => "5",
        KeyCode::Digit6 => "6",
        KeyCode::Digit7 => "7",
        KeyCode::Digit8 => "8",
        KeyCode::Digit9 => "9",
        KeyCode::Escape => "escape",
        KeyCode::Enter => "enter",
        KeyCode::Tab => "tab",
        KeyCode::Space => "space",
        KeyCode::Backspace => "backspace",
        KeyCode::Delete => "delete",
        KeyCode::Insert => "insert",
        KeyCode::Home => "home",
        KeyCode::End => "end",
        KeyCode::PageUp => "pageup",
        KeyCode::PageDown => "pagedown",
        KeyCode::ArrowLeft => "left",
        KeyCode::ArrowRight => "right",
        KeyCode::ArrowUp => "up",
        KeyCode::ArrowDown => "down",
        KeyCode::F1 => "f1",
        KeyCode::F2 => "f2",
        KeyCode::F3 => "f3",
        KeyCode::F4 => "f4",
        KeyCode::F5 => "f5",
        KeyCode::F6 => "f6",
        KeyCode::F7 => "f7",
        KeyCode::F8 => "f8",
        KeyCode::F9 => "f9",
        KeyCode::F10 => "f10",
        KeyCode::F11 => "f11",
        KeyCode::F12 => "f12",
        KeyCode::F13 => "f13",
        KeyCode::F14 => "f14",
        KeyCode::F15 => "f15",
        KeyCode::F16 => "f16",
        KeyCode::F17 => "f17",
        KeyCode::F18 => "f18",
        KeyCode::F19 => "f19",
        KeyCode::F20 => "f20",
        KeyCode::F21 => "f21",
        KeyCode::F22 => "f22",
        KeyCode::F23 => "f23",
        KeyCode::F24 => "f24",
        _ => "key",
    }
}

/// The letter a lowercase `a`..`z` names.
fn letter_key(character: char) -> Option<KeyCode> {
    let key = match character {
        'a' => KeyCode::KeyA,
        'b' => KeyCode::KeyB,
        'c' => KeyCode::KeyC,
        'd' => KeyCode::KeyD,
        'e' => KeyCode::KeyE,
        'f' => KeyCode::KeyF,
        'g' => KeyCode::KeyG,
        'h' => KeyCode::KeyH,
        'i' => KeyCode::KeyI,
        'j' => KeyCode::KeyJ,
        'k' => KeyCode::KeyK,
        'l' => KeyCode::KeyL,
        'm' => KeyCode::KeyM,
        'n' => KeyCode::KeyN,
        'o' => KeyCode::KeyO,
        'p' => KeyCode::KeyP,
        'q' => KeyCode::KeyQ,
        'r' => KeyCode::KeyR,
        's' => KeyCode::KeyS,
        't' => KeyCode::KeyT,
        'u' => KeyCode::KeyU,
        'v' => KeyCode::KeyV,
        'w' => KeyCode::KeyW,
        'x' => KeyCode::KeyX,
        'y' => KeyCode::KeyY,
        'z' => KeyCode::KeyZ,
        _ => return None,
    };
    Some(key)
}

/// The digit `0`..`9` as its number-row key.
fn digit_key(character: char) -> Option<KeyCode> {
    let key = match character {
        '0' => KeyCode::Digit0,
        '1' => KeyCode::Digit1,
        '2' => KeyCode::Digit2,
        '3' => KeyCode::Digit3,
        '4' => KeyCode::Digit4,
        '5' => KeyCode::Digit5,
        '6' => KeyCode::Digit6,
        '7' => KeyCode::Digit7,
        '8' => KeyCode::Digit8,
        '9' => KeyCode::Digit9,
        _ => return None,
    };
    Some(key)
}

// =============================================================================
// Cursor
// =============================================================================

/// A position in one line, holding the text left after the words taken so
/// far.
struct Cursor<'a> {
    rest: &'a str,
}

impl<'a> Cursor<'a> {
    /// A cursor over `line`, with its outer whitespace trimmed.
    fn new(line: &'a str) -> Self {
        Self { rest: line.trim() }
    }

    /// The next whitespace-delimited word, or `None` when the line is spent.
    fn word(&mut self) -> Option<&'a str> {
        let trimmed = self.rest.trim_start();
        if trimmed.is_empty() {
            self.rest = trimmed;
            return None;
        }
        match trimmed.find(char::is_whitespace) {
            Some(end) => {
                self.rest = &trimmed[end..];
                Some(&trimmed[..end])
            }
            None => {
                self.rest = "";
                Some(trimmed)
            }
        }
    }

    /// Everything left, trimmed, with one surrounding `"..."` pair removed
    /// and its escapes resolved.
    ///
    /// # Errors
    ///
    /// Returns `reason` when the line has nothing left on it.
    fn text(&mut self, reason: &str) -> Result<String, ParseError> {
        let rest = self.rest.trim();
        if rest.is_empty() {
            return Err(ParseError::new(reason));
        }
        self.rest = "";
        Ok(unquote(rest))
    }

    /// One path argument: a quoted path, which may hold spaces, or one word.
    ///
    /// A path is not always the last argument, so it cannot take the rest of
    /// the line the way [`Cursor::text`] does. The quoted form is how one in
    /// the middle keeps a space, and the bare form stops at the first one.
    ///
    /// # Errors
    ///
    /// Returns `reason` when no path is left, and its own sentence when a
    /// quoted path is never closed.
    fn path(&mut self, reason: &str) -> Result<String, ParseError> {
        let rest = self.rest.trim();
        if let Some(body) = rest.strip_prefix('"') {
            let mut escaped = false;
            for (index, character) in body.char_indices() {
                match character {
                    '\\' if !escaped => escaped = true,
                    '"' if !escaped => {
                        let quoted_length = index + 2;
                        self.take(quoted_length);
                        return Ok(unquote(&rest[..quoted_length]));
                    }
                    _ => escaped = false,
                }
            }
            return Err(ParseError::new("path opens a quote it never closes"));
        }
        self.word()
            .map(str::to_string)
            .ok_or_else(|| ParseError::new(reason))
    }

    /// Consumes the first `length` bytes of what is left, after its leading
    /// whitespace.
    fn take(&mut self, length: usize) {
        let rest = self.rest.trim_start();
        self.rest = &rest[length.min(rest.len())..];
    }

    /// What is left of the line, trimmed, without consuming it.
    fn rest(&self) -> &'a str {
        self.rest.trim()
    }

    /// Refuses whatever is left after a verb took its arguments.
    ///
    /// # Errors
    ///
    /// Returns an error naming `verb` and the leftover text when the line has
    /// words the verb did not take.
    fn finish(&self, verb: &str) -> Result<(), ParseError> {
        let rest = self.rest.trim();
        if rest.is_empty() {
            return Ok(());
        }
        Err(ParseError::new(format!(
            "{verb} takes no more arguments, got '{rest}'"
        )))
    }
}

/// One surrounding pair of double quotes removed, with a backslash escaping
/// the character after it.
fn unquote(text: &str) -> String {
    let Some(text_between_quotes) = text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    else {
        return text.to_string();
    };
    let mut unquoted_text = String::with_capacity(text_between_quotes.len());
    let mut characters = text_between_quotes.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            unquoted_text.push(character);
            continue;
        }
        match characters.next() {
            Some(escaped) => unquoted_text.push(escaped),
            None => unquoted_text.push('\\'),
        }
    }
    unquoted_text
}

/// The next word as a tab name.
///
/// # Errors
///
/// Returns `reason` when the next word is missing or is not a tab name.
fn tab(cursor: &mut Cursor<'_>, reason: &str) -> Result<EditorTab, ParseError> {
    cursor
        .word()
        .and_then(tab_by_name)
        .ok_or_else(|| ParseError::new(reason))
}

/// The next word as a number.
///
/// # Errors
///
/// Returns `reason` when the next word is missing or does not parse as the
/// requested number type.
fn number<T: std::str::FromStr>(cursor: &mut Cursor<'_>, reason: &str) -> Result<T, ParseError> {
    cursor
        .word()
        .and_then(|word| word.parse().ok())
        .ok_or_else(|| ParseError::new(reason))
}

/// The next two words as a point or a delta.
///
/// # Errors
///
/// Returns `reason` when either of the two words is missing or is not a
/// number.
fn point(cursor: &mut Cursor<'_>, reason: &str) -> Result<(f32, f32), ParseError> {
    let x = number(cursor, reason)?;
    let y = number(cursor, reason)?;
    Ok((x, y))
}

/// The tab a scripted name selects.
///
/// It reads the name with [`EditorTab::from_wire`], the parser every side of
/// the editor shares. A name this channel accepts is therefore a name the
/// dock and the popout map accept.
fn tab_by_name(name: &str) -> Option<EditorTab> {
    EditorTab::from_wire(name)
}

/// The tool a scripted name selects.
fn gizmo_mode(name: &str) -> Option<GizmoMode> {
    match name {
        "translate" => Some(GizmoMode::Translate),
        "rotate" => Some(GizmoMode::Rotate),
        "scale" => Some(GizmoMode::Scale),
        _ => None,
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// One accepted line and the command it parses to.
    struct Accept {
        /// The scripted line.
        line: &'static str,
        /// What it means.
        command: ScriptedCommand,
    }

    /// One refused line and the sentence it is refused with.
    struct Refuse {
        /// The scripted line.
        line: &'static str,
        /// The expected reason.
        reason: &'static str,
    }

    /// A chord for the key it is given, with no modifiers held.
    fn unmodified_chord(key_code: KeyCode) -> KeyChord {
        KeyChord {
            key: key_code,
            control: false,
            shift: false,
            alt: false,
            super_key: false,
        }
    }

    #[test]
    fn every_verb_parses() {
        let accepted = vec![
            Accept {
                line: "quit",
                command: ScriptedCommand::Quit,
            },
            Accept {
                line: "undock entities",
                command: ScriptedCommand::Undock(EditorTab::Entities),
            },
            Accept {
                line: "tab inspector",
                command: ScriptedCommand::ActivateTab(EditorTab::Inspector),
            },
            Accept {
                line: "tab scene",
                command: ScriptedCommand::ActivateTab(EditorTab::Scene(0)),
            },
            Accept {
                line: "tab scene2",
                command: ScriptedCommand::ActivateTab(EditorTab::Scene(1)),
            },
            Accept {
                line: "tab scene3",
                command: ScriptedCommand::ActivateTab(EditorTab::Scene(2)),
            },
            Accept {
                line: "command editor.scene_new_view",
                command: ScriptedCommand::NewSceneView,
            },
            Accept {
                line: "reveal models/cube.obj",
                command: ScriptedCommand::Reveal("models/cube.obj".to_string()),
            },
            Accept {
                line: "pick reference materials/a.material textures.color textures/rock.png",
                command: ScriptedCommand::PickReference {
                    asset: "materials/a.material".to_string(),
                    key: "textures.color".to_string(),
                    candidate: "textures/rock.png".to_string(),
                },
            },
            Accept {
                line: "popout inspector",
                command: ScriptedCommand::Popout {
                    tab: EditorTab::Inspector,
                    redock: false,
                },
            },
            Accept {
                line: "popout inspector redock",
                command: ScriptedCommand::Popout {
                    tab: EditorTab::Inspector,
                    redock: true,
                },
            },
            Accept {
                line: "palette translate",
                command: ScriptedCommand::Palette(PaletteRequest::Query("translate".to_string())),
            },
            Accept {
                line: "palette pause resume",
                command: ScriptedCommand::Palette(PaletteRequest::Query(
                    "pause resume".to_string(),
                )),
            },
            Accept {
                line: "palette",
                command: ScriptedCommand::Palette(PaletteRequest::Query(String::new())),
            },
            Accept {
                line: "palette run",
                command: ScriptedCommand::Palette(PaletteRequest::Run),
            },
            Accept {
                line: "palette close",
                command: ScriptedCommand::Palette(PaletteRequest::Close),
            },
            Accept {
                line: "click 640 312",
                command: ScriptedCommand::Click { x: 640.0, y: 312.0 },
            },
            Accept {
                line: "click 640.5 -12",
                command: ScriptedCommand::Click { x: 640.5, y: -12.0 },
            },
            Accept {
                line: "rclick 200 80",
                command: ScriptedCommand::RightClick { x: 200.0, y: 80.0 },
            },
            Accept {
                line: "drag 10 20 40 60",
                command: ScriptedCommand::Drag {
                    from: (10.0, 20.0),
                    to: (40.0, 60.0),
                },
            },
            Accept {
                line: "drag out local/temp/shot.png",
                command: ScriptedCommand::DragOut("local/temp/shot.png".to_string()),
            },
            Accept {
                line: "gizmo drag x 12 -4",
                command: ScriptedCommand::GizmoDrag {
                    target: "x".to_string(),
                    delta: (12.0, -4.0),
                },
            },
            Accept {
                line: "gizmo press xy",
                command: ScriptedCommand::GizmoPress {
                    target: "xy".to_string(),
                },
            },
            Accept {
                line: "gizmo release",
                command: ScriptedCommand::GizmoRelease,
            },
            Accept {
                line: "gizmo rotate",
                command: ScriptedCommand::GizmoTool(GizmoMode::Rotate),
            },
            Accept {
                line: "key w",
                command: ScriptedCommand::Key(unmodified_chord(KeyCode::KeyW)),
            },
            Accept {
                line: "key ctrl+z",
                command: ScriptedCommand::Key(KeyChord {
                    control: true,
                    ..unmodified_chord(KeyCode::KeyZ)
                }),
            },
            Accept {
                line: "key ctrl+shift+s",
                command: ScriptedCommand::Key(KeyChord {
                    control: true,
                    shift: true,
                    ..unmodified_chord(KeyCode::KeyS)
                }),
            },
            Accept {
                line: "key alt+enter",
                command: ScriptedCommand::Key(KeyChord {
                    alt: true,
                    ..unmodified_chord(KeyCode::Enter)
                }),
            },
            Accept {
                line: "key f2",
                command: ScriptedCommand::Key(unmodified_chord(KeyCode::F2)),
            },
            Accept {
                line: "key delete",
                command: ScriptedCommand::Key(unmodified_chord(KeyCode::Delete)),
            },
            Accept {
                line: "dump tabs",
                command: ScriptedCommand::Dump(Dump::Tabs),
            },
            Accept {
                line: "dump scenes",
                command: ScriptedCommand::Dump(Dump::Scenes),
            },
            Accept {
                line: "dump viewport",
                command: ScriptedCommand::Dump(Dump::Viewport),
            },
            Accept {
                line: "dump grid",
                command: ScriptedCommand::Dump(Dump::Grid),
            },
            Accept {
                line: "dump gizmos",
                command: ScriptedCommand::Dump(Dump::Gizmos),
            },
            Accept {
                line: "dump play",
                command: ScriptedCommand::Dump(Dump::Play),
            },
            Accept {
                line: "dump assets",
                command: ScriptedCommand::Dump(Dump::Assets),
            },
            Accept {
                line: "dump entities",
                command: ScriptedCommand::Dump(Dump::Entities),
            },
            Accept {
                line: "dump selection",
                command: ScriptedCommand::Dump(Dump::Selection),
            },
            Accept {
                line: "dump resources",
                command: ScriptedCommand::Dump(Dump::Resources),
            },
            Accept {
                line: "dump browser",
                command: ScriptedCommand::Dump(Dump::Browser),
            },
            Accept {
                line: "dump engine-assets",
                command: ScriptedCommand::Dump(Dump::EngineAssets),
            },
            Accept {
                line: "dump standalone-types",
                command: ScriptedCommand::Dump(Dump::StandaloneTypes),
            },
            Accept {
                line: "dump systems",
                command: ScriptedCommand::Dump(Dump::Systems),
            },
            Accept {
                line: "dump preferences",
                command: ScriptedCommand::Dump(Dump::Preferences),
            },
            Accept {
                line: "dump theme",
                command: ScriptedCommand::Dump(Dump::Theme),
            },
            Accept {
                line: "dump console",
                command: ScriptedCommand::Dump(Dump::Console),
            },
            Accept {
                line: "dump problems",
                command: ScriptedCommand::Dump(Dump::Problems),
            },
            Accept {
                line: "dump stats",
                command: ScriptedCommand::Dump(Dump::Stats),
            },
            Accept {
                line: "dump cadence",
                command: ScriptedCommand::Dump(Dump::Cadence),
            },
            Accept {
                line: "problem open 0",
                command: ScriptedCommand::ProblemOpen(0),
            },
            Accept {
                line: "safe_mode retry",
                command: ScriptedCommand::SafeModeRetry,
            },
            Accept {
                line: "panel unload pill_editor_shell",
                command: ScriptedCommand::PanelUnload("pill_editor_shell".to_string()),
            },
            Accept {
                line: "dump panels",
                command: ScriptedCommand::Dump(Dump::Panels),
            },
            Accept {
                line: "dump windows",
                command: ScriptedCommand::Dump(Dump::Windows),
            },
            Accept {
                line: "panel window 0 demo.confirm",
                command: ScriptedCommand::PanelWindow {
                    slot: 0,
                    id: "demo.confirm".to_string(),
                },
            },
            Accept {
                line: "panel field Spline control_points[0].x 2.5",
                command: ScriptedCommand::PanelField {
                    component: "Spline".to_string(),
                    field: "control_points[0].x".to_string(),
                    value: "2.5".to_string(),
                },
            },
            Accept {
                line: "console filter warn",
                command: ScriptedCommand::ConsoleFilter("warn".to_string()),
            },
            Accept {
                line: "console search frame heartbeat",
                command: ScriptedCommand::ConsoleSearch("frame heartbeat".to_string()),
            },
            Accept {
                line: "dump field 3v0 TransformComponent translation",
                command: ScriptedCommand::Dump(Dump::Field {
                    entity: "3v0".to_string(),
                    component: "TransformComponent".to_string(),
                    field: "translation".to_string(),
                }),
            },
            Accept {
                line: "browser textures",
                command: ScriptedCommand::Browser("textures".to_string()),
            },
            Accept {
                line: "browser my folder",
                command: ScriptedCommand::Browser("my folder".to_string()),
            },
            Accept {
                line: "camera orbit 10 -5",
                command: ScriptedCommand::Camera {
                    navigation: CameraNavigation::Orbit,
                    delta: (10.0, -5.0),
                },
            },
            Accept {
                line: "camera pan 10 -5",
                command: ScriptedCommand::Camera {
                    navigation: CameraNavigation::Pan,
                    delta: (10.0, -5.0),
                },
            },
            Accept {
                line: "camera dolly 3",
                command: ScriptedCommand::Camera {
                    navigation: CameraNavigation::Dolly,
                    delta: (0.0, 3.0),
                },
            },
            Accept {
                line: "camera zoom 2.5",
                command: ScriptedCommand::CameraZoom(2.5),
            },
            Accept {
                line: "hover 0 320 240",
                command: ScriptedCommand::Hover {
                    view: 0,
                    x: 320.0,
                    y: 240.0,
                },
            },
            Accept {
                line: "hover ui 640 312",
                command: ScriptedCommand::HoverUi { x: 640.0, y: 312.0 },
            },
            Accept {
                // Reserved for the shell's reference rows: the line arms the
                // row, and the next scene press names it. No suite drives it,
                // so this table is the only place the arm runs.
                line: "pick entity 3v1 ShowcaseComponent note",
                command: ScriptedCommand::PickEntity {
                    entity: "3v1".to_string(),
                    component: "ShowcaseComponent".to_string(),
                    field: "note".to_string(),
                },
            },
            Accept {
                line: "drop local/temp/a b.png",
                command: ScriptedCommand::Drop("local/temp/a b.png".to_string()),
            },
            Accept {
                line: "hover file local/temp/a b.png",
                command: ScriptedCommand::HoverFile("local/temp/a b.png".to_string()),
            },
            Accept {
                line: "select asset textures/wall.png",
                command: ScriptedCommand::SelectAsset("textures/wall.png".to_string()),
            },
            Accept {
                line: "select add 3v2",
                command: ScriptedCommand::SelectAdd("3v2".to_string()),
            },
            Accept {
                line: "select element 0,2",
                command: ScriptedCommand::SelectElement {
                    ids: vec![0, 2],
                    add: false,
                },
            },
            Accept {
                line: "select element add 7",
                command: ScriptedCommand::SelectElement {
                    ids: vec![7],
                    add: true,
                },
            },
            Accept {
                // Reserved for the shell's Entities list. The native runner
                // answers it, the browser editor refuses it by name, and no
                // suite drives it, so this table is the only place it runs.
                line: "select range 3v4",
                command: ScriptedCommand::SelectRange("3v4".to_string()),
            },
            Accept {
                line: "select resource lighting",
                command: ScriptedCommand::SelectResource("lighting".to_string()),
            },
            Accept {
                line: "select entity 3v1",
                command: ScriptedCommand::SelectEntity("3v1".to_string()),
            },
            Accept {
                line: "toolbar create",
                command: ScriptedCommand::Toolbar(ToolbarAction::Create),
            },
            Accept {
                line: "toolbar rename",
                command: ScriptedCommand::Toolbar(ToolbarAction::Rename),
            },
            Accept {
                line: "toolbar delete",
                command: ScriptedCommand::Toolbar(ToolbarAction::Delete),
            },
            Accept {
                line: "toolbar meta",
                command: ScriptedCommand::Toolbar(ToolbarAction::Meta),
            },
            Accept {
                line: "dialog set name s16_probe",
                command: ScriptedCommand::DialogSetName("s16_probe".to_string()),
            },
            Accept {
                line: "dialog set name two words",
                command: ScriptedCommand::DialogSetName("two words".to_string()),
            },
            Accept {
                line: "dialog submit",
                command: ScriptedCommand::DialogSubmit,
            },
            Accept {
                line: "edit asset create Texture res/wall wall",
                command: ScriptedCommand::EditAsset(AssetEdit::Create {
                    type_name: "Texture".to_string(),
                    folder: "res/wall".to_string(),
                    name: "wall".to_string(),
                }),
            },
            Accept {
                line: "edit asset delete textures/s16_probe.png",
                command: ScriptedCommand::EditAsset(AssetEdit::Delete {
                    path: "textures/s16_probe.png".to_string(),
                }),
            },
            Accept {
                line: "edit asset move textures/a.png models/a.png",
                command: ScriptedCommand::EditAsset(AssetEdit::Move {
                    from: "textures/a.png".to_string(),
                    to: "models/a.png".to_string(),
                }),
            },
            Accept {
                // A path in a position other than the last keeps its spaces
                // by being quoted. The destination is the trailing free text.
                line: "edit asset move \"res/my file.mat\" res/other.mat",
                command: ScriptedCommand::EditAsset(AssetEdit::Move {
                    from: "res/my file.mat".to_string(),
                    to: "res/other.mat".to_string(),
                }),
            },
            Accept {
                line: "edit asset create Texture \"res/my folder\" my name",
                command: ScriptedCommand::EditAsset(AssetEdit::Create {
                    type_name: "Texture".to_string(),
                    folder: "res/my folder".to_string(),
                    name: "my name".to_string(),
                }),
            },
            Accept {
                line: "scene open \"res/my scene.scene\"",
                command: ScriptedCommand::SceneOpen("res/my scene.scene".to_string()),
            },
            Accept {
                line: "edit asset meta textures/s16_stage.png",
                command: ScriptedCommand::EditAsset(AssetEdit::Meta {
                    path: "textures/s16_stage.png".to_string(),
                }),
            },
            Accept {
                line: "edit set 3v1 ShowcaseComponent lean 0.25",
                command: ScriptedCommand::EditSet {
                    entity: "3v1".to_string(),
                    component: "ShowcaseComponent".to_string(),
                    field: "lean".to_string(),
                    value: "0.25".to_string(),
                },
            },
            Accept {
                line: "edit set 3v1 ShowcaseComponent note hello world",
                command: ScriptedCommand::EditSet {
                    entity: "3v1".to_string(),
                    component: "ShowcaseComponent".to_string(),
                    field: "note".to_string(),
                    value: "hello world".to_string(),
                },
            },
            Accept {
                line: "edit selection ShowcaseComponent note two words",
                command: ScriptedCommand::EditSelection {
                    component: "ShowcaseComponent".to_string(),
                    field: "note".to_string(),
                    value: "two words".to_string(),
                },
            },
            Accept {
                line: "edit resource lighting intensity 1.5",
                command: ScriptedCommand::EditResource {
                    resource: "lighting".to_string(),
                    field: "intensity".to_string(),
                    value: "1.5".to_string(),
                },
            },
            Accept {
                line: "edit array 3v1 TransformComponent translation 1 2.5",
                command: ScriptedCommand::EditArray {
                    entity: "3v1".to_string(),
                    component: "TransformComponent".to_string(),
                    field: "translation".to_string(),
                    index: 1,
                    value: "2.5".to_string(),
                },
            },
            Accept {
                line: "edit asset-settings textures/wall.png sampling.filter linear",
                command: ScriptedCommand::EditAssetSettings {
                    path: "textures/wall.png".to_string(),
                    key: "sampling.filter".to_string(),
                    value: "linear".to_string(),
                },
            },
            Accept {
                line: "edit system 2 off",
                command: ScriptedCommand::EditSystem {
                    index: 2,
                    enabled: false,
                },
            },
            Accept {
                line: "edit preference keymap blender",
                command: ScriptedCommand::EditPreference {
                    key: "keymap".to_string(),
                    value: "blender".to_string(),
                },
            },
            Accept {
                line: "edit add 3v1 Color",
                command: ScriptedCommand::EditComponent {
                    entity: "3v1".to_string(),
                    component: "Color".to_string(),
                    add: true,
                },
            },
            Accept {
                line: "edit remove 3v1 Color",
                command: ScriptedCommand::EditComponent {
                    entity: "3v1".to_string(),
                    component: "Color".to_string(),
                    add: false,
                },
            },
            Accept {
                line: "command editor.undo",
                command: ScriptedCommand::Undo,
            },
            Accept {
                line: "command editor.redo",
                command: ScriptedCommand::Redo,
            },
            Accept {
                line: "command editor.pause_toggle",
                command: ScriptedCommand::PauseToggle,
            },
            Accept {
                line: "command editor.play",
                command: ScriptedCommand::Play,
            },
            Accept {
                line: "command editor.stop",
                command: ScriptedCommand::Stop,
            },
            Accept {
                line: "command editor.play_toggle",
                command: ScriptedCommand::PlayToggle,
            },
            Accept {
                line: "command editor.step",
                command: ScriptedCommand::Step,
            },
            Accept {
                line: "command editor.entity_create",
                command: ScriptedCommand::EntityCreate,
            },
            Accept {
                line: "command editor.entity_delete",
                command: ScriptedCommand::EntityDelete,
            },
            Accept {
                line: "command editor.tool_translate",
                command: ScriptedCommand::GizmoTool(GizmoMode::Translate),
            },
            Accept {
                line: "command editor.tool_rotate",
                command: ScriptedCommand::GizmoTool(GizmoMode::Rotate),
            },
            Accept {
                line: "command editor.tool_scale",
                command: ScriptedCommand::GizmoTool(GizmoMode::Scale),
            },
            Accept {
                line: "tool translate",
                command: ScriptedCommand::Tool(Some("translate".to_string())),
            },
            Accept {
                line: "tool none",
                command: ScriptedCommand::Tool(None),
            },
            Accept {
                line: "extensions on",
                command: ScriptedCommand::Extensions { enabled: true },
            },
            Accept {
                line: "extensions off",
                command: ScriptedCommand::Extensions { enabled: false },
            },
            Accept {
                line: "dump tools",
                command: ScriptedCommand::Dump(Dump::Tools),
            },
            Accept {
                line: "command editor.tool_snap_toggle",
                command: ScriptedCommand::SnapToggle,
            },
            Accept {
                line: "command editor.tool_space_toggle",
                command: ScriptedCommand::SpaceToggle,
            },
        ];
        for case in accepted {
            assert_eq!(
                ScriptedCommand::parse(case.line),
                Ok(case.command),
                "parsing {:?}",
                case.line
            );
        }
    }

    #[test]
    fn quoted_and_padded_values_keep_their_text() {
        // One surrounding pair of quotes is removed, escapes are resolved,
        // and the value's own spaces survive. That is why the text form
        // exists.
        assert_eq!(
            ScriptedCommand::parse("edit set 3v1 C f \"  padded  \""),
            Ok(ScriptedCommand::EditSet {
                entity: "3v1".to_string(),
                component: "C".to_string(),
                field: "f".to_string(),
                value: "  padded  ".to_string(),
            })
        );
        assert_eq!(
            ScriptedCommand::parse(r#"edit preference note "say \"hi\"""#),
            Ok(ScriptedCommand::EditPreference {
                key: "note".to_string(),
                value: "say \"hi\"".to_string(),
            })
        );
    }

    #[test]
    fn malformed_lines_are_refused_with_a_reason() {
        let refused = vec![
            Refuse {
                line: "",
                reason: "the line is empty",
            },
            Refuse {
                line: "nonsense-verb",
                reason: "unknown verb 'nonsense-verb'",
            },
            Refuse {
                line: "undock nowhere",
                reason: "undock needs a tab",
            },
            Refuse {
                line: "click 10",
                reason: "click needs an x and a y",
            },
            Refuse {
                line: "click 10 20 30",
                reason: "click takes no more arguments, got '30'",
            },
            Refuse {
                line: "gizmo",
                reason: "gizmo needs a tool or a drag",
            },
            Refuse {
                line: "gizmo skew",
                reason: "unknown gizmo target",
            },
            Refuse {
                line: "gizmo drag x 10",
                reason: "gizmo drag needs an axis or plane and two pixel deltas",
            },
            Refuse {
                line: "key ctrl+",
                reason: "key needs a chord, like ctrl+z or w",
            },
            Refuse {
                line: "key hyper+z",
                reason: "key chord modifier 'hyper' is not ctrl, shift, alt or super",
            },
            Refuse {
                line: "dump nothing",
                reason: "unknown dump target",
            },
            Refuse {
                line: "dump field 3v0 C",
                reason: "dump field needs an entity, a component and a field",
            },
            Refuse {
                line: "select nothing",
                reason: "unknown select target",
            },
            Refuse {
                line: "toolbar explode",
                reason: "toolbar needs create, rename, delete or meta",
            },
            Refuse {
                line: "dialog open create",
                reason: "dialog needs open, set, submit or cancel",
            },
            Refuse {
                line: "edit nothing",
                reason: "unknown edit target",
            },
            Refuse {
                line: "edit array 3v1 C f x 1",
                reason: "edit array needs a numeric index, got 'x'",
            },
            Refuse {
                line: "edit set 3v1 C f",
                reason: "edit set needs a value",
            },
            Refuse {
                line: "command",
                reason: "command needs an id",
            },
            Refuse {
                line: "command editor.undo entity 3v1",
                reason: "command takes no more arguments, got 'entity 3v1'",
            },
            Refuse {
                line: "command pill_test.run viewport 2",
                reason: "command takes `entity <name>` or `asset <path>`, got 'viewport'",
            },
            Refuse {
                line: "command pill_test.run entity",
                reason: "command entity needs an entity name",
            },
            Refuse {
                line: "camera spin 1",
                reason: "camera needs orbit, pan, dolly or zoom",
            },
            Refuse {
                line: "scene open",
                reason: "scene open needs a .scene path",
            },
            Refuse {
                line: "edit asset move \"res/my file.mat",
                reason: "path opens a quote it never closes",
            },
        ];
        for case in refused {
            let error = ScriptedCommand::parse(case.line)
                .expect_err(&format!("{:?} should be refused", case.line));
            assert_eq!(error.reason, case.reason, "refusing {:?}", case.line);
        }
    }

    #[test]
    fn a_modules_command_parses_with_its_target() {
        assert_eq!(
            ScriptedCommand::parse("command pill_test.run").expect("a plain invocation"),
            ScriptedCommand::Command {
                id: "pill_test.run".to_string(),
                target: ScriptedTarget::None,
            }
        );
        assert_eq!(
            ScriptedCommand::parse("command pill_test.run entity the mesh").expect("an entity"),
            ScriptedCommand::Command {
                id: "pill_test.run".to_string(),
                target: ScriptedTarget::Entity("the mesh".to_string()),
            }
        );
        assert_eq!(
            ScriptedCommand::parse("command pill_test.run asset res/a.scene").expect("an asset"),
            ScriptedCommand::Command {
                id: "pill_test.run".to_string(),
                target: ScriptedTarget::Asset("res/a.scene".to_string()),
            }
        );
        // A built-in keeps its own verb, so the executable's own actions do
        // not turn into an invocation.
        assert_eq!(
            ScriptedCommand::parse("command editor.undo").expect("a built-in"),
            ScriptedCommand::Undo
        );
    }

    #[test]
    fn the_keys_the_suites_press_are_named() {
        // The chord table is the API a suite reads. Renaming an entry here
        // breaks a scripted line, so the mapping is fixed.
        assert_eq!(key_code("a"), Some(KeyCode::KeyA));
        assert_eq!(key_code("Z"), Some(KeyCode::KeyZ));
        assert_eq!(key_code("7"), Some(KeyCode::Digit7));
        assert_eq!(key_code("esc"), Some(KeyCode::Escape));
        assert_eq!(key_code("f24"), Some(KeyCode::F24));
        assert_eq!(key_code("f25"), None);
        assert_eq!(key_code("-"), None);
        // A chord's label round-trips through the parser.
        let chord = match ScriptedCommand::parse("key ctrl+alt+k").expect("chord") {
            ScriptedCommand::Key(chord) => chord,
            other => panic!("{other:?} is not a key"),
        };
        assert_eq!(chord.label(), "ctrl+alt+k");
        // A plain letter types its character; a command chord does not.
        assert_eq!(unmodified_chord(KeyCode::KeyW).typed_character(), Some('w'));
        assert_eq!(
            KeyChord {
                control: true,
                ..unmodified_chord(KeyCode::KeyW)
            }
            .typed_character(),
            None
        );
    }
}
