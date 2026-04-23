use crate::{
    ecs::{GlobalComponent, GlobalComponentStorage},
    engine::{KeyboardKey, MouseButton},
    internal::NUM_SUPPORTED_GAMEPADS,
};

use pill_core::{PillTypeMapKey, Vector2f};

use anyhow::{Error, Result};
use std::collections::{HashMap, VecDeque};
use winit::event::{ElementState, MouseScrollDelta};

// Native has gilrs for gamepads + bitvec for compact key/button state +
// std::time::Instant for force-feedback deadlines. Wasm has none of
// those (no process-wide input, no std::time on wasm32), so we stub
// the types with the same shapes. Module-level cfg-gating keeps the
// scattered attributes out of the rest of the file — mirrors how
// `input_system.rs` splits native/wasm into sub-modules.
#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::*;
    use bitvec::prelude::*;
    pub use gilrs::{ff::Effect, GamepadId};
    pub use std::time::Instant;

    // 256 bits — actually we have less but this is fine
    pub type KeyState = BitArray<[u64; 4], Lsb0>;
    // we have 17 buttons
    pub type GamepadButtonState = BitArray<[u32; NUM_SUPPORTED_GAMEPADS], Lsb0>;

    pub const fn key_state_zero() -> KeyState {
        BitArray::ZERO
    }
    pub const fn gamepad_button_state_zero() -> GamepadButtonState {
        BitArray::ZERO
    }

    pub fn set_key_state(state: &mut KeyState, i: usize, v: bool) {
        state.set(i, v);
    }
    pub fn set_gamepad_state(state: &mut GamepadButtonState, i: usize, v: bool) {
        state.set(i, v);
    }

    pub struct InFlight {
        pub(crate) id: GamepadId,
        pub(crate) effect: Effect,
        pub(crate) end_at: Instant,
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    use super::*;

    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
    pub struct GamepadId(pub usize);

    impl std::fmt::Display for GamepadId {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "GamepadId({})", self.0)
        }
    }

    #[derive(Clone)]
    pub struct Effect;

    impl Effect {
        pub fn add_gamepad(&self, _gamepad: &()) -> Result<()> {
            Ok(())
        }
        pub fn play(&self) -> Result<()> {
            Ok(())
        }
    }

    pub type KeyState = [bool; KEYBOARD_KEY_MAX];
    pub type GamepadButtonState = [bool; GAMEPAD_BUTTON_MAX];

    pub const fn key_state_zero() -> KeyState {
        [false; KEYBOARD_KEY_MAX]
    }
    pub const fn gamepad_button_state_zero() -> GamepadButtonState {
        [false; GAMEPAD_BUTTON_MAX]
    }

    pub fn set_key_state(state: &mut KeyState, i: usize, v: bool) {
        state[i] = v;
    }
    pub fn set_gamepad_state(state: &mut GamepadButtonState, i: usize, v: bool) {
        state[i] = v;
    }

    // No force-feedback on wasm — InFlight just tags the id for reset tracking.
    pub struct InFlight {
        pub(crate) id: GamepadId,
    }
}

#[cfg(not(target_arch = "wasm32"))]
use native::{
    gamepad_button_state_zero, key_state_zero, set_gamepad_state, set_key_state,
    GamepadButtonState, KeyState,
};
#[cfg(not(target_arch = "wasm32"))]
pub use native::{Effect, GamepadId, InFlight};

#[cfg(target_arch = "wasm32")]
use wasm::{
    gamepad_button_state_zero, key_state_zero, set_gamepad_state, set_key_state,
    GamepadButtonState, KeyState,
};
#[cfg(target_arch = "wasm32")]
pub use wasm::{Effect, GamepadId, InFlight};

#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum PlayerId {
    Player1 = 0,
    Player2,
    Player3,
    Player4,
}

impl TryFrom<u8> for PlayerId {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(PlayerId::Player1),
            1 => Ok(PlayerId::Player2),
            2 => Ok(PlayerId::Player3),
            3 => Ok(PlayerId::Player4),
            _ => Err(anyhow::anyhow!("Invalid PlayerId value: {}", value)),
        }
    }
}

pub const GAMEPAD_DEADZONE: f32 = 0.05; // Deadzone for gamepad axes

pub const KEYBOARD_KEY_COUNT: usize = KeyboardKey::F35 as usize + 1; // Total number of keys in KeyboardKey enum
pub const MOUSE_BUTTON_COUNT: usize = 3; // Left, Middle, Right

/// Gamepad enums with more descriptive names
#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum GamepadButton {
    A = 0,
    B,
    X,
    Y,
    LeftBumper,
    RightBumper,
    LeftTrigger,
    RightTrigger,
    Back,
    Start,
    Mode,
    LeftStick,
    RightStick,
    DPadUp,
    DPadDown,
    DPadLeft,
    DPadRight,
}
pub const GAMEPAD_BUTTON_COUNT: usize = GamepadButton::DPadRight as usize + 1;

