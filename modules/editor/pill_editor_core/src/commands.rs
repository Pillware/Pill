//! The editor's command registry: named actions, their menus and their
//! rebindable shortcuts.
//!
//! # Responsibilities
//!
//! - Model a command ([`CommandSpec`]) and the registry that owns every
//!   registration ([`CommandRegistry`]), grouped by the registering module.
//! - Resolve effective shortcuts from the active keymap preset plus the
//!   keybindings file's overrides, and report every chord that two commands
//!   answer to.
//! - Parse and format a chord as the `ctrl+shift+p` text the keybindings file
//!   uses ([`Chord`], [`KeyName`]).
//! - Model the keybindings and preferences files ([`KeybindingsFile`],
//!   [`EditorPreferences`]) and read and write the gizmo settings a project
//!   starts with ([`read_gizmo_settings`], [`write_gizmo_preferences`]).
//!
//! # Design
//!
//! The registry lives in a world resource as plain data, because the
//! executable, the reloadable shell and any editor crate all read or write it.
//! The executable resolves a chord and applies the batches the shell queued;
//! the shell paints the menus and queues commands; an editor crate adds its
//! own commands with [`CommandRegistry::register`]. A module that re-registers
//! its owner replaces that owner's previous set, so a reload never leaves
//! duplicate commands behind. Chords are this crate's own serde types rather
//! than the engine's input types, so the files stay stable when the engine's
//! key vocabulary changes. The conversions to and from
//! [`pill_engine::KeyCode`] live in [`KeyName`].

// Standard library
use std::collections::BTreeMap;
use std::str::FromStr;

// External crates
use serde::{Deserialize, Serialize};

// Current crate
use crate::drain::HistoryDirection;
use crate::editor_state::EditorCommand;
use crate::layout::EditorTab;

// =============================================================================
// Keys and chords
// =============================================================================

/// The key vocabulary command bindings use.
///
/// Deliberately small: it holds only the keys a preset or a user binding can
/// name. The conversion to and from the engine's [`pill_engine::KeyCode`] sits
/// in the impl below, so the two lists cannot fall out of step quietly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyName {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Space,
    Escape,
    Enter,
    Delete,
    Left,
    Right,
    Up,
    Down,
    F3,
    F5,
    F6,
    F10,
    Numpad1,
    Numpad3,
    Numpad7,
}

impl KeyName {
    /// Returns the engine key this name stands for. Every name has one, so
    /// the result is always `Some`.
    pub fn engine_key(self) -> Option<pill_engine::KeyCode> {
        use pill_engine::KeyCode as K;
        Some(match self {
            Self::A => K::KeyA,
            Self::B => K::KeyB,
            Self::C => K::KeyC,
            Self::D => K::KeyD,
            Self::E => K::KeyE,
            Self::F => K::KeyF,
            Self::G => K::KeyG,
            Self::H => K::KeyH,
            Self::I => K::KeyI,
            Self::J => K::KeyJ,
            Self::K => K::KeyK,
            Self::L => K::KeyL,
            Self::M => K::KeyM,
            Self::N => K::KeyN,
            Self::O => K::KeyO,
            Self::P => K::KeyP,
            Self::Q => K::KeyQ,
            Self::R => K::KeyR,
            Self::S => K::KeyS,
            Self::T => K::KeyT,
            Self::U => K::KeyU,
            Self::V => K::KeyV,
            Self::W => K::KeyW,
            Self::X => K::KeyX,
            Self::Y => K::KeyY,
            Self::Z => K::KeyZ,
            Self::Space => K::Space,
            Self::Escape => K::Escape,
            Self::Enter => K::Enter,
            Self::Delete => K::Delete,
            Self::Left => K::ArrowLeft,
            Self::Right => K::ArrowRight,
            Self::Up => K::ArrowUp,
            Self::Down => K::ArrowDown,
            Self::F3 => K::F3,
            Self::F5 => K::F5,
            Self::F6 => K::F6,
            Self::F10 => K::F10,
            Self::Numpad1 => K::Numpad1,
            Self::Numpad3 => K::Numpad3,
            Self::Numpad7 => K::Numpad7,
        })
    }

    /// Returns this vocabulary's name for an engine key, or `None` when it
    /// names no such key. Input events are turned into chords through this.
    pub fn from_engine(key: pill_engine::KeyCode) -> Option<Self> {
        use pill_engine::KeyCode as K;
        Some(match key {
            K::KeyA => Self::A,
            K::KeyB => Self::B,
            K::KeyC => Self::C,
            K::KeyD => Self::D,
            K::KeyE => Self::E,
            K::KeyF => Self::F,
            K::KeyG => Self::G,
            K::KeyH => Self::H,
            K::KeyI => Self::I,
            K::KeyJ => Self::J,
            K::KeyK => Self::K,
            K::KeyL => Self::L,
            K::KeyM => Self::M,
            K::KeyN => Self::N,
            K::KeyO => Self::O,
            K::KeyP => Self::P,
            K::KeyQ => Self::Q,
            K::KeyR => Self::R,
            K::KeyS => Self::S,
            K::KeyT => Self::T,
            K::KeyU => Self::U,
            K::KeyV => Self::V,
            K::KeyW => Self::W,
            K::KeyX => Self::X,
            K::KeyY => Self::Y,
            K::KeyZ => Self::Z,
            K::Space => Self::Space,
            K::Escape => Self::Escape,
            K::Enter => Self::Enter,
            K::Delete => Self::Delete,
            K::ArrowLeft => Self::Left,
            K::ArrowRight => Self::Right,
            K::ArrowUp => Self::Up,
            K::ArrowDown => Self::Down,
            K::F3 => Self::F3,
            K::F5 => Self::F5,
            K::F6 => Self::F6,
            K::F10 => Self::F10,
            K::Numpad1 => Self::Numpad1,
            K::Numpad3 => Self::Numpad3,
            K::Numpad7 => Self::Numpad7,
            _ => return None,
        })
    }
}

/// One shortcut: a key plus the modifiers that must be held with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Chord {
    /// Whether the control key is part of the chord.
    pub ctrl: bool,
    /// Whether the shift key is part of the chord.
    pub shift: bool,
    /// Whether the alt key is part of the chord.
    pub alt: bool,
    /// The key itself.
    pub key: KeyName,
}

