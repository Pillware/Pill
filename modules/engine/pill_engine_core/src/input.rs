//! Keyboard, mouse and gamepad state for the current frame, maintained by the
//! engine and readable from any system.
//!
//! # Responsibilities
//!
//! - Defines [`Input`], the engine-owned resource, and the plain-data types a
//!   frontend feeds it ([`InputEvent`], [`KeyCode`], and the gamepad and mouse
//!   types).
//! - Tracks what is held this frame, what was pressed or released since the
//!   previous frame, and the cursor, motion and scroll collected over the
//!   frame. Also queues rumble requests for the frontend that owns the
//!   gamepads.
//!
//! # Design
//!
//! The engine knows no windowing or gamepad library. A frontend translates its
//! platform's events (winit, the editor's DOM, gilrs) into [`InputEvent`]s and
//! queues them with
//! [`Engine::push_input_event`](crate::engine::Engine::push_input_event). The
//! engine applies the queue at the top of the next frame, just after it
//! advances [`Time`](crate::time::Time). Both happen there for the same reason:
//! the scheduler orders systems on its own, without regard to registration
//! order, so a separate "input system" could run after the systems that read
//! its data. Applying the queue at the frame start means every system sees the
//! same input.
//!
//! The type lives in the engine core, the one shared dylib every artifact
//! links. A project DLL therefore reads the host's instance through
//! `Res<Input>` across a reload.
//!
//! Key codes name physical keys, using the W3C `KeyboardEvent.code` vocabulary
//! that winit also uses. They do not name the characters a layout produces:
//! `KeyW` is the key left of `KeyE` on every layout, which is what movement
//! bindings want. Text arrives separately, through `InputEvent::TextInput`,
//! `ImeCommit` and `ImePreedit`.
//!
//! In frame terms, `*_held` was down at the start of the frame, and `*_pressed`
//! and `*_released` changed since the previous frame. A press and a release
//! inside one frame report both edges, so a short tap is never lost. Operating
//! system key repeat reports no new press. Motion, scroll and committed text
//! add up over the frame. The cursor position, modifier state and IME
//! composition are the latest reported values.

// Standard library
use std::time::Duration;

// External crates
use pill_core::math::Vector2f;

// Current crate
use crate::resource::Resource;

// =============================================================================
// Constants
// =============================================================================

/// Number of gamepads tracked at once, one per [`PlayerId`].
pub const GAMEPAD_SLOT_COUNT: usize = 4;

/// Axis values closer to zero than this read as exactly zero.
///
/// Resting sticks rarely report a clean zero, and without a deadzone a
/// character drifts while nobody touches the pad.
pub const GAMEPAD_AXIS_DEADZONE: f32 = 0.05;

/// Number of 64-bit words in the key bit set. Their 256 bits have room for
/// every [`KeyCode`].
const KEY_SET_WORD_COUNT: usize = 4;

// =============================================================================
// KeyCode
// =============================================================================

/// Declares [`KeyCode`] with each key's W3C `KeyboardEvent.code` name.
///
/// One list produces the enum, [`KeyCode::ALL`], [`KeyCode::code_name`] and
/// [`KeyCode::from_code_name`], so the four cannot drift apart.
macro_rules! key_codes {
    ($($variant:ident => $code_name:literal),* $(,)?) => {
        /// A physical key, named after its position on a US layout.
        ///
        /// The variants match winit's `KeyCode`, and their
        /// [`code_name`](Self::code_name)s are the W3C `KeyboardEvent.code`
        /// values a browser reports.
        #[repr(u8)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum KeyCode {
            $($variant),*
        }

        impl KeyCode {
            /// Every key, in declaration order.
            pub const ALL: &'static [KeyCode] = &[$(KeyCode::$variant),*];

            /// The W3C `KeyboardEvent.code` name of this key.
            pub const fn code_name(self) -> &'static str {
                match self {
                    $(KeyCode::$variant => $code_name),*
                }
            }

            /// The key a W3C `KeyboardEvent.code` value names, or `None` for
            /// a code the engine does not know.
            ///
            /// Also accepts `OSLeft`/`OSRight`, which older Firefox versions
            /// report for the Windows/Command keys.
            pub fn from_code_name(code_name: &str) -> Option<KeyCode> {
                match code_name {
                    $($code_name => Some(KeyCode::$variant),)*
                    "OSLeft" => Some(KeyCode::SuperLeft),
                    "OSRight" => Some(KeyCode::SuperRight),
                    _ => None,
                }
            }
        }
    };
}