#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum GamepadAxis {
    LeftStickX = 0,
    LeftStickY,
    RightStickX,
    RightStickY,
    LeftTrigger,
    RightTrigger,
    DPadX,
    DPadY,
}
pub const GAMEPAD_AXIS_COUNT: usize = GamepadAxis::DPadY as usize + 1;

#[derive(Clone)]
pub enum HapticCommand {
    Rumble {
        player_id: PlayerId,
        weak: f32,
        strong: f32,
        duration_ms: u32,
    },
    PlayEffect {
        player_id: PlayerId,
        effect: Effect,
        duration_ms: u32,
    },
}

#[derive(Copy, Clone, Debug)]
pub enum KeyboardEvent {
    Key {
        key: KeyboardKey,
        state: ElementState,
    },
}

#[derive(Copy, Clone, Debug)]
pub enum MouseEvent {
    Button {
        key: MouseButton,
        state: ElementState,
    },
    Wheel {
        delta: MouseScrollDelta,
    },
    Delta {
        delta: Vector2f,
    },
    Position {
        position: Vector2f,
    },
}

#[derive(Copy, Clone, Debug)]
pub enum GamepadEvent {
    Button {
        id: GamepadId,
        button: GamepadButton,
        state: ElementState,
    },
    Axis {
        id: GamepadId,
        axis: GamepadAxis,
        value: f32,
    },
    Connected {
        id: GamepadId,
    },
    Disconnected {
        id: GamepadId,
    },
    ForceFeedbackEffectCompleted {
        id: GamepadId,
    },
}

pub enum InputEvent {
    Mouse(MouseEvent),
    Keyboard(KeyboardEvent),
    Gamepad(GamepadEvent),
}

pub const KEYBOARD_KEY_MAX: usize = 256;
pub const GAMEPAD_BUTTON_MAX: usize = GAMEPAD_BUTTON_COUNT * NUM_SUPPORTED_GAMEPADS;

pub struct InputComponent {
    pub(crate) pressed_keyboard_keys: KeyState,
    pub(crate) released_keyboard_keys: KeyState,
    pub(crate) keyboard_keys: KeyState,

    pub(crate) pressed_mouse_buttons: [bool; MOUSE_BUTTON_COUNT],
    pub(crate) released_mouse_buttons: [bool; MOUSE_BUTTON_COUNT],
    pub(crate) mouse_buttons: [bool; MOUSE_BUTTON_COUNT],

    pub(crate) current_mouse_delta: Vector2f,
    pub(crate) current_mouse_position: Vector2f,

    pub(crate) current_mouse_scroll_delta: Vector2f,
    pub(crate) current_mouse_scroll_pixel_delta: Vector2f,

    pub(crate) pressed_gamepad_buttons: GamepadButtonState,
    pub(crate) released_gamepad_buttons: GamepadButtonState,
    pub(crate) gamepad_buttons: GamepadButtonState,
    pub(crate) gamepad_axes: [f32; GAMEPAD_AXIS_COUNT * NUM_SUPPORTED_GAMEPADS],

    pub(crate) gamepad_ids: [Option<GamepadId>; NUM_SUPPORTED_GAMEPADS],
    pub(crate) gamepad_id_to_player: HashMap<GamepadId, PlayerId>,

    pub(crate) haptic_commands: VecDeque<HapticCommand>,
    pub(crate) in_flight_force_feedback: Vec<InFlight>,
}

impl Default for InputComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl InputComponent {
    pub fn new() -> Self {
        Self {
            pressed_keyboard_keys: key_state_zero(),
            released_keyboard_keys: key_state_zero(),
            keyboard_keys: key_state_zero(),

            pressed_mouse_buttons: [false; MOUSE_BUTTON_COUNT],
            released_mouse_buttons: [false; MOUSE_BUTTON_COUNT],
            mouse_buttons: [false; MOUSE_BUTTON_COUNT],

            current_mouse_delta: Vector2f::new(0.0, 0.0),
            current_mouse_position: Vector2f::new(0.0, 0.0),

            current_mouse_scroll_delta: Vector2f::new(0.0, 0.0),
            current_mouse_scroll_pixel_delta: Vector2f::new(0.0, 0.0),

            pressed_gamepad_buttons: gamepad_button_state_zero(),
            released_gamepad_buttons: gamepad_button_state_zero(),
            gamepad_buttons: gamepad_button_state_zero(),
            gamepad_axes: [0.0; GAMEPAD_AXIS_COUNT * NUM_SUPPORTED_GAMEPADS],

            gamepad_ids: [None; NUM_SUPPORTED_GAMEPADS],
            gamepad_id_to_player: HashMap::new(),

            haptic_commands: VecDeque::new(),
            in_flight_force_feedback: Vec::new(),
        }
    }