impl Chord {
    /// Builds a chord with no modifiers.
    pub fn plain(key: KeyName) -> Self {
        Self {
            ctrl: false,
            shift: false,
            alt: false,
            key,
        }
    }

    /// Builds a chord from a key and the modifiers held with it.
    pub fn with(key: KeyName, ctrl: bool, shift: bool, alt: bool) -> Self {
        Self {
            ctrl,
            shift,
            alt,
            key,
        }
    }

    /// Returns whether this chord matches a key press with these modifiers.
    pub fn matches(&self, key: pill_engine::KeyCode, modifiers: pill_engine::Modifiers) -> bool {
        KeyName::from_engine(key) == Some(self.key)
            && self.ctrl == modifiers.control
            && self.shift == modifiers.shift
            && self.alt == modifiers.alt
    }
}

impl std::fmt::Display for KeyName {
    /// Writes the lowercase key name the files use (`space`, `f10`,
    /// `numpad1`).
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", format!("{self:?}").to_lowercase())
    }
}

impl std::fmt::Display for Chord {
    /// Writes the `ctrl+shift+p` form: the syntax the keybindings file uses,
    /// and what the conflict log prints.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ctrl {
            write!(formatter, "ctrl+")?;
        }
        if self.shift {
            write!(formatter, "shift+")?;
        }
        if self.alt {
            write!(formatter, "alt+")?;
        }
        write!(formatter, "{}", self.key)
    }
}

/// The reason a chord or key name could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChordParseError(pub String);

impl std::fmt::Display for ChordParseError {
    /// Writes the reason the parse failed.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl FromStr for Chord {
    type Err = ChordParseError;

    /// Parses `[ctrl+][shift+][alt+]key`, ignoring case. `control` is
    /// accepted for `ctrl`.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut chord = Chord::plain(KeyName::Escape);
        let mut key_seen = false;
        for part in text.split('+') {
            let part = part.trim();
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => chord.ctrl = true,
                "shift" => chord.shift = true,
                "alt" => chord.alt = true,
                _ => {
                    chord.key = KeyName::from_str(part).map_err(|_| {
                        ChordParseError(format!("unknown key `{part}` in `{text}`"))
                    })?;
                    key_seen = true;
                }
            }
        }
        if !key_seen {
            return Err(ChordParseError(format!("no key in `{text}`")));
        }
        Ok(chord)
    }
}

impl FromStr for KeyName {
    type Err = ChordParseError;

    /// Parses a key name, ignoring case. `esc` and `del` are aliases, and
    /// `return` stands for enter.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let lower = text.to_ascii_lowercase();
        let named_key = match lower.as_str() {
            "space" => Some(Self::Space),
            "escape" | "esc" => Some(Self::Escape),
            "enter" | "return" => Some(Self::Enter),
            "delete" | "del" => Some(Self::Delete),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "f3" => Some(Self::F3),
            "f5" => Some(Self::F5),
            "f6" => Some(Self::F6),
            "f10" => Some(Self::F10),
            "numpad1" => Some(Self::Numpad1),
            "numpad3" => Some(Self::Numpad3),
            "numpad7" => Some(Self::Numpad7),
            other => {
                let mut characters = other.chars();
                match (characters.next(), characters.next()) {
                    (Some(letter), None) if letter.is_ascii_lowercase() => {
                        LETTERS.get(letter as usize - 'a' as usize).copied()
                    }
                    _ => None,
                }
            }
        };
        named_key.ok_or_else(|| ChordParseError(format!("unknown key `{text}`")))
    }
}

/// The letter keys in alphabetical order. A single letter parses by its index
/// here.
const LETTERS: [KeyName; 26] = [
    KeyName::A,
    KeyName::B,
    KeyName::C,
    KeyName::D,
    KeyName::E,
    KeyName::F,
    KeyName::G,
    KeyName::H,
    KeyName::I,
    KeyName::J,
    KeyName::K,
    KeyName::L,
    KeyName::M,
    KeyName::N,
    KeyName::O,
    KeyName::P,
    KeyName::Q,
    KeyName::R,
    KeyName::S,
    KeyName::T,
    KeyName::U,
    KeyName::V,
    KeyName::W,
    KeyName::X,
    KeyName::Y,
    KeyName::Z,
];

// =============================================================================
// Presets
// =============================================================================

/// The keymap presets the editor ships.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeymapPreset {
    /// The default preset: Blender-like.
    #[default]
    Blender,
    /// Unity-like shortcuts.
    Unity,
    /// Godot-like shortcuts.
    Godot,
    /// Unreal-Engine-like shortcuts.
    Unreal,
}

impl std::fmt::Display for KeymapPreset {
    /// Writes the preset's name as the files spell it (`blender`, `unity`,
    /// `godot`, `unreal`).
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Blender => "blender",
            Self::Unity => "unity",
            Self::Godot => "godot",
            Self::Unreal => "unreal",
        };
        write!(formatter, "{name}")
    }
}

// =============================================================================
// Commands
// =============================================================================

/// One registered command.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandSpec {
    /// Stable identifier (`editor.pause_toggle`).
    pub id: String,
    /// The title menus and the palette show.
    pub title: String,
    /// Menu path (`["Edit", "Undo"]`); an empty path means no menu bar row.
    pub menu_path: Vec<String>,
    /// The default binding per preset; a preset that is absent leaves the
    /// command unbound in it.
    pub shortcuts: BTreeMap<KeymapPreset, Vec<Chord>>,
    /// Whether the shortcut is a global binding. A global binding is meant to
    /// fire even while a text field has focus, where every other shortcut
    /// yields to the field.
    pub global: bool,
    /// Whether a menu draws a rule above this command's row, grouping it
    /// with the rows after it.
    pub separator_before: bool,
}

