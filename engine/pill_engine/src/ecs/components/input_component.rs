use crate::{
    ecs::{GlobalComponent, GlobalComponentStorage},
    engine::{KeyboardKey, MouseButton},
    internal::NUM_SUPPORTED_GAMEPADS,
};

use bitvec::prelude::*;
use pill_core::{PillTypeMapKey, Vector2f};

use anyhow::{Error, Result};
use gilrs::{ff::Effect, GamepadId};
use std::{
    collections::{HashMap, VecDeque},
    time::Instant,
};
use winit::event::{ElementState, MouseScrollDelta};

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
    RightTrigger, // listed twice for convenience
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
    /// Simple dual-rumble (weak/strong in [0.0, 1.0]) for a duration in milliseconds
    Rumble {
        player_id: PlayerId,
        weak: f32,
        strong: f32,
        duration_ms: u32,
    },
    /// Play a prebuilt gilrs::ff::Effect on the given player_id
    PlayEffect {
        player_id: PlayerId,
        effect: Effect,
        duration_ms: u32,
    },
}

pub struct InFlight {
    pub(crate) id: GamepadId,
    pub(crate) effect: Effect,
    pub(crate) end_at: Instant,
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

pub type KeyState = BitArray<[u64; 4]>; // actually we have less but this is fine
pub type GamepadButtonState = BitArray<[u32; NUM_SUPPORTED_GAMEPADS]>; // we have 17 buttons

pub struct InputComponent {
    // Keyboard arrays
    pub(crate) pressed_keyboard_keys: KeyState,
    pub(crate) released_keyboard_keys: KeyState,
    pub(crate) keyboard_keys: KeyState,

    // Mouse buttons arrays
    pub(crate) pressed_mouse_buttons: [bool; MOUSE_BUTTON_COUNT],
    pub(crate) released_mouse_buttons: [bool; MOUSE_BUTTON_COUNT],
    pub(crate) mouse_buttons: [bool; MOUSE_BUTTON_COUNT],

    // Mouse motion
    pub(crate) current_mouse_delta: Vector2f,
    pub(crate) current_mouse_position: Vector2f,

    // Mouse scroll wheels delta
    pub(crate) current_mouse_scroll_delta: Vector2f,
    pub(crate) current_mouse_scroll_pixel_delta: Vector2f,

    // Gamepad buttons and axes
    pub(crate) pressed_gamepad_buttons: GamepadButtonState,
    pub(crate) released_gamepad_buttons: GamepadButtonState,
    pub(crate) gamepad_buttons: GamepadButtonState,
    pub(crate) gamepad_axes: [f32; GAMEPAD_AXIS_COUNT * NUM_SUPPORTED_GAMEPADS],

    // Is gamepad Connected
    pub(crate) gamepad_ids: [Option<GamepadId>; NUM_SUPPORTED_GAMEPADS],
    pub(crate) gamepad_id_to_player: HashMap<GamepadId, PlayerId>,

    // Haptics commands queue and in-flight effects
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
            pressed_keyboard_keys: KeyState::ZERO,
            released_keyboard_keys: KeyState::ZERO,
            keyboard_keys: KeyState::ZERO,

            pressed_mouse_buttons: [false; MOUSE_BUTTON_COUNT],
            released_mouse_buttons: [false; MOUSE_BUTTON_COUNT],
            mouse_buttons: [false; MOUSE_BUTTON_COUNT],

            current_mouse_delta: Vector2f::new(0.0, 0.0),
            current_mouse_position: Vector2f::new(0.0, 0.0),

            current_mouse_scroll_delta: Vector2f::new(0.0, 0.0),
            current_mouse_scroll_pixel_delta: Vector2f::new(0.0, 0.0),

            pressed_gamepad_buttons: GamepadButtonState::ZERO,
            released_gamepad_buttons: GamepadButtonState::ZERO,
            gamepad_buttons: GamepadButtonState::ZERO,
            gamepad_axes: [0.0; GAMEPAD_AXIS_COUNT * NUM_SUPPORTED_GAMEPADS],

            gamepad_ids: [None; NUM_SUPPORTED_GAMEPADS],
            gamepad_id_to_player: HashMap::new(),

