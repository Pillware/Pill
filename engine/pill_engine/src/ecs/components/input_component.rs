use crate::{
    engine::{ KeyboardKey, MouseButton },
    ecs::{ GlobalComponent, GlobalComponentStorage },
};

use pill_core::{ PillTypeMapKey, Vector2f };
use bitvec::prelude::*;

use std::{
    any::Any,
    cell::RefCell,
    collections::HashMap,
};
use winit::dpi::PhysicalPosition;
use winit::event::{ ElementState, MouseScrollDelta };
use anyhow::{ Result, Context, Error };

pub const GAMEPAD_DEADZONE: f32 = 0.05; // Deadzone for gamepad axes

pub const KEYBOARD_KEY_COUNT: usize = KeyboardKey::F35 as usize + 1; // Total number of keys in KeyboardKey enum
pub const MOUSE_BUTTON_COUNT: usize = 3; // Left, Middle, Right

/// Gamepad enums with more descriptive names
#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum GamepadButton {
    A = 0, B, X, Y,
    LeftBumper, RightBumper,
    LeftTrigger, RightTrigger, // listed twice for convenience
    Back, Start, Mode,
    LeftStick, RightStick,
    DPadUp, DPadDown, DPadLeft, DPadRight,
}
pub const GAMEPAD_BUTTON_COUNT: usize = GamepadButton::DPadRight as usize + 1;

#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum GamepadAxis {
    LeftStickX = 0, LeftStickY,
    RightStickX, RightStickY,
    LeftTrigger, RightTrigger,
}
pub const GAMEPAD_AXIS_COUNT: usize = GamepadAxis::RightTrigger as usize + 1;

pub enum InputEvent {
    KeyboardKey { key: KeyboardKey, state: ElementState },
    MouseButton { key: MouseButton, state: ElementState },
    MouseWheel { delta: MouseScrollDelta },
    MouseDelta { delta: Vector2f },
    MousePosition { position: Vector2f },
    GamepadButton { button: GamepadButton, state: ElementState },
    GamepadAxis { axis: GamepadAxis, value: f32 },
}

pub type KeyState = BitArray<[u64; 4]>; // actually we have less but this is fine
pub type GamepadButtonState = BitArray<[u32; 1]>; // we have 17 buttons

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
    pub(crate) gamepad_axes: [f32; GAMEPAD_AXIS_COUNT],
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
            gamepad_axes: [0.0; GAMEPAD_AXIS_COUNT],
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
                }
                else {
                    self.pressed_keyboard_keys.set(i, true);
                    self.keyboard_keys.set(i, true);
                }
            },
            ElementState::Released => {
                self.released_keyboard_keys.set(i, true);
                self.keyboard_keys.set(i, false);
            },
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
            _ => return
        };

        match state {
            ElementState::Pressed => {
                if self.mouse_buttons[index] {
                    self.pressed_mouse_buttons[index] = false;
                }
                else {
                    self.pressed_mouse_buttons[index] = true;
                    self.mouse_buttons[index] = true;
                }
            },
            ElementState::Released => {
                self.released_mouse_buttons[index] = true;
                self.mouse_buttons[index] = false;
            }
        }
    }

    pub fn get_mouse_button_pressed(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => self.pressed_mouse_buttons[0],
            MouseButton::Middle =>  self.pressed_mouse_buttons[1],
            MouseButton::Right => self.pressed_mouse_buttons[2],
            _ => false
        }
    }

    pub fn get_mouse_button(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left =>  self.mouse_buttons[0],
            MouseButton::Middle => self.mouse_buttons[1],
            MouseButton::Right =>  self.mouse_buttons[2],
            _ => false
        }
    }

    pub fn get_mouse_button_released(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => self.released_mouse_buttons[0],
            MouseButton::Middle => self.released_mouse_buttons[1],
            MouseButton::Right => self.released_mouse_buttons[2],
            _ => false
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

    // Gamepad buttons
    pub fn set_gamepad_button(&mut self, button: GamepadButton, state: ElementState) {
        let i = button as usize;
        match state {
            ElementState::Pressed => {
                if self.gamepad_buttons[i] {
                    self.pressed_gamepad_buttons.set(i, false);
                }
                else {
                    self.pressed_gamepad_buttons.set(i, true);
                    self.gamepad_buttons.set(i, true);
                }
            },
            ElementState::Released => {
                self.released_gamepad_buttons.set(i, true);
                self.gamepad_buttons.set(i, false);
            },
        }
    }

    pub fn get_gamepad_button(&self, button: GamepadButton) -> bool {
        self.gamepad_buttons[button as usize]
    }

    pub fn get_gamepad_button_pressed(&self, button: GamepadButton) -> bool {
        self.pressed_gamepad_buttons[button as usize]
    }

    pub fn get_gamepad_button_released(&self, button: GamepadButton) -> bool {
        self.released_gamepad_buttons[button as usize]
    }

    pub fn set_gamepad_axis(&mut self, axis: GamepadAxis, raw: f32) {
        let v = if raw.abs() < GAMEPAD_DEADZONE { 0.0 } else { raw };
        self.gamepad_axes[axis as usize] = raw;
    }

    pub fn get_gamepad_axis(&self, axis: GamepadAxis) -> f32 {
        self.gamepad_axes[axis as usize]
    }
}

impl PillTypeMapKey for InputComponent {
    type Storage = GlobalComponentStorage<InputComponent>;
}

impl GlobalComponent for InputComponent { }