impl CommandSpec {
    /// Builds a command with a title and no menu or bindings yet.
    pub fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            menu_path: Vec::new(),
            shortcuts: BTreeMap::new(),
            global: false,
            separator_before: false,
        }
    }

    /// Adds a menu path.
    pub fn menu(mut self, path: &[&str]) -> Self {
        self.menu_path = path.iter().map(|segment| segment.to_string()).collect();
        self
    }

    /// Marks that a rule is drawn above this command's menu row.
    pub fn separator(mut self) -> Self {
        self.separator_before = true;
        self
    }

    /// Adds one default chord for a preset.
    ///
    /// # Panics
    ///
    /// Panics when `chord_text` is not valid chord text. Every caller is in
    /// this file (the built-in table and its tests), so a bad string is a
    /// mistake here rather than input to handle.
    pub fn bind(mut self, preset: KeymapPreset, chord_text: &str) -> Self {
        let chord = chord_text
            .parse::<Chord>()
            .unwrap_or_else(|error| panic!("built-in chord `{chord_text}` is invalid: {error}"));
        self.shortcuts.entry(preset).or_default().push(chord);
        self
    }

    /// Makes the shortcut global: it is meant to fire even while a text field
    /// has focus.
    pub fn global(mut self) -> Self {
        self.global = true;
        self
    }
}

/// Every command every owner registered, plus the preset and the file
/// overrides that decide each command's chords.
///
/// A world resource (inserted by [`CommandRegistry::ensure`]); owners are
/// keyed by the module that registered them so a re-registration replaces
/// exactly that owner's set.
#[derive(Clone, Debug, Default)]
pub struct CommandRegistry {
    owners: BTreeMap<String, Vec<CommandSpec>>,
    overrides: BTreeMap<String, Vec<Chord>>,
    preset: KeymapPreset,
}

impl CommandRegistry {
    /// Builds a registry holding only the built-in commands, under owner
    /// `editor`.
    pub fn with_builtins() -> Self {
        let mut registry = Self {
            owners: BTreeMap::new(),
            overrides: BTreeMap::new(),
            preset: KeymapPreset::default(),
        };
        registry.register("editor", builtin_commands());
        registry
    }

    /// Registers the registry resource in `world` and inserts the built-ins
    /// when it is absent.
    pub fn ensure(world: &mut pill_engine::World) {
        world.register_resource::<CommandRegistry>();
        if world.get_resource::<CommandRegistry>().is_none() {
            world.insert_resource(Self::with_builtins());
        }
    }

    /// Replaces `owner`'s registrations with `commands`.
    pub fn register(&mut self, owner: &str, commands: Vec<CommandSpec>) {
        self.owners.insert(owner.to_string(), commands);
    }

    /// Iterates every command, owners in key order; the built-in commands
    /// belong to owner `editor`.
    pub fn commands(&self) -> impl Iterator<Item = &CommandSpec> {
        self.owners.values().flatten()
    }

    /// Returns the command with this id, whichever owner registered it.
    pub fn command(&self, id: &str) -> Option<&CommandSpec> {
        self.commands().find(|spec| spec.id == id)
    }

    /// Returns the active preset.
    pub fn preset(&self) -> KeymapPreset {
        self.preset
    }

    /// Returns how many commands the keybindings file's overrides name; the
    /// `[editor] keybindings applied` log line prints it.
    pub fn override_count(&self) -> usize {
        self.overrides.len()
    }

    /// Returns the chords a command currently answers to: the file's override
    /// when it names this command (an empty list means unbound), or the
    /// preset's defaults otherwise.
    pub fn effective_chords(&self, id: &str) -> Vec<Chord> {
        if let Some(chords) = self.overrides.get(id) {
            return chords.clone();
        }
        self.command(id)
            .and_then(|spec| spec.shortcuts.get(&self.preset).cloned())
            .unwrap_or_default()
    }

    /// Applies a keybindings file: switches to its preset when it names one,
    /// replaces the overrides with its own, and returns every problem found
    /// for the caller to report.
    ///
    /// An override for a command nothing has registered is kept, so it starts
    /// working once that id exists.
    pub fn apply_file(&mut self, file: &KeybindingsFile) -> Vec<String> {
        let mut problems = Vec::new();
        // Step 1: switch to the file's preset when it names one.
        if let Some(preset) = file.preset {
            self.preset = preset;
        }
        // Step 2: replace every override with the file's own list.
        self.overrides.clear();
        for (id, chord_texts) in &file.overrides {
            if self.command(id).is_none() {
                problems.push(format!(
                    "keybindings: override for unknown command `{id}` (kept; it binds once that id exists)"
                ));
            }
            let mut chords = Vec::new();
            for text in chord_texts {
                match text.parse::<Chord>() {
                    Ok(chord) => chords.push(chord),
                    Err(error) => problems.push(format!("keybindings: {error}")),
                }
            }
            self.overrides.insert(id.clone(), chords);
        }
        problems
    }

    /// Returns the command a key press runs, or `None` when no command is
    /// bound to it.
    ///
    /// The first command in the registry's order wins when two share a chord;
    /// [`Self::conflicts`] reports the collision.
    pub fn resolve(
        &self,
        key: pill_engine::KeyCode,
        modifiers: pill_engine::Modifiers,
    ) -> Option<&CommandSpec> {
        self.commands().find(|spec| {
            self.effective_chords(&spec.id)
                .iter()
                .any(|chord| chord.matches(key, modifiers))
        })
    }

    /// Returns every chord bound to more than one command, with the ids
    /// involved.
    pub fn conflicts(&self) -> Vec<(Chord, Vec<String>)> {
        let mut by_chord: BTreeMap<Chord, Vec<String>> = BTreeMap::new();
        for spec in self.commands() {
            for chord in self.effective_chords(&spec.id) {
                by_chord.entry(chord).or_default().push(spec.id.clone());
            }
        }
        by_chord
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .collect()
    }

    /// Returns the commands as plain data, in the registry's order. The
    /// frame's snapshot carries these rows, so the menus and the palette build
    /// themselves without touching the world.
    pub fn views(&self) -> Vec<CommandView> {
        self.commands()
            .map(|spec| CommandView {
                id: spec.id.clone(),
                title: spec.title.clone(),
                menu_path: spec.menu_path.clone(),
                separator_before: spec.separator_before,
            })
            .collect()
    }
}

/// One registered command as the UI sees it.
///
/// It is a copy, not a borrow. The frame's snapshot keeps these rows after the
/// world borrow ends, and a row's label and presence must come from the
/// registry that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandView {
    /// Stable identifier (`editor.pause_toggle`).
    pub id: String,
    /// The title a menu row shows.
    pub title: String,
    /// The menu path; an empty path means no menu row.
    pub menu_path: Vec<String>,
    /// Whether a rule is drawn above the row.
    pub separator_before: bool,
}