    pub fn clear_transient_states(&mut self) {
        self.reset_keyboard();
        self.reset_mouse_buttons();
        self.reset_gamepad_buttons();
        self.reset_mouse_motion();
    }

    fn reset_keyboard(&mut self) {
        self.pressed_keyboard_keys = key_state_zero();
        self.released_keyboard_keys = key_state_zero();
    }

    fn reset_mouse_buttons(&mut self) {
        self.pressed_mouse_buttons = [false; MOUSE_BUTTON_COUNT];
        self.released_mouse_buttons = [false; MOUSE_BUTTON_COUNT];
    }

    fn reset_gamepad_buttons(&mut self) {
        self.pressed_gamepad_buttons = gamepad_button_state_zero();
        self.released_gamepad_buttons = gamepad_button_state_zero();
    }

    fn reset_mouse_motion(&mut self) {
        self.current_mouse_delta = Vector2f::new(0.0, 0.0);
        self.current_mouse_scroll_delta = Vector2f::new(0.0, 0.0);
        self.current_mouse_scroll_pixel_delta = Vector2f::new(0.0, 0.0);
    }

    pub fn set_key(&mut self, key: KeyboardKey, state: ElementState) {
        let i = key as usize;
        match state {
            ElementState::Pressed => {
                if self.keyboard_keys[i] {
                    set_key_state(&mut self.pressed_keyboard_keys, i, false);
                } else {
                    set_key_state(&mut self.pressed_keyboard_keys, i, true);
                    set_key_state(&mut self.keyboard_keys, i, true);
                }
            }
            ElementState::Released => {
                set_key_state(&mut self.released_keyboard_keys, i, true);
                set_key_state(&mut self.keyboard_keys, i, false);
            }
        }
    }

    pub fn get_key_pressed(&self, key: KeyboardKey) -> bool {
        self.pressed_keyboard_keys[key as usize]
    }

    pub fn get_key(&self, key: KeyboardKey) -> bool {
        self.keyboard_keys[key as usize]
    }

    pub fn get_key_released(&self, key: KeyboardKey) -> bool {
        self.released_keyboard_keys[key as usize]
    }