key_codes! {
    // Writing system keys.
    Backquote => "Backquote", Backslash => "Backslash", BracketLeft => "BracketLeft",
    BracketRight => "BracketRight", Comma => "Comma",
    Digit0 => "Digit0", Digit1 => "Digit1", Digit2 => "Digit2", Digit3 => "Digit3",
    Digit4 => "Digit4", Digit5 => "Digit5", Digit6 => "Digit6", Digit7 => "Digit7",
    Digit8 => "Digit8", Digit9 => "Digit9",
    Equal => "Equal", IntlBackslash => "IntlBackslash", IntlRo => "IntlRo", IntlYen => "IntlYen",
    KeyA => "KeyA", KeyB => "KeyB", KeyC => "KeyC", KeyD => "KeyD", KeyE => "KeyE",
    KeyF => "KeyF", KeyG => "KeyG", KeyH => "KeyH", KeyI => "KeyI", KeyJ => "KeyJ",
    KeyK => "KeyK", KeyL => "KeyL", KeyM => "KeyM", KeyN => "KeyN", KeyO => "KeyO",
    KeyP => "KeyP", KeyQ => "KeyQ", KeyR => "KeyR", KeyS => "KeyS", KeyT => "KeyT",
    KeyU => "KeyU", KeyV => "KeyV", KeyW => "KeyW", KeyX => "KeyX", KeyY => "KeyY",
    KeyZ => "KeyZ",
    Minus => "Minus", Period => "Period", Quote => "Quote", Semicolon => "Semicolon",
    Slash => "Slash",
    // Functional keys.
    AltLeft => "AltLeft", AltRight => "AltRight", Backspace => "Backspace",
    CapsLock => "CapsLock", ContextMenu => "ContextMenu", ControlLeft => "ControlLeft",
    ControlRight => "ControlRight", Enter => "Enter", SuperLeft => "MetaLeft",
    SuperRight => "MetaRight", ShiftLeft => "ShiftLeft", ShiftRight => "ShiftRight",
    Space => "Space", Tab => "Tab",
    Convert => "Convert", KanaMode => "KanaMode", Lang1 => "Lang1", Lang2 => "Lang2",
    Lang3 => "Lang3", Lang4 => "Lang4", Lang5 => "Lang5", NonConvert => "NonConvert",
    // Control pad and arrows.
    Delete => "Delete", End => "End", Help => "Help", Home => "Home", Insert => "Insert",
    PageDown => "PageDown", PageUp => "PageUp",
    ArrowDown => "ArrowDown", ArrowLeft => "ArrowLeft", ArrowRight => "ArrowRight",
    ArrowUp => "ArrowUp",
    // Numpad.
    NumLock => "NumLock",
    Numpad0 => "Numpad0", Numpad1 => "Numpad1", Numpad2 => "Numpad2", Numpad3 => "Numpad3",
    Numpad4 => "Numpad4", Numpad5 => "Numpad5", Numpad6 => "Numpad6", Numpad7 => "Numpad7",
    Numpad8 => "Numpad8", Numpad9 => "Numpad9",
    NumpadAdd => "NumpadAdd", NumpadBackspace => "NumpadBackspace",
    NumpadClear => "NumpadClear", NumpadClearEntry => "NumpadClearEntry",
    NumpadComma => "NumpadComma", NumpadDecimal => "NumpadDecimal",
    NumpadDivide => "NumpadDivide", NumpadEnter => "NumpadEnter", NumpadEqual => "NumpadEqual",
    NumpadHash => "NumpadHash", NumpadMemoryAdd => "NumpadMemoryAdd",
    NumpadMemoryClear => "NumpadMemoryClear", NumpadMemoryRecall => "NumpadMemoryRecall",
    NumpadMemoryStore => "NumpadMemoryStore", NumpadMemorySubtract => "NumpadMemorySubtract",
    NumpadMultiply => "NumpadMultiply", NumpadParenLeft => "NumpadParenLeft",
    NumpadParenRight => "NumpadParenRight", NumpadStar => "NumpadStar",
    NumpadSubtract => "NumpadSubtract",
    // Function section.
    Escape => "Escape", Fn => "Fn", FnLock => "FnLock", PrintScreen => "PrintScreen",
    ScrollLock => "ScrollLock", Pause => "Pause",
    // Media and browser keys.
    BrowserBack => "BrowserBack", BrowserFavorites => "BrowserFavorites",
    BrowserForward => "BrowserForward", BrowserHome => "BrowserHome",
    BrowserRefresh => "BrowserRefresh", BrowserSearch => "BrowserSearch",
    BrowserStop => "BrowserStop", Eject => "Eject", LaunchApp1 => "LaunchApp1",
    LaunchApp2 => "LaunchApp2", LaunchMail => "LaunchMail", MediaPlayPause => "MediaPlayPause",
    MediaSelect => "MediaSelect", MediaStop => "MediaStop", MediaTrackNext => "MediaTrackNext",
    MediaTrackPrevious => "MediaTrackPrevious", Power => "Power", Sleep => "Sleep",
    AudioVolumeDown => "AudioVolumeDown", AudioVolumeMute => "AudioVolumeMute",
    AudioVolumeUp => "AudioVolumeUp", WakeUp => "WakeUp",
    // Legacy and non-standard keys.
    Meta => "Meta", Hyper => "Hyper", Turbo => "Turbo", Abort => "Abort", Resume => "Resume",
    Suspend => "Suspend", Again => "Again", Copy => "Copy", Cut => "Cut", Find => "Find",
    Open => "Open", Paste => "Paste", Props => "Props", Select => "Select", Undo => "Undo",
    Hiragana => "Hiragana", Katakana => "Katakana",
    // Function keys.
    F1 => "F1", F2 => "F2", F3 => "F3", F4 => "F4", F5 => "F5", F6 => "F6", F7 => "F7",
    F8 => "F8", F9 => "F9", F10 => "F10", F11 => "F11", F12 => "F12", F13 => "F13",
    F14 => "F14", F15 => "F15", F16 => "F16", F17 => "F17", F18 => "F18", F19 => "F19",
    F20 => "F20", F21 => "F21", F22 => "F22", F23 => "F23", F24 => "F24", F25 => "F25",
    F26 => "F26", F27 => "F27", F28 => "F28", F29 => "F29", F30 => "F30", F31 => "F31",
    F32 => "F32", F33 => "F33", F34 => "F34", F35 => "F35",
}

// Every key needs a bit in the key set.
const _: () = assert!(KeyCode::ALL.len() <= KEY_SET_WORD_COUNT * 64);

// =============================================================================
// Buttons, Axes and Players
// =============================================================================

/// Whether a key or button went down or came up.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ButtonState {
    /// The key or button went down.
    Pressed,
    /// The key or button came up.
    Released,
}

/// A mouse button.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    /// The primary button, usually the left one.
    Left,
    /// The secondary button, usually the right one.
    Right,
    /// The wheel button.
    Middle,
    /// The "browser back" side button.
    Back,
    /// The "browser forward" side button.
    Forward,
}

impl MouseButton {
    /// Every mouse button, in declaration order.
    pub const ALL: [MouseButton; 5] = [
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::Back,
        MouseButton::Forward,
    ];

    /// This button's bit mask.
    const fn bit_mask(self) -> u8 {
        1 << self as u8
    }
}

/// A gamepad button, named after the Xbox layout.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum GamepadButton {
    /// The bottom face button (Xbox A, PlayStation Cross).
    A,
    /// The right face button (Xbox B, PlayStation Circle).
    B,
    /// The left face button (Xbox X, PlayStation Square).
    X,
    /// The top face button (Xbox Y, PlayStation Triangle).
    Y,
    /// The left shoulder button.
    LeftBumper,
    /// The right shoulder button.
    RightBumper,
    /// The left trigger, as a button; its travel is [`GamepadAxis::LeftTrigger`].
    LeftTrigger,
    /// The right trigger, as a button; its travel is [`GamepadAxis::RightTrigger`].
    RightTrigger,
    /// The left menu button (Back, View, Select, Share).
    Back,
    /// The right menu button (Start, Menu, Options).
    Start,
    /// The guide button (Xbox, PlayStation).
    Mode,
    /// Pressing the left stick in.
    LeftStick,
    /// Pressing the right stick in.
    RightStick,
    /// D-pad up.
    DPadUp,
    /// D-pad down.
    DPadDown,
    /// D-pad left.
    DPadLeft,
    /// D-pad right.
    DPadRight,
}

impl GamepadButton {
    /// This button's bit mask.
    const fn bit_mask(self) -> u32 {
        1 << self as u8
    }
}