impl pill_engine::Resource for CommandRegistry {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

/// Groups the registry's commands by their top-level menu, in command order.
/// Commands with no menu path are left out.
pub fn menu_tree(registry: &CommandRegistry) -> BTreeMap<String, Vec<&CommandSpec>> {
    let mut menus: BTreeMap<String, Vec<&CommandSpec>> = BTreeMap::new();
    for spec in registry.commands() {
        let Some(top_menu) = spec.menu_path.first() else {
            continue;
        };
        menus.entry(top_menu.clone()).or_default().push(spec);
    }
    menus
}

/// The built-in commands every editor has (owner `editor`).
///
/// This table is what the menu bar draws. A row exists because a command
/// registers a menu path. Its label is the command's title, except for the
/// gizmo's space row, which shows the space the gizmo is in. A line between
/// two groups is drawn for the spec marked [`CommandSpec::separator`].
/// Removing a spec here removes its row.
fn builtin_commands() -> Vec<CommandSpec> {
    use KeymapPreset::{Blender, Godot, Unity, Unreal};
    let mut commands = vec![
        // The layout commands carry no menu row: `Menu::ALL` has no Layout
        // menu, so a menu path here would describe a row the bar can never
        // draw.
        CommandSpec::new("editor.save_layout", "Save Layout")
            .bind(Blender, "ctrl+alt+s")
            .bind(Unity, "ctrl+shift+l")
            .bind(Godot, "ctrl+alt+l")
            .bind(Unreal, "ctrl+shift+l"),
        CommandSpec::new("editor.reset_layout", "Reset Layout")
            .bind(Blender, "ctrl+alt+r")
            .bind(Unity, "alt+r")
            .bind(Godot, "ctrl+alt+r")
            .bind(Unreal, "alt+r"),
        // The pop-out pair: Pop Out moves a tab into its own window, and
        // Redock takes every tab of that window back into the main dock. The
        // tab's own context menu is the only surface that knows which tab, so
        // neither command takes a menu path or a chord; the shell draws that
        // menu itself.
        CommandSpec::new("editor.popout_tab", "Pop Out Panel"),
        CommandSpec::new("editor.redock_tab", "Redock Popped Panels"),
        CommandSpec::new("editor.command_palette", "Command Palette")
            .bind(Blender, "f3")
            .bind(Unity, "ctrl+shift+p")
            .bind(Godot, "ctrl+shift+p")
            .bind(Unreal, "ctrl+shift+p")
            .global(),
        CommandSpec::new("editor.undo", "Undo")
            .menu(&["Edit", "Undo"])
            .bind(Blender, "ctrl+z")
            .bind(Unity, "ctrl+z")
            .bind(Godot, "ctrl+z")
            .bind(Unreal, "ctrl+z")
            .global(),
        CommandSpec::new("editor.redo", "Redo")
            .menu(&["Edit", "Redo"])
            .bind(Blender, "ctrl+shift+z")
            .bind(Unity, "ctrl+y")
            .bind(Godot, "ctrl+shift+z")
            .bind(Unreal, "ctrl+y")
            .global(),
        // The clipboard commands are not global on purpose: while a widget
        // holds the focus (a text field), ctrl+c/x/v belong to the field's
        // own Edit handling.
        CommandSpec::new("editor.copy", "Copy")
            .menu(&["Edit", "Copy"])
            .bind(Blender, "ctrl+c")
            .bind(Unity, "ctrl+c")
            .bind(Godot, "ctrl+c")
            .bind(Unreal, "ctrl+c"),
        CommandSpec::new("editor.cut", "Cut")
            .menu(&["Edit", "Cut"])
            .bind(Blender, "ctrl+x")
            .bind(Unity, "ctrl+x")
            .bind(Godot, "ctrl+x")
            .bind(Unreal, "ctrl+x"),
        CommandSpec::new("editor.paste", "Paste")
            .menu(&["Edit", "Paste"])
            .bind(Blender, "ctrl+v")
            .bind(Unity, "ctrl+v")
            .bind(Godot, "ctrl+v")
            .bind(Unreal, "ctrl+v"),
        // The transform tools: the chords are the ones Unity and
        // Unreal already use, and Blender's own grab/rotate/scale keys. The
        // titles are the toolbox's row labels, so the View menu, the panel and
        // the palette show the same word for a tool.
        CommandSpec::new("editor.tool_translate", "Translate")
            .menu(&["View", "Translate"])
            .bind(Blender, "g")
            .bind(Unity, "w")
            .bind(Godot, "w")
            .bind(Unreal, "w"),
        CommandSpec::new("editor.tool_rotate", "Rotate")
            .menu(&["View", "Rotate"])
            .bind(Blender, "r")
            .bind(Unity, "e")
            .bind(Godot, "e")
            .bind(Unreal, "e"),
        CommandSpec::new("editor.tool_scale", "Scale")
            .menu(&["View", "Scale"])
            .bind(Blender, "s")
            .bind(Unity, "r")
            .bind(Godot, "r")
            .bind(Unreal, "r"),
        // Unbound on purpose: the space toggle appears in the View menu, the
        // palette and the toolbox, and no preset has a free chord that would
        // also mean "local". The title names both spaces; the menu row shows
        // the space the gizmo is in now, so the menu model replaces the title
        // with that label.
        CommandSpec::new("editor.tool_space_toggle", "Local / World Space")
            .menu(&["View", "Local / World Space"])
            .separator(),
        CommandSpec::new("editor.tool_snap_toggle", "Snap")
            .menu(&["View", "Snap"])
            .bind(Blender, "shift+x")
            .bind(Unity, "ctrl+shift+x")
            .bind(Godot, "ctrl+shift+x")
            .bind(Unreal, "ctrl+shift+x"),
        CommandSpec::new("editor.grid_toggle", "Grid")
            .menu(&["View", "Grid"])
            .separator(),
        // The simulation controls close the View menu, behind a rule. Play
        // opens the group and its title follows the run mode.
        CommandSpec::new("editor.play_toggle", "Play / Stop Simulation")
            .menu(&["View", "Play / Stop Simulation"])
            .separator()
            .bind(Blender, "ctrl+p")
            .bind(Unity, "ctrl+p")
            .bind(Godot, "ctrl+p")
            .bind(Unreal, "ctrl+p"),
        // Ctrl+P belongs to Play now, and Ctrl+Shift+P opens the palette in
        // every preset but Blender's, so Pause takes Ctrl+Alt+P. Blender keeps
        // Space, which is its own play key.
        CommandSpec::new("editor.pause_toggle", "Pause / Resume Simulation")
            .menu(&["View", "Pause / Resume Simulation"])
            .bind(Blender, "space")
            .bind(Unity, "ctrl+alt+p")
            .bind(Godot, "ctrl+alt+p")
            .bind(Unreal, "ctrl+alt+p"),
        CommandSpec::new("editor.step", "Step One Frame")
            .menu(&["View", "Step One Frame"])
            .bind(Blender, "right")
            .bind(Unity, "f10")
            .bind(Godot, "f6")
            .bind(Unreal, "f10"),
        // The Entities tab's actions: creation makes one empty entity, and
        // deletion clears the selection in the same step. No chords: the menu,
        // the palette and the tab's context menu carry them.
        CommandSpec::new("editor.entity_create", "New Entity").menu(&["Create", "New Entity"]),
        CommandSpec::new("editor.entity_delete", "Delete Entity")
            .menu(&["Edit", "Delete Entity"])
            .separator(),
        // The rows that open and write the scene document. Open acts on the
        // scene selected in the Assets tab. Save As carries no chord, like the
        // other chordless rows, because the menu and the palette reach it.
        CommandSpec::new("editor.scene_new", "Create Scene")
            .menu(&["Create", "Create Scene"])
            .bind(Blender, "ctrl+n")
            .bind(Unity, "ctrl+n")
            .bind(Godot, "ctrl+n")
            .bind(Unreal, "ctrl+n"),
        CommandSpec::new("editor.scene_open", "Open Scene").menu(&["File", "Open Scene"]),
        CommandSpec::new("editor.scene_save", "Save Scene")
            .menu(&["File", "Save Scene"])
            .bind(Blender, "ctrl+s")
            .bind(Unity, "ctrl+s")
            .bind(Godot, "ctrl+s")
            .bind(Unreal, "ctrl+s"),
        CommandSpec::new("editor.scene_save_as", "Save Scene As")
            .menu(&["File", "Save Scene As"])
            .separator(),
        // The Assets dialogs' buttons. Create keeps its menu row beside the
        // entity and scene creates; Rename has none, because it acts on the file
        // whose dialog opened it.
        CommandSpec::new("editor.asset.create", "Create Asset...")
            .menu(&["Create", "Create Asset..."]),
        CommandSpec::new("editor.asset.rename", "Rename Asset"),
        // File's trailing rows: the sidecar write, the Preferences tab and
        // quitting.
        CommandSpec::new("editor.asset_write_meta", "Write Asset Meta")
            .menu(&["File", "Write Asset Meta"]),
        CommandSpec::new("editor.preferences_open", "Editor preferences")
            .menu(&["File", "Editor preferences"])
            .separator(),
        CommandSpec::new("editor.quit", "Quit Editor")
            .menu(&["File", "Quit Editor"])
            .separator(),
        // Help's one row.
        CommandSpec::new("editor.about", "About Pill Editor").menu(&["Help", "About Pill Editor"]),
        // One more scene view, ahead of the Window menu's tab rows: no chord,
        // because the menu and the palette carry it.
        CommandSpec::new("editor.scene_new_view", "New Scene View")
            .menu(&["Window", "New Scene View"])
            .separator(),
    ];
    // Window: one row per built-in tab, in `EditorTab::ALL` order, and the id
    // names the tab it activates. A scene view's title carries its number
    // (`Scene`, `Scene 2`, ...), and a module panel has no row.
    for tab in EditorTab::ALL {
        let title = tab.title();
        commands.push(
            CommandSpec::new(&format!("editor.window_{}", tab.name()), title.as_str())
                .menu(&["Window", title.as_str()]),
        );
    }
    commands
}

// =============================================================================
// Files
// =============================================================================

/// The keybindings file: the active preset and the per-command chord
/// overrides.
///
/// Shape: `{ "preset": "unity", "overrides": { "editor.pause_toggle": ["space"] } }`.
/// A command missing from `overrides` keeps its preset binding; a command
/// listed with an empty array is unbound.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KeybindingsFile {
    /// The preset to activate; absent keeps the current one.
    #[serde(default)]
    pub preset: Option<KeymapPreset>,
    /// Per-command chord lists replacing the preset's bindings.
    #[serde(default)]
    pub overrides: BTreeMap<String, Vec<String>>,
}