    pub fn set_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        let index = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return,
        };

        match state {
            ElementState::Pressed => {
                if self.mouse_buttons[index] {
                    self.pressed_mouse_buttons[index] = false;
                } else {
                    self.pressed_mouse_buttons[index] = true;
                    self.mouse_buttons[index] = true;
                }
            }
            ElementState::Released => {
                self.released_mouse_buttons[index] = true;
                self.mouse_buttons[index] = false;
            }
        }
    }

    pub fn get_mouse_button_pressed(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => self.pressed_mouse_buttons[0],
            MouseButton::Middle => self.pressed_mouse_buttons[1],
            MouseButton::Right => self.pressed_mouse_buttons[2],
            _ => false,
        }
    }

    pub fn get_mouse_button(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => self.mouse_buttons[0],
            MouseButton::Middle => self.mouse_buttons[1],
            MouseButton::Right => self.mouse_buttons[2],
            _ => false,
        }
    }

    pub fn get_mouse_button_released(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => self.released_mouse_buttons[0],
            MouseButton::Middle => self.released_mouse_buttons[1],
            MouseButton::Right => self.released_mouse_buttons[2],
            _ => false,
        }
    }

    pub fn get_mouse_scroll_delta(&self) -> Vector2f {
        self.current_mouse_scroll_delta
    }

    pub fn set_mouse_scroll_delta(&mut self, delta: Vector2f) {
        self.current_mouse_scroll_delta = delta;
    }

    pub fn get_mouse_scroll_pixel_delta(&self) -> Vector2f {
        self.current_mouse_scroll_pixel_delta
    }

    pub fn set_mouse_scroll_pixel_delta(&mut self, delta: Vector2f) {
        self.current_mouse_scroll_pixel_delta = delta;
    }

    pub fn get_mouse_delta(&self) -> Vector2f {
        self.current_mouse_delta
    }

    pub fn set_mouse_delta(&mut self, delta: Vector2f) {
        self.current_mouse_delta = delta;
    }

    pub fn get_mouse_position(&self) -> Vector2f {
        self.current_mouse_position
    }

    pub fn set_mouse_position(&mut self, position: Vector2f) {
        self.current_mouse_position = position;
    }

    pub fn set_gamepad_button(
        &mut self,
        gamepad_id: GamepadId,
        button: GamepadButton,
        state: ElementState,
    ) {
        let player_id = match self.gamepad_id_to_player.get(&gamepad_id) {
            Some(&pid) => pid,
            None => return,
        };
        let i = button as usize + (player_id as usize * GAMEPAD_BUTTON_COUNT);
        match state {
            ElementState::Pressed => {
                if self.gamepad_buttons[i] {
                    set_gamepad_state(&mut self.pressed_gamepad_buttons, i, false);
                } else {
                    set_gamepad_state(&mut self.pressed_gamepad_buttons, i, true);
                    set_gamepad_state(&mut self.gamepad_buttons, i, true);
                }
            }
            ElementState::Released => {
                set_gamepad_state(&mut self.released_gamepad_buttons, i, true);
                set_gamepad_state(&mut self.gamepad_buttons, i, false);
            }
        }
    }

    pub fn get_gamepad_button(&self, player_id: PlayerId, button: GamepadButton) -> bool {
        let i = button as usize + (player_id as usize * GAMEPAD_BUTTON_COUNT);
        self.gamepad_buttons[i]
    }

    pub fn get_gamepad_button_pressed(&self, player_id: PlayerId, button: GamepadButton) -> bool {
        let i = button as usize + (player_id as usize * GAMEPAD_BUTTON_COUNT);
        self.pressed_gamepad_buttons[i]
    }

    pub fn get_gamepad_button_released(&self, player_id: PlayerId, button: GamepadButton) -> bool {
        let i = button as usize + (player_id as usize * GAMEPAD_BUTTON_COUNT);
        self.released_gamepad_buttons[i]
    }

    pub fn set_gamepad_axis(&mut self, gamepad_id: GamepadId, axis: GamepadAxis, raw: f32) {
        let player_id = match self.gamepad_id_to_player.get(&gamepad_id) {
            Some(&pid) => pid,
            None => return,
        };

        let v = if raw.abs() < GAMEPAD_DEADZONE {
            0.0
        } else {
            raw
        };
        let i = axis as usize + (player_id as usize * GAMEPAD_AXIS_COUNT);
        self.gamepad_axes[i] = v;
    }

    pub fn get_gamepad_axis(&self, player_id: PlayerId, axis: GamepadAxis) -> f32 {
        let i = axis as usize + (player_id as usize * GAMEPAD_AXIS_COUNT);
        self.gamepad_axes[i]
    }

    pub fn connect_gamepad(&mut self, gamepad_id: GamepadId) {
        if self.gamepad_id_to_player.contains_key(&gamepad_id) {
            return;
        }

        for i in 0..NUM_SUPPORTED_GAMEPADS {
            if self.gamepad_ids[i].is_none() {
                self.gamepad_ids[i] = Some(gamepad_id);
                let pid = PlayerId::try_from(i as u8).unwrap();
                self.gamepad_id_to_player.insert(gamepad_id, pid);
                return;
            }
        }
    }

    pub fn disconnect_gamepad(&mut self, gamepad_id: GamepadId) {
        if let Some(player_id) = self.gamepad_id_to_player.remove(&gamepad_id) {
            let index = player_id as usize;
            self.gamepad_ids[index] = None;
            self.in_flight_force_feedback
                .retain(|ff| ff.id != gamepad_id);

            let base = index * GAMEPAD_BUTTON_COUNT;
            for j in 0..GAMEPAD_BUTTON_COUNT {
                let k = base + j;
                set_gamepad_state(&mut self.gamepad_buttons, k, false);
                set_gamepad_state(&mut self.pressed_gamepad_buttons, k, false);
                set_gamepad_state(&mut self.released_gamepad_buttons, k, false);
            }
            let base_ax = index * GAMEPAD_AXIS_COUNT;
            for j in 0..GAMEPAD_AXIS_COUNT {
                self.gamepad_axes[base_ax + j] = 0.0;
            }
        }
    }

    pub fn enqueue_rumble(
        &mut self,
        player_id: PlayerId,
        weak: f32,
        strong: f32,
        duration_ms: u32,
    ) {
        self.haptic_commands.push_back(HapticCommand::Rumble {
            player_id,
            weak,
            strong,
            duration_ms,
        });
    }

    pub fn enqueue_effect(&mut self, player_id: PlayerId, effect: Effect, duration_ms: u32) {
        self.haptic_commands.push_back(HapticCommand::PlayEffect {
            player_id,
            effect,
            duration_ms,
        });
    }

    pub fn complete_force_feedback_effect(&mut self, gamepad_id: GamepadId) {
        self.in_flight_force_feedback
            .retain(|ff| ff.id != gamepad_id);
    }
}

impl PillTypeMapKey for InputComponent {
    type Storage = GlobalComponentStorage<InputComponent>;
}

impl GlobalComponent for InputComponent {}