/// A gamepad axis.
///
/// Stick axes range over `-1.0..=1.0`, with up and right positive. Trigger axes
/// range over `0.0..=1.0`.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum GamepadAxis {
    /// Left stick, horizontal.
    LeftStickX,
    /// Left stick, vertical.
    LeftStickY,
    /// Right stick, horizontal.
    RightStickX,
    /// Right stick, vertical.
    RightStickY,
    /// Left trigger travel.
    LeftTrigger,
    /// Right trigger travel.
    RightTrigger,
    /// D-pad as an axis, on pads that report it as one.
    DPadX,
    /// D-pad as an axis, on pads that report it as one.
    DPadY,
}

impl GamepadAxis {
    /// Number of axes.
    pub const COUNT: usize = GamepadAxis::DPadY as usize + 1;
}

/// The player a gamepad belongs to.
///
/// Gamepads take the lowest free slot when they connect, and keep it until
/// they disconnect.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PlayerId {
    /// The first slot.
    Player1,
    /// The second slot.
    Player2,
    /// The third slot.
    Player3,
    /// The fourth slot.
    Player4,
}

impl PlayerId {
    /// Every slot, in order.
    pub const ALL: [PlayerId; GAMEPAD_SLOT_COUNT] = [
        PlayerId::Player1,
        PlayerId::Player2,
        PlayerId::Player3,
        PlayerId::Player4,
    ];

    /// The slot at `index`, or `None` past the last one.
    pub const fn from_index(index: usize) -> Option<PlayerId> {
        if index < GAMEPAD_SLOT_COUNT {
            Some(Self::ALL[index])
        } else {
            None
        }
    }

    /// This slot's zero-based index.
    pub const fn index(self) -> usize {
        self as usize
    }
}

// =============================================================================
// Events and Requests
// =============================================================================

/// How far a mouse wheel turned.
///
/// Positive `y` scrolls up (away from the user), positive `x` scrolls right.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ScrollDelta {
    /// Notched wheels: lines (or rows) to scroll.
    Lines(Vector2f),
    /// Touchpads and smooth wheels: physical pixels to scroll.
    Pixels(Vector2f),
}

/// The modifier keys' state, as the platform reports it.
///
/// This is separate from the physical modifier keys ([`KeyCode::ShiftLeft`]
/// and the other modifier codes). A platform merges both shifts, both controls
/// and AltGr into this one state, which is what shortcut matching needs.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    /// Whether a shift key is down.
    pub shift: bool,
    /// Whether a control key is down.
    pub control: bool,
    /// Whether an alt key is down.
    pub alt: bool,
    /// Whether a Windows/Command key is down.
    pub super_key: bool,
}

/// One input change, as a frontend reports it.
///
/// Cursor positions and wheel amounts are in physical pixels. A cursor position
/// is measured from the top-left corner of the area the game is drawn in. Raw
/// mouse motion has no pixel unit: the platform reports it in the device's own
/// units and does not scale it.
///
/// The text variants carry owned strings, so the enum is not `Copy`. Every
/// event is still plain data, safe to pass across a module boundary.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    /// A key went down or came up.
    Key {
        /// Which key.
        key: KeyCode,
        /// Down or up.
        state: ButtonState,
    },
    /// A held key repeated, as the operating system reports it.
    ///
    /// A repeat is not a new press or release. [`Input`] leaves the held and
    /// edge state alone. Frontends deliver repeats so text fields and
    /// navigation can respond to a held key. A game that ignores this event
    /// sees no change.
    KeyRepeat {
        /// Which key.
        key: KeyCode,
    },
    /// The modifier keys' state changed.
    ModifiersChanged {
        /// The new state, as the platform reports it.
        modifiers: Modifiers,
    },
    /// Text committed by typing. Usually one character, or a short run for a
    /// key that produces several.
    TextInput {
        /// The committed text.
        text: String,
    },
    /// The IME's composition text changed.
    ///
    /// An empty `text` means no composition is active.
    ImePreedit {
        /// The composition text.
        text: String,
        /// The platform's composing cursor range within `text`, as byte
        /// offsets, when it reports one.
        cursor: Option<(usize, usize)>,
    },
    /// The IME committed its composition as text.
    ImeCommit {
        /// The committed text.
        text: String,
    },
    /// A mouse button went down or came up.
    MouseButton {
        /// Which button.
        button: MouseButton,
        /// Down or up.
        state: ButtonState,
    },
    /// The cursor moved over the game.
    CursorMoved {
        /// The new position.
        position: Vector2f,
    },
    /// The cursor left the game's area.
    CursorLeft,
    /// The mouse moved, separately from the cursor. The movement is
    /// unaccelerated where the platform reports it that way, and it continues
    /// when the cursor reaches the window border.
    MouseMotion {
        /// The movement.
        delta: Vector2f,
    },
    /// The mouse wheel or touchpad scrolled.
    MouseWheel {
        /// How far.
        delta: ScrollDelta,
    },
    /// The game lost keyboard focus. Everything held is released, because the
    /// matching release events will go to another window.
    FocusLost,
    /// A gamepad connected and took a slot.
    GamepadConnected {
        /// The slot it took.
        player: PlayerId,
    },
    /// A gamepad disconnected and freed its slot.
    GamepadDisconnected {
        /// The slot it freed.
        player: PlayerId,
    },
    /// A gamepad button went down or came up.
    GamepadButton {
        /// Whose gamepad.
        player: PlayerId,
        /// Which button.
        button: GamepadButton,
        /// Down or up.
        state: ButtonState,
    },
    /// A gamepad axis moved.
    GamepadAxis {
        /// Whose gamepad.
        player: PlayerId,
        /// Which axis.
        axis: GamepadAxis,
        /// The raw value, before the deadzone.
        value: f32,
    },
}

/// A request to rumble a player's gamepad, queued by a system for the
/// frontend to play.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RumbleRequest {
    /// Whose gamepad.
    pub player: PlayerId,
    /// High-frequency motor strength, `0.0..=1.0`.
    pub weak: f32,
    /// Low-frequency motor strength, `0.0..=1.0`.
    pub strong: f32,
    /// How long to rumble.
    pub duration: Duration,
}

// =============================================================================
// KeySet
// =============================================================================

/// One bit per [`KeyCode`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
struct KeySet([u64; KEY_SET_WORD_COUNT]);

impl KeySet {
    /// Whether `key` is in the set.
    fn contains(&self, key: KeyCode) -> bool {
        let index = key as usize;
        self.0[index / 64] & (1 << (index % 64)) != 0
    }