/// The typed view of the preferences file. It holds the keys the executable
/// reads as values, which are the theme layer, the gizmo entry, the snapping
/// steps, the code editor command and the history limits.
///
/// The Preferences tab reads and writes the same document key by key through
/// [`crate::preferences`]; the two owners only ever change their own keys.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct EditorPreferences {
    /// The user theme layer's file, absolute or relative to the state
    /// directory.
    #[serde(default)]
    pub theme: Option<String>,
    /// Transform snapping steps, in world units and degrees.
    #[serde(default)]
    pub snapping_steps: Option<SnappingSteps>,
    /// The gizmo tool's persisted state.
    #[serde(default)]
    pub gizmo: Option<GizmoPreferences>,
    /// The command that opens the user's code editor.
    #[serde(default)]
    pub code_editor_command: Option<String>,
    /// Autosave interval for the editor state, in seconds, as the file
    /// stores it.
    #[serde(default)]
    pub autosave_interval_seconds: Option<f64>,
    /// Maximum undo steps kept (absent uses the history default, 500).
    #[serde(default)]
    pub history_step_cap: Option<usize>,
    /// Undo memory budget in megabytes (absent uses the default, 256).
    #[serde(default)]
    pub history_memory_budget_mb: Option<usize>,
}

/// Snapping steps the gizmo's drags use.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct SnappingSteps {
    /// Translation step in world units.
    pub translation: f32,
    /// Rotation step in degrees.
    pub rotation_degrees: f32,
    /// Scale step, as a factor increment.
    #[serde(default = "default_scale_step")]
    pub scale: f32,
}

/// The scale step a preferences file without one uses.
fn default_scale_step() -> f32 {
    0.1
}