            haptic_commands: VecDeque::new(),
            in_flight_force_feedback: Vec::new(),
        }
    }

    // frame-reset
    pub fn clear_transient_states(&mut self) {
        self.reset_keyboard();
        self.reset_mouse_buttons();
        self.reset_gamepad_buttons();
        self.reset_mouse_motion();
    }

    fn reset_keyboard(&mut self) {
        self.pressed_keyboard_keys = KeyState::ZERO;
        self.released_keyboard_keys = KeyState::ZERO;
    }

    fn reset_mouse_buttons(&mut self) {
        self.pressed_mouse_buttons = [false; MOUSE_BUTTON_COUNT];
        self.released_mouse_buttons = [false; MOUSE_BUTTON_COUNT];
    }

    fn reset_gamepad_buttons(&mut self) {
        self.pressed_gamepad_buttons = GamepadButtonState::ZERO;
        self.released_gamepad_buttons = GamepadButtonState::ZERO;
    }

    fn reset_mouse_motion(&mut self) {
        self.current_mouse_delta = Vector2f::new(0.0, 0.0);
        self.current_mouse_scroll_delta = Vector2f::new(0.0, 0.0);
        self.current_mouse_scroll_pixel_delta = Vector2f::new(0.0, 0.0);
    }

    // Keyboard keys
    pub fn set_key(&mut self, key: KeyboardKey, state: ElementState) {
        let i = key as usize;
        match state {
            ElementState::Pressed => {
                if self.keyboard_keys[i] {
                    self.pressed_keyboard_keys.set(i, false);
                } else {
                    self.pressed_keyboard_keys.set(i, true);
                    self.keyboard_keys.set(i, true);
                }
            }
            ElementState::Released => {
                self.released_keyboard_keys.set(i, true);
                self.keyboard_keys.set(i, false);
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

    // Mouse buttons
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

    // Mouse scroll
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

    // - Mouse motion

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

    // Gamepad buttons (get by PlayerId, set by Gamepad's Id)
    pub fn set_gamepad_button(
        &mut self,
        gamepad_id: GamepadId,
        button: GamepadButton,
        state: ElementState,
    ) {
        let player_id = match self.gamepad_id_to_player.get(&gamepad_id) {
            Some(&pid) => pid,
            None => return, // gamepad not recognized
        };
        let i = button as usize + (player_id as usize * u32::BITS as usize);
        match state {
            ElementState::Pressed => {
                if self.gamepad_buttons[i] {
                    self.pressed_gamepad_buttons.set(i, false);
                } else {
                    self.pressed_gamepad_buttons.set(i, true);
                    self.gamepad_buttons.set(i, true);
                }
            }
            ElementState::Released => {
                self.released_gamepad_buttons.set(i, true);
                self.gamepad_buttons.set(i, false);
            }
        }
    }

    pub fn get_gamepad_button(&self, player_id: PlayerId, button: GamepadButton) -> bool {
        let i = button as usize + (player_id as usize * u32::BITS as usize);
        self.gamepad_buttons[i]
    }

    pub fn get_gamepad_button_pressed(&self, player_id: PlayerId, button: GamepadButton) -> bool {
        let i = button as usize + (player_id as usize * u32::BITS as usize);
        self.pressed_gamepad_buttons[i]
    }

    pub fn get_gamepad_button_released(&self, player_id: PlayerId, button: GamepadButton) -> bool {
        let i = button as usize + (player_id as usize * u32::BITS as usize);
        self.released_gamepad_buttons[i]
    }

    pub fn set_gamepad_axis(&mut self, gamepad_id: GamepadId, axis: GamepadAxis, raw: f32) {
        let player_id = match self.gamepad_id_to_player.get(&gamepad_id) {
            Some(&pid) => pid,
            None => return, // gamepad not recognized
        };

        let v = if raw.abs() < GAMEPAD_DEADZONE {
            0.0
        } else {
            raw
        };
        let i = axis as usize + (player_id as usize * GAMEPAD_AXIS_COUNT);
        println!("Gamepad {:?} axis {:?} set to {}", gamepad_id, axis, v);
        self.gamepad_axes[i] = v;
    }

    pub fn get_gamepad_axis(&self, player_id: PlayerId, axis: GamepadAxis) -> f32 {
        let i = axis as usize + (player_id as usize * GAMEPAD_AXIS_COUNT);
        self.gamepad_axes[i]
    }

    pub fn connect_gamepad(&mut self, gamepad_id: GamepadId) {
        if self.gamepad_id_to_player.contains_key(&gamepad_id) {
            return; // already connected
        }

        for i in 0..NUM_SUPPORTED_GAMEPADS {
            if self.gamepad_ids[i].is_none() {
                self.gamepad_ids[i] = Some(gamepad_id);
                // Player Ids are assigned in order of connection
                let pid = PlayerId::try_from(i as u8).unwrap();
                self.gamepad_id_to_player.insert(gamepad_id, pid);
                println!(
                    "Gamepad connected: id {}, assigned to player {:?}",
                    gamepad_id, pid
                );
                return;
            }
        }

        println!(
            "Gamepad connected: id {}, but no free slots available",
            gamepad_id
        );
    }

    pub fn disconnect_gamepad(&mut self, gamepad_id: GamepadId) {
        if let Some(player_id) = self.gamepad_id_to_player.remove(&gamepad_id) {
            let index = player_id as usize;
            self.gamepad_ids[index] = None;
            self.in_flight_force_feedback
                .retain(|ff| ff.id != gamepad_id);

            // Reset buttons and axes for this gamepad
            let base = index * u32::BITS as usize;
            for j in 0..GAMEPAD_BUTTON_COUNT {
                let k = base + j;
                self.gamepad_buttons.set(k, false);
                self.pressed_gamepad_buttons.set(k, false);
                self.released_gamepad_buttons.set(k, false);
            }
            let base_ax = index * GAMEPAD_AXIS_COUNT;
            for j in 0..GAMEPAD_AXIS_COUNT {
                self.gamepad_axes[base_ax + j] = 0.0;
            }
            println!(
                "Gamepad disconnected: id {}, was player {:?}",
                gamepad_id, player_id
            );
        } else {
            println!(
                "Gamepad disconnected: id {}, but was not recognized",
                gamepad_id
            );
        }
    }

    // Haptics functions
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