    /// Every key in the set, in [`KeyCode`] order.
    fn iter(&self) -> impl Iterator<Item = KeyCode> + '_ {
        KeyCode::ALL
            .iter()
            .copied()
            .filter(|key| self.contains(*key))
    }
}

// =============================================================================
// GamepadState
// =============================================================================

/// The state of one gamepad slot.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
struct GamepadState {
    /// Whether a gamepad holds this slot.
    connected: bool,
    /// [`GamepadButton`] bits down at the start of the frame.
    buttons_held: u32,
    /// Button bits that went down since the previous frame.
    buttons_pressed: u32,
    /// Button bits that came up since the previous frame.
    buttons_released: u32,
    /// Latest value per [`GamepadAxis`], after the deadzone.
    axes: [f32; GamepadAxis::COUNT],
}

// =============================================================================
// Edge Tracking
// =============================================================================

/// A fixed set of button bits, one bit per button.
///
/// Each device kind stores its bits differently: [`KeySet`] uses 64-bit words,
/// the mouse uses a `u8`, and each gamepad uses a `u32`. This trait hides those
/// differences, so the edge rule below can be written once for all of them.
trait EdgeBits: Copy + Default {
    /// Whether `bit_index` is set.
    fn has(&self, bit_index: u32) -> bool;

    /// Sets or clears `bit_index`.
    fn set(&mut self, bit_index: u32, present: bool);

    /// Adds every bit of `other`.
    fn merge(&mut self, other: Self);
}

impl EdgeBits for KeySet {
    fn has(&self, bit_index: u32) -> bool {
        let index = bit_index as usize;
        self.0[index / 64] & (1 << (index % 64)) != 0
    }

    fn set(&mut self, bit_index: u32, present: bool) {
        let index = bit_index as usize;
        let mask = 1 << (index % 64);
        if present {
            self.0[index / 64] |= mask;
        } else {
            self.0[index / 64] &= !mask;
        }
    }

    fn merge(&mut self, other: Self) {
        for (word, incoming) in self.0.iter_mut().zip(other.0) {
            *word |= incoming;
        }
    }
}

impl EdgeBits for u8 {
    fn has(&self, bit_index: u32) -> bool {
        *self & (1u8 << bit_index) != 0
    }

    fn set(&mut self, bit_index: u32, present: bool) {
        if present {
            *self |= 1u8 << bit_index;
        } else {
            *self &= !(1u8 << bit_index);
        }
    }

    fn merge(&mut self, other: Self) {
        *self |= other;
    }
}

impl EdgeBits for u32 {
    fn has(&self, bit_index: u32) -> bool {
        *self & (1u32 << bit_index) != 0
    }

    fn set(&mut self, bit_index: u32, present: bool) {
        if present {
            *self |= 1u32 << bit_index;
        } else {
            *self &= !(1u32 << bit_index);
        }
    }

    fn merge(&mut self, other: Self) {
        *self |= other;
    }
}

// =============================================================================
// Input
// =============================================================================

/// Keyboard, mouse and gamepad state for the current frame.
///
/// The engine inserts this resource automatically, and a project never
/// constructs one. Read it from any system:
///
/// ```no_run
/// # use pill_engine::*;
/// fn jump(input: Res<Input>) {
///     let Some(input) = input.get() else { return };
///     if input.key_pressed(KeyCode::Space)
///         || input.gamepad_button_pressed(PlayerId::Player1, GamepadButton::A)
///     {
///         // Start the jump.
///     }
/// }
/// ```
///
/// Every value describes the frame being processed and stays fixed for the
/// whole frame. Headless runs never receive events, so nothing is held,
/// nothing accumulates, and there is no cursor.
#[derive(Debug, Default)]
pub struct Input {
    /// Keys down at the start of the frame.
    keys_held: KeySet,
    /// Keys that went down since the previous frame.
    keys_pressed: KeySet,
    /// Keys that came up since the previous frame.
    keys_released: KeySet,
    /// [`MouseButton`] bits down at the start of the frame.
    mouse_held: u8,
    /// Mouse button bits that went down since the previous frame.
    mouse_pressed: u8,
    /// Mouse button bits that came up since the previous frame.
    mouse_released: u8,
    /// The latest cursor position over the game, or `None` when it is not
    /// over it.
    cursor_position: Option<Vector2f>,
    /// The modifier keys' state, as last reported.
    modifiers: Modifiers,
    /// Text committed since the previous frame: typed characters and IME
    /// commits, in order.
    text_entered: String,
    /// The IME's active composition text, while a composition is in progress.
    ime_preedit: Option<String>,
    /// The composing cursor range within [`Self::ime_preedit`], as byte
    /// offsets, when the platform reports one.
    ime_cursor: Option<(usize, usize)>,
    /// Mouse motion accumulated since the previous frame.
    mouse_delta: Vector2f,
    /// Line scrolling accumulated since the previous frame.
    scroll_lines: Vector2f,
    /// Pixel scrolling accumulated since the previous frame.
    scroll_pixels: Vector2f,
    /// One state per [`PlayerId`].
    gamepads: [GamepadState; GAMEPAD_SLOT_COUNT],
    /// Events queued since the previous frame, applied at the next one.
    pending_events: Vec<InputEvent>,
    /// Rumble requests not yet taken by the frontend.
    rumble_requests: Vec<RumbleRequest>,
}

impl Resource for Input {
    /// Infrastructure, not game state: Play leaves this value alone and
    /// Stop does not restore it.
    fn play_state() -> crate::resource::PlayState {
        crate::resource::PlayState::Keep
    }
}

impl Input {
    /// Input with nothing held and no gamepad connected.
    pub fn new() -> Self {
        Self::default()
    }

    // -------------------------------------------------------------------------
    // Keyboard
    // -------------------------------------------------------------------------

    /// Whether `key` is down this frame.
    pub fn key_held(&self, key: KeyCode) -> bool {
        self.keys_held.contains(key)
    }

    /// Whether `key` went down since the previous frame.
    pub fn key_pressed(&self, key: KeyCode) -> bool {
        self.keys_pressed.contains(key)
    }

    /// Whether `key` came up since the previous frame.
    pub fn key_released(&self, key: KeyCode) -> bool {
        self.keys_released.contains(key)
    }