/// The gizmo tool's persisted state.
///
/// The top-level fields hold the last-used values; `projects` remembers a
/// choice per project, keyed by the project directory's name, so moving
/// between projects brings each one's tool back.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GizmoPreferences {
    /// The last active mode, when one was chosen.
    #[serde(default)]
    pub mode: Option<crate::gizmo::GizmoMode>,
    /// The last active space, when one was chosen.
    #[serde(default)]
    pub space: Option<crate::gizmo::GizmoSpace>,
    /// Whether dragging snapped, when the toggle was used.
    #[serde(default)]
    pub snap: Option<bool>,
    /// The component gizmo types the editor hides.
    #[serde(default)]
    pub hidden_types: Vec<String>,
    /// Per-project overrides, by project directory name.
    #[serde(default)]
    pub projects: BTreeMap<String, GizmoProjectPreferences>,
}

/// One project's gizmo choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GizmoProjectPreferences {
    /// The project's last active mode.
    #[serde(default)]
    pub mode: Option<crate::gizmo::GizmoMode>,
    /// The project's last active space.
    #[serde(default)]
    pub space: Option<crate::gizmo::GizmoSpace>,
    /// The project's last snapping toggle.
    #[serde(default)]
    pub snap: Option<bool>,
}

/// The preferences file's name, next to the layout file.
///
/// Declared here rather than in an editor frontend: the executable reads the
/// file and applies the shell's save commands to it, and both sides link this
/// crate.
pub const PREFERENCES_FILE: &str = "preferences.yaml";

/// The keybindings file's name, next to the layout file.
///
/// The executable watches it and applies every change through
/// [`CommandRegistry::apply_file`]; the preset and the overrides it holds
/// decide what each shortcut does.
pub const KEYBINDINGS_FILE: &str = "keybindings.yaml";

/// Reads the gizmo settings a project starts a session with.
///
/// The file's last-used values are applied first, then the project's own
/// remembered entry, then the defaults for anything neither sets. The snapping
/// steps come from the file alone, one set for every project. A missing or
/// unreadable file gives the defaults and never an error. The tool has to work
/// before anyone edits preferences.
pub fn read_gizmo_settings(
    directory: &std::path::Path,
    project: &str,
) -> crate::gizmo::GizmoSettings {
    // Step 1: read the file, treating a missing or unreadable one as empty.
    let preferences =
        crate::editor_files::read_yaml::<EditorPreferences>(&directory.join(PREFERENCES_FILE))
            .ok()
            .flatten()
            .unwrap_or_default();
    let mut settings = crate::gizmo::GizmoSettings::default();
    if let Some(steps) = preferences.snapping_steps {
        settings.translate_step = steps.translation;
        settings.rotate_step_degrees = steps.rotation_degrees;
        settings.scale_step = steps.scale;
    }
    let Some(gizmo) = &preferences.gizmo else {
        return settings;
    };
    // Step 2: take the file's last-used values.
    if let Some(mode) = gizmo.mode {
        settings.mode = mode;
    }
    if let Some(space) = gizmo.space {
        settings.space = space;
    }
    if let Some(snap) = gizmo.snap {
        settings.snap = snap;
    }
    // Step 3: let this project's own entry win.
    if let Some(project_entry) = gizmo.projects.get(project) {
        if let Some(mode) = project_entry.mode {
            settings.mode = mode;
        }
        if let Some(space) = project_entry.space {
            settings.space = space;
        }
        if let Some(snap) = project_entry.snap {
            settings.snap = snap;
        }
    }
    settings
}

/// Writes one project's gizmo choices into the preferences file.
///
/// This reads the document, changes only the gizmo keys and writes it back,
/// so every other key survives. The top-level fields hold the values written
/// last, which a project without an entry of its own starts from; the
/// project's own entry is what its next session starts with. The write goes
/// through a temporary file and a rename, so the live reload reader never sees
/// a half-written document.
///
/// # Errors
///
/// Returns a message when the file cannot be read, is not a YAML mapping, or
/// cannot be written back.
pub fn write_gizmo_preferences(
    directory: &std::path::Path,
    project: &str,
    settings: &crate::gizmo::GizmoSettings,
) -> Result<(), String> {
    // Edit the document rather than writing a new one. A document that cannot
    // be read stops the save; overwriting it with one holding only `gizmo`
    // would erase every other key.
    crate::preferences::update(directory, |document| {
        let mut gizmo = document
            .get("gizmo")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        gizmo.insert("mode".to_string(), serde_json::json!(settings.mode));
        gizmo.insert("space".to_string(), serde_json::json!(settings.space));
        gizmo.insert("snap".to_string(), serde_json::json!(settings.snap));
        let mut projects = gizmo
            .get("projects")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        projects.insert(
            project.to_string(),
            serde_json::json!({
                "mode": settings.mode,
                "space": settings.space,
                "snap": settings.snap,
            }),
        );
        gizmo.insert("projects".to_string(), serde_json::Value::Object(projects));
        document.insert("gizmo".to_string(), serde_json::Value::Object(gizmo));
        Ok(())
    })
}

/// Writes the component gizmo types the editor hides into the preferences
/// file.
///
/// This reads the document, changes only the `gizmo.hidden_types` key and
/// writes it back, so every other key survives. The list is what a reload and
/// the next session start from, which is why the executable owns the write.
///
/// # Errors
///
/// Returns a message when the file cannot be read, is not a YAML mapping, or
/// cannot be written back.
pub fn write_gizmo_hidden_types(
    directory: &std::path::Path,
    hidden_types: &[String],
) -> Result<(), String> {
    crate::preferences::update(directory, |document| {
        let mut gizmo = document
            .get("gizmo")
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        gizmo.insert(
            "hidden_types".to_string(),
            serde_json::Value::Array(
                hidden_types
                    .iter()
                    .map(|type_name| serde_json::Value::String(type_name.clone()))
                    .collect(),
            ),
        );
        document.insert("gizmo".to_string(), serde_json::Value::Object(gizmo));
        Ok(())
    })
}

/// The world resource that carries a frame's executed command ids to the
/// modules that own them.
///
/// A module reads its own ids in a system of its own. The shell cannot call
/// into a module's code, and a reloadable module must not hand function
/// pointers across the reload boundary.
#[derive(Clone, Debug, Default)]
pub struct CommandInbox {
    /// Command ids executed this frame, in execution order.
    pub executed: Vec<String>,
}

impl pill_engine::Resource for CommandInbox {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

/// The world resource holding the engine mutations queued by command
/// handlers.
///
/// The shell has no `Engine`, so its handlers queue [`EditorCommand`]s
/// instead; the executable applies each batch at the frame boundary through
/// [`crate::drain::apply_pending`].
#[derive(Clone, Debug, Default)]
pub struct PendingEditorCommands {
    /// The queued batches, drained by the executable.
    pub batches: Vec<PendingCommandBatch>,
    /// Asset keys the shell asked the executable to show in the platform's
    /// file manager, in request order.
    ///
    /// They are not commands: showing a file changes nothing in the world, so
    /// there is nothing to undo. The shell also cannot reach the asset root;
    /// only the executable knows where the project lives and which file
    /// manager the OS has.
    pub reveals: Vec<String>,
    /// Keys the shell asked the editor to show in the Assets panel, in request
    /// order. Not commands either: moving a selection changes nothing in the
    /// world, and only the executable owns the panel's open folder.
    pub focuses: Vec<String>,
}

impl PendingEditorCommands {
    /// Queues a batch that is applied but never recorded as an undo step
    /// (pause, step, a scripted command that is not an edit).
    pub fn push(&mut self, commands: Vec<EditorCommand>) {
        self.batches.push(PendingCommandBatch {
            commands,
            record: false,
            label: String::new(),
            coalesce_key: None,
            replay: None,
        });
    }

    /// Queues the batch that replays one step of the undo history.
    ///
    /// The batch keeps `direction` because the step is popped before its
    /// commands run. When a replay fails, the executable puts the step back on
    /// the side it came from; without that, the history would claim an edit
    /// that never happened.
    pub fn push_replay(&mut self, commands: Vec<EditorCommand>, direction: HistoryDirection) {
        self.batches.push(PendingCommandBatch {
            commands,
            record: false,
            label: String::new(),
            coalesce_key: None,
            replay: Some(direction),
        });
    }

    /// Asks the executable to show `path` in the platform's file manager.
    /// `path` is an asset key, relative to the asset root that holds it (the
    /// project's `res` or a module's resources).
    pub fn reveal(&mut self, path: impl Into<String>) {
        self.reveals.push(path.into());
    }

    /// Takes the reveal requests that are waiting for the executable.
    pub fn take_reveals(&mut self) -> Vec<String> {
        std::mem::take(&mut self.reveals)
    }

    /// Asks the editor to show `key` in the Assets panel: the selection moves
    /// to the asset and the panel opens the folder that holds it.
    pub fn focus_asset(&mut self, key: impl Into<String>) {
        self.focuses.push(key.into());
    }

    /// Takes the focus requests that are waiting for the executable.
    pub fn take_focuses(&mut self) -> Vec<String> {
        std::mem::take(&mut self.focuses)
    }