    /// Every key down this frame.
    pub fn keys_held(&self) -> impl Iterator<Item = KeyCode> + '_ {
        self.keys_held.iter()
    }

    /// Every key that went down since the previous frame.
    pub fn keys_pressed(&self) -> impl Iterator<Item = KeyCode> + '_ {
        self.keys_pressed.iter()
    }

    /// Every key that came up since the previous frame.
    pub fn keys_released(&self) -> impl Iterator<Item = KeyCode> + '_ {
        self.keys_released.iter()
    }

    // -------------------------------------------------------------------------
    // Modifiers and text
    // -------------------------------------------------------------------------

    /// The modifier keys' state.
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// Text committed since the previous frame: typed characters and IME
    /// commits, in order.
    pub fn text_entered(&self) -> &str {
        &self.text_entered
    }

    /// The IME's active composition text, or `None` while not composing.
    pub fn ime_preedit(&self) -> Option<&str> {
        self.ime_preedit.as_deref()
    }

    /// The composing cursor range within [`Self::ime_preedit`], as byte
    /// offsets, when the platform reports one.
    pub fn ime_cursor(&self) -> Option<(usize, usize)> {
        self.ime_cursor
    }

    // -------------------------------------------------------------------------
    // Mouse
    // -------------------------------------------------------------------------

    /// Whether `button` is down this frame.
    pub fn mouse_button_held(&self, button: MouseButton) -> bool {
        self.mouse_held & button.bit_mask() != 0
    }

    /// Whether `button` went down since the previous frame.
    pub fn mouse_button_pressed(&self, button: MouseButton) -> bool {
        self.mouse_pressed & button.bit_mask() != 0
    }

    /// Whether `button` came up since the previous frame.
    pub fn mouse_button_released(&self, button: MouseButton) -> bool {
        self.mouse_released & button.bit_mask() != 0
    }

    /// The cursor position in physical pixels from the top-left corner of the
    /// game's area, or `None` while the cursor is elsewhere.
    pub fn cursor_position(&self) -> Option<Vector2f> {
        self.cursor_position
    }

    /// Mouse movement since the previous frame, in the frontend's raw units.
    /// The platform reports device motion without scaling it.
    ///
    /// Motion keeps arriving when the cursor reaches the window border, so this
    /// is the value to use for mouse-look.
    pub fn mouse_delta(&self) -> Vector2f {
        self.mouse_delta
    }

    /// Wheel lines scrolled since the previous frame; positive `y` is up.
    pub fn scroll_lines(&self) -> Vector2f {
        self.scroll_lines
    }

    /// Touchpad pixels scrolled since the previous frame; positive `y` is up.
    pub fn scroll_pixels(&self) -> Vector2f {
        self.scroll_pixels
    }

    // -------------------------------------------------------------------------
    // Gamepads
    // -------------------------------------------------------------------------

    /// Whether `player` has a gamepad connected.
    pub fn gamepad_connected(&self, player: PlayerId) -> bool {
        self.gamepads[player.index()].connected
    }

    /// Every player with a gamepad connected.
    pub fn connected_gamepads(&self) -> impl Iterator<Item = PlayerId> + '_ {
        PlayerId::ALL
            .into_iter()
            .filter(|player| self.gamepad_connected(*player))
    }

    /// Whether `player`'s `button` is down this frame.
    pub fn gamepad_button_held(&self, player: PlayerId, button: GamepadButton) -> bool {
        self.gamepads[player.index()].buttons_held & button.bit_mask() != 0
    }

    /// Whether `player`'s `button` went down since the previous frame.
    pub fn gamepad_button_pressed(&self, player: PlayerId, button: GamepadButton) -> bool {
        self.gamepads[player.index()].buttons_pressed & button.bit_mask() != 0
    }

    /// Whether `player`'s `button` came up since the previous frame.
    pub fn gamepad_button_released(&self, player: PlayerId, button: GamepadButton) -> bool {
        self.gamepads[player.index()].buttons_released & button.bit_mask() != 0
    }

    /// `player`'s `axis`, after the deadzone; zero without a gamepad.
    pub fn gamepad_axis(&self, player: PlayerId, axis: GamepadAxis) -> f32 {
        self.gamepads[player.index()].axes[axis as usize]
    }

    /// Ask the frontend to rumble `player`'s gamepad.
    ///
    /// Both strengths are clamped to `0.0..=1.0`. The frontend that owns the
    /// gamepads plays the request after the frame. It is ignored when no such
    /// frontend exists (headless, the browser) or when the gamepad cannot
    /// rumble.
    pub fn request_rumble(&mut self, player: PlayerId, weak: f32, strong: f32, duration: Duration) {
        self.rumble_requests.push(RumbleRequest {
            player,
            weak: weak.clamp(0.0, 1.0),
            strong: strong.clamp(0.0, 1.0),
            duration,
        });
    }

    /// Take every rumble request queued so far. This is the frontend's half of
    /// [`Self::request_rumble`].
    pub fn take_rumble_requests(&mut self) -> Vec<RumbleRequest> {
        std::mem::take(&mut self.rumble_requests)
    }

    // -------------------------------------------------------------------------
    // Feeding
    // -------------------------------------------------------------------------

    /// Queue `event` for the next frame.
    ///
    /// Frontends call this between frames, through
    /// [`Engine::push_input_event`](crate::engine::Engine::push_input_event). A
    /// system may also queue events to simulate input. Either way, the events
    /// take effect on the next frame.
    pub fn push_event(&mut self, event: InputEvent) {
        self.pending_events.push(event);
    }

    /// Start a frame: forget the previous frame's edges, motion and committed
    /// text, then apply every queued event in order.
    ///
    /// Called by the engine at the top of each frame, before systems run.
    pub(crate) fn begin_frame(&mut self) {
        // Step 1: Clear the previous frame's edges, motion and committed text.
        self.keys_pressed = KeySet::default();
        self.keys_released = KeySet::default();
        self.mouse_pressed = 0;
        self.mouse_released = 0;
        self.mouse_delta = Vector2f::ZERO;
        self.scroll_lines = Vector2f::ZERO;
        self.scroll_pixels = Vector2f::ZERO;
        self.text_entered.clear();
        for gamepad in &mut self.gamepads {
            gamepad.buttons_pressed = 0;
            gamepad.buttons_released = 0;
        }

        // Step 2: Apply every queued event in order. The buffer is taken out
        // and put back so its capacity is reused.
        let mut events = std::mem::take(&mut self.pending_events);
        for event in events.drain(..) {
            self.apply_event(event);
        }
        self.pending_events = events;
    }

    /// Apply one event to the current frame's state.
    fn apply_event(&mut self, event: InputEvent) {
        match event {
            InputEvent::Key { key, state } => self.apply_key(key, state),
            // A repeat is not a new press. The held state stays as it was.
            InputEvent::KeyRepeat { .. } => {}
            InputEvent::ModifiersChanged { modifiers } => self.modifiers = modifiers,
            InputEvent::TextInput { text } => self.text_entered.push_str(&text),
            InputEvent::ImePreedit { text, cursor } => {
                if text.is_empty() {
                    self.ime_preedit = None;
                    self.ime_cursor = None;
                } else {
                    self.ime_preedit = Some(text);
                    self.ime_cursor = cursor;
                }
            }
            InputEvent::ImeCommit { text } => {
                self.text_entered.push_str(&text);
                self.ime_preedit = None;
                self.ime_cursor = None;
            }
            InputEvent::MouseButton { button, state } => self.apply_mouse_button(button, state),
            InputEvent::CursorMoved { position } => self.cursor_position = Some(position),
            InputEvent::CursorLeft => self.cursor_position = None,
            InputEvent::MouseMotion { delta } => self.mouse_delta += delta,
            InputEvent::MouseWheel {
                delta: ScrollDelta::Lines(delta),
            } => self.scroll_lines += delta,
            InputEvent::MouseWheel {
                delta: ScrollDelta::Pixels(delta),
            } => self.scroll_pixels += delta,
            InputEvent::FocusLost => {
                // A composition cannot outlive the focus that owned it. The
                // platform also stops reporting modifier state while it has no
                // focus, so the state is reset. The next report sets both
                // again.
                self.modifiers = Modifiers::default();
                self.ime_preedit = None;
                self.ime_cursor = None;
                self.release_keyboard_and_mouse();
            }
            InputEvent::GamepadConnected { player } => {
                self.gamepads[player.index()] = GamepadState {
                    connected: true,
                    ..GamepadState::default()
                };
            }
            InputEvent::GamepadDisconnected { player } => {
                self.gamepads[player.index()] = GamepadState::default();
            }
            InputEvent::GamepadButton {
                player,
                button,
                state,
            } => {
                let gamepad = &mut self.gamepads[player.index()];
                apply_edge(
                    &mut gamepad.buttons_held,
                    &mut gamepad.buttons_pressed,
                    &mut gamepad.buttons_released,
                    button.bit_mask().trailing_zeros(),
                    state,
                );
            }
            InputEvent::GamepadAxis {
                player,
                axis,
                value,
            } => {
                let value = if value.abs() < GAMEPAD_AXIS_DEADZONE {
                    0.0
                } else {
                    value
                };
                self.gamepads[player.index()].axes[axis as usize] = value;
            }
        }
    }

    /// Apply a key change.
    ///
    /// A press of a held key is the operating system's repeat, so nothing
    /// changes. A release of a key that is not held changes nothing either; it
    /// may already have been released by [`InputEvent::FocusLost`].
    fn apply_key(&mut self, key: KeyCode, state: ButtonState) {
        apply_edge(
            &mut self.keys_held,
            &mut self.keys_pressed,
            &mut self.keys_released,
            key as u32,
            state,
        );
    }

    /// Apply a mouse button change, with the same rules as [`Self::apply_key`].
    fn apply_mouse_button(&mut self, button: MouseButton, state: ButtonState) {
        apply_edge(
            &mut self.mouse_held,
            &mut self.mouse_pressed,
            &mut self.mouse_released,
            button.bit_mask().trailing_zeros(),
            state,
        );
    }

    /// Release every held key and mouse button, reporting each as released.
    fn release_keyboard_and_mouse(&mut self) {
        release_edges(&mut self.keys_held, &mut self.keys_released);
        release_edges(&mut self.mouse_held, &mut self.mouse_released);
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Applies one button edge.
///
/// A press of a bit that is already held is the operating system's repeat, so
/// nothing changes. A release of a bit that is not held changes nothing either;
/// [`InputEvent::FocusLost`] may already have released it. Keys, mouse buttons
/// and gamepad buttons all come through here, so the rule lives in one place.
fn apply_edge<B: EdgeBits>(
    held: &mut B,
    pressed: &mut B,
    released: &mut B,
    bit_index: u32,
    state: ButtonState,
) {
    match state {
        ButtonState::Pressed if !held.has(bit_index) => {
            held.set(bit_index, true);
            pressed.set(bit_index, true);
        }
        ButtonState::Released if held.has(bit_index) => {
            held.set(bit_index, false);
            released.set(bit_index, true);
        }
        ButtonState::Pressed | ButtonState::Released => {}
    }
}

/// Releases every held bit, reporting each as released.
fn release_edges<B: EdgeBits>(held: &mut B, released: &mut B) {
    released.merge(*held);
    *held = B::default();
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Queue `events` and start a frame.
    fn frame(input: &mut Input, events: &[InputEvent]) {
        for event in events {
            input.push_event(event.clone());
        }
        input.begin_frame();
    }

    fn key(key: KeyCode, state: ButtonState) -> InputEvent {
        InputEvent::Key { key, state }
    }

    #[test]
    fn a_press_is_an_edge_for_one_frame_and_held_until_released() {
        let mut input = Input::new();
        frame(&mut input, &[key(KeyCode::KeyW, ButtonState::Pressed)]);
        assert!(input.key_pressed(KeyCode::KeyW));
        assert!(input.key_held(KeyCode::KeyW));

        frame(&mut input, &[]);
        assert!(!input.key_pressed(KeyCode::KeyW));
        assert!(input.key_held(KeyCode::KeyW));

        frame(&mut input, &[key(KeyCode::KeyW, ButtonState::Released)]);
        assert!(input.key_released(KeyCode::KeyW));
        assert!(!input.key_held(KeyCode::KeyW));

        frame(&mut input, &[]);
        assert!(!input.key_released(KeyCode::KeyW));
    }

    #[test]
    fn a_tap_inside_one_frame_reports_both_edges() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[
                key(KeyCode::Space, ButtonState::Pressed),
                key(KeyCode::Space, ButtonState::Released),
            ],
        );
        assert!(input.key_pressed(KeyCode::Space));
        assert!(input.key_released(KeyCode::Space));
        assert!(!input.key_held(KeyCode::Space));
    }

    #[test]
    fn key_repeat_is_not_a_new_press() {
        let mut input = Input::new();
        frame(&mut input, &[key(KeyCode::KeyA, ButtonState::Pressed)]);
        frame(&mut input, &[key(KeyCode::KeyA, ButtonState::Pressed)]);
        assert!(!input.key_pressed(KeyCode::KeyA));
        assert!(input.key_held(KeyCode::KeyA));
    }

    #[test]
    fn a_repeat_in_the_frame_of_the_press_keeps_the_press() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[
                key(KeyCode::KeyA, ButtonState::Pressed),
                key(KeyCode::KeyA, ButtonState::Pressed),
            ],
        );
        assert!(input.key_pressed(KeyCode::KeyA));
    }

    #[test]
    fn focus_loss_releases_everything_held() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[
                key(KeyCode::ShiftLeft, ButtonState::Pressed),
                key(KeyCode::F35, ButtonState::Pressed),
                InputEvent::MouseButton {
                    button: MouseButton::Right,
                    state: ButtonState::Pressed,
                },
            ],
        );
        frame(&mut input, &[InputEvent::FocusLost]);
        assert!(input.key_released(KeyCode::ShiftLeft));
        assert!(input.key_released(KeyCode::F35));
        assert!(!input.key_held(KeyCode::ShiftLeft));
        assert!(input.mouse_button_released(MouseButton::Right));
        assert!(!input.mouse_button_held(MouseButton::Right));

        // The real release arriving later is not a second edge.
        frame(
            &mut input,
            &[key(KeyCode::ShiftLeft, ButtonState::Released)],
        );
        assert!(!input.key_released(KeyCode::ShiftLeft));
    }

    #[test]
    fn motion_and_scroll_add_up_over_a_frame_and_reset_after_it() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[
                InputEvent::MouseMotion {
                    delta: Vector2f::new(3.0, -1.0),
                },
                InputEvent::MouseMotion {
                    delta: Vector2f::new(2.0, -1.0),
                },
                InputEvent::MouseWheel {
                    delta: ScrollDelta::Lines(Vector2f::new(0.0, 1.0)),
                },
                InputEvent::MouseWheel {
                    delta: ScrollDelta::Lines(Vector2f::new(0.0, 2.0)),
                },
                InputEvent::MouseWheel {
                    delta: ScrollDelta::Pixels(Vector2f::new(0.0, -40.0)),
                },
            ],
        );
        assert_eq!(input.mouse_delta(), Vector2f::new(5.0, -2.0));
        assert_eq!(input.scroll_lines(), Vector2f::new(0.0, 3.0));
        assert_eq!(input.scroll_pixels(), Vector2f::new(0.0, -40.0));

        frame(&mut input, &[]);
        assert_eq!(input.mouse_delta(), Vector2f::ZERO);
        assert_eq!(input.scroll_lines(), Vector2f::ZERO);
        assert_eq!(input.scroll_pixels(), Vector2f::ZERO);
    }

    #[test]
    fn the_cursor_keeps_its_latest_position_until_it_leaves() {
        let mut input = Input::new();
        assert_eq!(input.cursor_position(), None);
        frame(
            &mut input,
            &[
                InputEvent::CursorMoved {
                    position: Vector2f::new(1.0, 2.0),
                },
                InputEvent::CursorMoved {
                    position: Vector2f::new(10.0, 20.0),
                },
            ],
        );
        assert_eq!(input.cursor_position(), Some(Vector2f::new(10.0, 20.0)));
        frame(&mut input, &[]);
        assert_eq!(input.cursor_position(), Some(Vector2f::new(10.0, 20.0)));
        frame(&mut input, &[InputEvent::CursorLeft]);
        assert_eq!(input.cursor_position(), None);
    }

    #[test]
    fn gamepads_track_buttons_and_axes_per_player() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[
                InputEvent::GamepadConnected {
                    player: PlayerId::Player2,
                },
                InputEvent::GamepadButton {
                    player: PlayerId::Player2,
                    button: GamepadButton::A,
                    state: ButtonState::Pressed,
                },
                InputEvent::GamepadAxis {
                    player: PlayerId::Player2,
                    axis: GamepadAxis::LeftStickX,
                    value: 0.75,
                },
                InputEvent::GamepadAxis {
                    player: PlayerId::Player2,
                    axis: GamepadAxis::LeftStickY,
                    value: 0.01,
                },
            ],
        );
        assert!(input.gamepad_connected(PlayerId::Player2));
        assert!(!input.gamepad_connected(PlayerId::Player1));
        assert_eq!(
            input.connected_gamepads().collect::<Vec<_>>(),
            [PlayerId::Player2]
        );
        assert!(input.gamepad_button_pressed(PlayerId::Player2, GamepadButton::A));
        assert!(!input.gamepad_button_pressed(PlayerId::Player1, GamepadButton::A));
        assert_eq!(
            input.gamepad_axis(PlayerId::Player2, GamepadAxis::LeftStickX),
            0.75
        );
        // Inside the deadzone.
        assert_eq!(
            input.gamepad_axis(PlayerId::Player2, GamepadAxis::LeftStickY),
            0.0
        );

        frame(&mut input, &[]);
        assert!(!input.gamepad_button_pressed(PlayerId::Player2, GamepadButton::A));
        assert!(input.gamepad_button_held(PlayerId::Player2, GamepadButton::A));

        frame(
            &mut input,
            &[InputEvent::GamepadDisconnected {
                player: PlayerId::Player2,
            }],
        );
        assert!(!input.gamepad_connected(PlayerId::Player2));
        assert!(!input.gamepad_button_held(PlayerId::Player2, GamepadButton::A));
        assert_eq!(
            input.gamepad_axis(PlayerId::Player2, GamepadAxis::LeftStickX),
            0.0
        );
    }

    #[test]
    fn rumble_requests_are_clamped_and_taken_once() {
        let mut input = Input::new();
        input.request_rumble(PlayerId::Player1, 2.0, -1.0, Duration::from_millis(200));
        let requests = input.take_rumble_requests();
        assert_eq!(
            requests,
            [RumbleRequest {
                player: PlayerId::Player1,
                weak: 1.0,
                strong: 0.0,
                duration: Duration::from_millis(200),
            }]
        );
        assert!(input.take_rumble_requests().is_empty());
    }

    #[test]
    fn every_key_code_name_round_trips() {
        for key in KeyCode::ALL {
            assert_eq!(KeyCode::from_code_name(key.code_name()), Some(*key));
        }
        assert_eq!(KeyCode::from_code_name("OSLeft"), Some(KeyCode::SuperLeft));
        assert_eq!(KeyCode::from_code_name("NoSuchKey"), None);
    }

    #[test]
    fn keys_held_lists_keys_in_order() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[
                key(KeyCode::KeyD, ButtonState::Pressed),
                key(KeyCode::KeyA, ButtonState::Pressed),
            ],
        );
        assert_eq!(
            input.keys_held().collect::<Vec<_>>(),
            [KeyCode::KeyA, KeyCode::KeyD]
        );
        assert_eq!(
            input.keys_pressed().collect::<Vec<_>>(),
            [KeyCode::KeyA, KeyCode::KeyD]
        );
    }

    #[test]
    fn a_repeat_event_leaves_the_held_state_alone() {
        let mut input = Input::new();
        frame(&mut input, &[key(KeyCode::KeyA, ButtonState::Pressed)]);
        frame(&mut input, &[InputEvent::KeyRepeat { key: KeyCode::KeyA }]);
        assert!(input.key_held(KeyCode::KeyA));
        assert!(!input.key_pressed(KeyCode::KeyA));
        assert!(!input.key_released(KeyCode::KeyA));
    }

    #[test]
    fn committed_text_adds_up_over_the_frame_in_order() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[
                InputEvent::TextInput {
                    text: "he".to_string(),
                },
                InputEvent::ImeCommit {
                    text: "llo".to_string(),
                },
            ],
        );
        assert_eq!(input.text_entered(), "hello");
        frame(&mut input, &[]);
        assert_eq!(input.text_entered(), "");
    }

    #[test]
    fn the_modifier_state_is_the_latest_reported_one() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[
                InputEvent::ModifiersChanged {
                    modifiers: Modifiers {
                        shift: true,
                        ..Modifiers::default()
                    },
                },
                InputEvent::ModifiersChanged {
                    modifiers: Modifiers {
                        control: true,
                        ..Modifiers::default()
                    },
                },
            ],
        );
        assert_eq!(
            input.modifiers(),
            Modifiers {
                control: true,
                ..Modifiers::default()
            }
        );
    }

    #[test]
    fn a_preedit_lives_until_committed_and_a_commit_clears_it() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[InputEvent::ImePreedit {
                text: "ni".to_string(),
                cursor: Some((0, 2)),
            }],
        );
        assert_eq!(input.ime_preedit(), Some("ni"));
        assert_eq!(input.ime_cursor(), Some((0, 2)));
        frame(
            &mut input,
            &[InputEvent::ImeCommit {
                text: "nihon".to_string(),
            }],
        );
        assert_eq!(input.ime_preedit(), None);
        assert_eq!(input.ime_cursor(), None);
        assert_eq!(input.text_entered(), "nihon");
    }

    #[test]
    fn an_empty_preedit_cancels_the_composition() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[InputEvent::ImePreedit {
                text: "kai".to_string(),
                cursor: None,
            }],
        );
        frame(
            &mut input,
            &[InputEvent::ImePreedit {
                text: String::new(),
                cursor: None,
            }],
        );
        assert_eq!(input.ime_preedit(), None);
    }

    #[test]
    fn focus_loss_clears_the_modifier_state_and_the_composition() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[
                InputEvent::ModifiersChanged {
                    modifiers: Modifiers {
                        shift: true,
                        ..Modifiers::default()
                    },
                },
                InputEvent::ImePreedit {
                    text: "ka".to_string(),
                    cursor: None,
                },
            ],
        );
        frame(&mut input, &[InputEvent::FocusLost]);
        assert_eq!(input.modifiers(), Modifiers::default());
        assert_eq!(input.ime_preedit(), None);
    }

    /// Keys, mouse buttons and gamepad buttons report the same edge pattern.
    ///
    /// The three device kinds share one edge rule, so the same sequence driven
    /// through each of them must give the same answers. A press is one press, a
    /// repeat while held is nothing, and a release is one release.
    #[test]
    fn every_device_kind_reports_the_same_press_repeat_release_pattern() {
        let mut input = Input::new();
        frame(
            &mut input,
            &[InputEvent::GamepadConnected {
                player: PlayerId::Player1,
            }],
        );
        assert!(!input.gamepad_button_held(PlayerId::Player1, GamepadButton::A));

        // One press through all three kinds at once.
        frame(
            &mut input,
            &[
                key(KeyCode::KeyW, ButtonState::Pressed),
                InputEvent::MouseButton {
                    button: MouseButton::Left,
                    state: ButtonState::Pressed,
                },
                InputEvent::GamepadButton {
                    player: PlayerId::Player1,
                    button: GamepadButton::A,
                    state: ButtonState::Pressed,
                },
            ],
        );
        let first = (
            input.key_pressed(KeyCode::KeyW),
            input.mouse_button_pressed(MouseButton::Left),
            input.gamepad_button_pressed(PlayerId::Player1, GamepadButton::A),
        );
        assert_eq!(
            first,
            (true, true, true),
            "a press is an edge on every kind"
        );

        // A repeat press of something already held is not a new edge.
        frame(
            &mut input,
            &[
                key(KeyCode::KeyW, ButtonState::Pressed),
                InputEvent::MouseButton {
                    button: MouseButton::Left,
                    state: ButtonState::Pressed,
                },
                InputEvent::GamepadButton {
                    player: PlayerId::Player1,
                    button: GamepadButton::A,
                    state: ButtonState::Pressed,
                },
            ],
        );
        let repeat = (
            input.key_pressed(KeyCode::KeyW),
            input.mouse_button_pressed(MouseButton::Left),
            input.gamepad_button_pressed(PlayerId::Player1, GamepadButton::A),
        );
        assert_eq!(repeat, (false, false, false), "a repeat is not an edge");
        assert!(
            input.key_held(KeyCode::KeyW)
                && input.mouse_button_held(MouseButton::Left)
                && input.gamepad_button_held(PlayerId::Player1, GamepadButton::A),
            "a repeat leaves every kind held"
        );

        // One release. A second release of the same button is not an edge.
        frame(
            &mut input,
            &[
                key(KeyCode::KeyW, ButtonState::Released),
                InputEvent::MouseButton {
                    button: MouseButton::Left,
                    state: ButtonState::Released,
                },
                InputEvent::GamepadButton {
                    player: PlayerId::Player1,
                    button: GamepadButton::A,
                    state: ButtonState::Released,
                },
            ],
        );
        let released = (
            input.key_released(KeyCode::KeyW),
            input.mouse_button_released(MouseButton::Left),
            input.gamepad_button_released(PlayerId::Player1, GamepadButton::A),
        );
        assert_eq!(
            released,
            (true, true, true),
            "a release is an edge on every kind"
        );

        frame(
            &mut input,
            &[
                key(KeyCode::KeyW, ButtonState::Released),
                InputEvent::MouseButton {
                    button: MouseButton::Left,
                    state: ButtonState::Released,
                },
                InputEvent::GamepadButton {
                    player: PlayerId::Player1,
                    button: GamepadButton::A,
                    state: ButtonState::Released,
                },
            ],
        );
        let double_release = (
            input.key_released(KeyCode::KeyW),
            input.mouse_button_released(MouseButton::Left),
            input.gamepad_button_released(PlayerId::Player1, GamepadButton::A),
        );
        assert_eq!(
            double_release,
            (false, false, false),
            "releasing something already released is not an edge"
        );
    }
}