    /// Queues an edit the history should record as one undo step.
    ///
    /// `coalesce_key` merges continuous edits (a slider drag) that arrive
    /// back to back under the same key into the previous step.
    pub fn push_recorded(
        &mut self,
        commands: Vec<EditorCommand>,
        label: impl Into<String>,
        coalesce_key: Option<String>,
    ) {
        self.batches.push(PendingCommandBatch {
            commands,
            record: true,
            label: label.into(),
            coalesce_key,
            replay: None,
        });
    }
}

/// One queued batch and what the executable does with it at the frame
/// boundary.
#[derive(Clone, Debug)]
pub struct PendingCommandBatch {
    /// The commands to apply, in order.
    pub commands: Vec<EditorCommand>,
    /// Whether the batch is an edit the history records.
    pub record: bool,
    /// History label, when recorded.
    pub label: String,
    /// Continuous-edit key, when recorded.
    pub coalesce_key: Option<String>,
    /// The history move this batch replays, when it is one.
    pub replay: Option<HistoryDirection>,
}

impl pill_engine::Resource for PendingEditorCommands {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> pill_engine::resource::PlayState {
        pill_engine::resource::PlayState::Keep
    }
}

/// Inserts every command resource the editor relies on, keeping the values a
/// previous generation set.
pub fn ensure_command_resources(world: &mut pill_engine::World) {
    CommandRegistry::ensure(world);
    world.register_resource::<CommandInbox>();
    if world.get_resource::<CommandInbox>().is_none() {
        world.insert_resource(CommandInbox::default());
    }
    world.register_resource::<PendingEditorCommands>();
    if world.get_resource::<PendingEditorCommands>().is_none() {
        world.insert_resource(PendingEditorCommands::default());
    }
    world.register_resource::<crate::editor_state::EntityPickRequest>();
    if world
        .get_resource::<crate::editor_state::EntityPickRequest>()
        .is_none()
    {
        world.insert_resource(crate::editor_state::EntityPickRequest::default());
    }
    crate::history::CommandHistory::ensure(world);
    crate::scene_workflow::SceneSession::ensure(world);
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The gizmo choices round-trip through the preferences file, and the
    /// keys another writer owns survive.
    #[test]
    fn gizmo_preferences_round_trip_and_preserve_other_keys() {
        let directory = std::env::temp_dir().join(format!(
            "ecs-gizmo-preferences-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(PREFERENCES_FILE),
            "theme: dark\nhistory_step_cap: 42\n",
        )
        .unwrap();
        let settings = crate::gizmo::GizmoSettings {
            mode: crate::gizmo::GizmoMode::Rotate,
            space: crate::gizmo::GizmoSpace::Local,
            snap: true,
            ..crate::gizmo::GizmoSettings::default()
        };
        write_gizmo_preferences(&directory, "italian_brainrot", &settings).unwrap();
        // A second project's write keeps the first project's entry.
        let second_project_settings = crate::gizmo::GizmoSettings {
            mode: crate::gizmo::GizmoMode::Scale,
            ..crate::gizmo::GizmoSettings::default()
        };
        write_gizmo_preferences(&directory, "project_rs", &second_project_settings).unwrap();
        let preferences_bytes = std::fs::read(directory.join(PREFERENCES_FILE)).unwrap();
        let document: serde_json::Value =
            crate::editor_files::parse_yaml(&preferences_bytes).unwrap();
        assert_eq!(document["theme"], "dark");
        assert_eq!(document["history_step_cap"], 42);
        assert_eq!(
            document["gizmo"]["projects"]["italian_brainrot"]["mode"],
            "rotate"
        );
        assert_eq!(document["gizmo"]["projects"]["project_rs"]["mode"], "scale");

        let read_back_settings = read_gizmo_settings(&directory, "italian_brainrot");
        assert_eq!(read_back_settings.mode, crate::gizmo::GizmoMode::Rotate);
        assert_eq!(read_back_settings.space, crate::gizmo::GizmoSpace::Local);
        assert!(read_back_settings.snap);
        // A project the file never saw keeps the last-used values.
        let unknown_project_settings = read_gizmo_settings(&directory, "circus_demo");
        assert_eq!(
            unknown_project_settings.mode,
            crate::gizmo::GizmoMode::Scale
        );
        // And no file at all is the defaults.
        let empty_directory = std::env::temp_dir().join("ecs-gizmo-nonexistent-dir");
        let default_settings = read_gizmo_settings(&empty_directory, "anything");
        assert_eq!(default_settings, crate::gizmo::GizmoSettings::default());
        // A profile whose layout was never saved has no directory yet: the
        // write creates it instead of failing.
        let fresh_directory = directory.join("unsaved-profile");
        write_gizmo_preferences(&fresh_directory, "italian_brainrot", &settings).unwrap();
        assert_eq!(
            read_gizmo_settings(&fresh_directory, "italian_brainrot").mode,
            crate::gizmo::GizmoMode::Rotate
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn chords_parse_and_format_round_trip() {
        let chord: Chord = "ctrl+shift+p".parse().unwrap();
        assert!(chord.ctrl && chord.shift && !chord.alt);
        assert_eq!(chord.key, KeyName::P);
        assert_eq!(chord.to_string(), "ctrl+shift+p");
        let plain_chord: Chord = "space".parse().unwrap();
        assert_eq!(plain_chord, Chord::plain(KeyName::Space));
        assert!("ctrl+".parse::<Chord>().is_err());
        assert!("ctrl+bogus".parse::<Chord>().is_err());
        assert!("hyper+x".parse::<Chord>().is_err());
    }

    #[test]
    fn presets_rebind_builtins_and_overrides_win() {
        let mut registry = CommandRegistry::with_builtins();
        let pause_command_id = "editor.pause_toggle";
        // Blender is the default: Space pauses.
        assert_eq!(
            registry.effective_chords(pause_command_id),
            vec![Chord::plain(KeyName::Space)]
        );
        // Switching the preset rebinds without touching any command. Unity
        // gives Ctrl+P to Play, so Pause answers to Ctrl+Alt+P there.
        let keybindings: KeybindingsFile =
            serde_json::from_str(r#"{ "preset": "unity" }"#).unwrap();
        assert!(registry.apply_file(&keybindings).is_empty());
        assert_eq!(
            registry.effective_chords(pause_command_id),
            vec![Chord::with(KeyName::P, true, false, true)]
        );
        assert_eq!(
            registry.effective_chords("editor.play_toggle"),
            vec![Chord::with(KeyName::P, true, false, false)]
        );
        // A single override wins on top of the preset.
        let keybindings: KeybindingsFile = serde_json::from_str(
            r#"{ "preset": "unity", "overrides": { "editor.pause_toggle": ["ctrl+j"] } }"#,
        )
        .unwrap();
        assert!(registry.apply_file(&keybindings).is_empty());
        assert_eq!(
            registry.effective_chords(pause_command_id),
            vec![Chord::with(KeyName::J, true, false, false)]
        );
        // An empty list unbinds it.
        let keybindings: KeybindingsFile =
            serde_json::from_str(r#"{ "overrides": { "editor.pause_toggle": [] } }"#).unwrap();
        registry.apply_file(&keybindings);
        assert!(registry.effective_chords(pause_command_id).is_empty());
    }

    #[test]
    fn conflicts_and_unknown_overrides_are_reported() {
        let mut registry = CommandRegistry::with_builtins();
        let keybindings: KeybindingsFile = serde_json::from_str(
            r#"{ "overrides": { "editor.pause_toggle": ["f10"], "editor.nope": ["x"] } }"#,
        )
        .unwrap();
        let problems = registry.apply_file(&keybindings);
        assert!(problems
            .iter()
            .any(|problem| problem.contains("editor.nope")));
        // The second file replaces the first one's overrides. Step now takes
        // space, which the Blender preset still gives to pause, so both
        // commands answer to one chord.
        let keybindings: KeybindingsFile =
            serde_json::from_str(r#"{ "overrides": { "editor.step": ["space"] } }"#).unwrap();
        assert!(registry.apply_file(&keybindings).is_empty());
        let conflicts = registry.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, Chord::plain(KeyName::Space));
        assert!(conflicts[0].1.contains(&"editor.pause_toggle".to_string()));
        assert!(conflicts[0].1.contains(&"editor.step".to_string()));
    }

    #[test]
    fn owners_replace_their_own_set_only() {
        let mut registry = CommandRegistry::with_builtins();
        registry.register(
            "pill_gadget_editor",
            vec![
                CommandSpec::new("spline_editor.reset_offset", "Reset Spline Offset")
                    .menu(&["Spline", "Reset Offset"])
                    .bind(KeymapPreset::Blender, "ctrl+j"),
            ],
        );
        assert!(registry.command("spline_editor.reset_offset").is_some());
        assert!(registry.command("editor.pause_toggle").is_some());
        // The menu tree lists it under its top segment, beside the built-ins.
        let menus = menu_tree(&registry);
        assert!(menus.contains_key("Spline"));
        assert!(menus.contains_key("View"));
        assert!(!menus.contains_key("editor.command_palette"));
        // The next generation re-registers; nothing duplicates.
        registry.register(
            "pill_gadget_editor",
            vec![CommandSpec::new(
                "spline_editor.reset_offset",
                "Reset Spline Offset",
            )],
        );
        assert_eq!(
            registry
                .commands()
                .filter(|spec| spec.id.starts_with("spline_editor."))
                .count(),
            1
        );
    }

    #[test]
    fn resolve_finds_the_first_matching_command() {
        let registry = CommandRegistry::with_builtins();
        let modifiers = pill_engine::Modifiers::default();
        let resolved_command = registry.resolve(pill_engine::KeyCode::Space, modifiers);
        assert_eq!(
            resolved_command.map(|spec| spec.id.as_str()),
            Some("editor.pause_toggle")
        );
        assert!(registry
            .resolve(pill_engine::KeyCode::KeyQ, modifiers)
            .is_none());
    }
}
