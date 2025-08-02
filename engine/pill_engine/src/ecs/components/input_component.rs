#![cfg(feature = "rendering")]

use crate::{
    engine::{KeyboardKey, MouseButton},
    ecs::{GlobalComponent, GlobalComponentStorage},
};
use pill_core::{PillTypeMapKey, Vector2f};
use winit::event::{ElementState, MouseScrollDelta};

//
// ────────────────────────── Game-pad enums ──────────────────────────
//
#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum GamepadButton {
    A = 0,  B,  X,  Y,
    LeftBumper,  RightBumper,
    Back, Start, Guide,
    LeftStick, RightStick,
    DPadUp, DPadDown, DPadLeft, DPadRight,
}
pub const GAMEPAD_BUTTON_COUNT: usize = GamepadButton::DPadRight as usize + 1;

#[repr(u8)]
#[derive(Copy, Clone, Debug)]
pub enum GamepadAxis {
    LeftStickX = 0, LeftStickY,
    RightStickX,   RightStickY,
    LeftTrigger,   RightTrigger,        // 0 … 1
}
pub const GAMEPAD_AXIS_COUNT: usize = GamepadAxis::RightTrigger as usize + 1;

//
// ──────────────────────────── InputEvent ────────────────────────────
//
pub enum InputEvent {
    KeyboardKey  { key: KeyboardKey, state: ElementState },
    MouseButton  { key: MouseButton, state: ElementState },
    MouseWheel   { delta: MouseScrollDelta },
    MouseDelta   { delta: Vector2f },
    MousePosition{ position: Vector2f },

    GamepadButton{ button: GamepadButton, state: ElementState },
    GamepadAxis  { axis: GamepadAxis, value: f32 },
}

//
// ─────────────────────────── InputComponent ─────────────────────────
//
pub struct InputComponent {
    // keyboard
    pressed_keyboard_keys:   [bool; 163],
    released_keyboard_keys:  [bool; 163],
    keyboard_keys:           [bool; 163],

    // mouse
    pressed_mouse_buttons:   [bool; 3],
    released_mouse_buttons:  [bool; 3],
    mouse_buttons:           [bool; 3],

    current_mouse_delta:           Vector2f,
    current_mouse_position:        Vector2f,
    current_mouse_scroll_delta:    Vector2f,
    current_mouse_scroll_pixel_delta: Vector2f,

    // game-pad
    pressed_gamepad_buttons:  [bool; GAMEPAD_BUTTON_COUNT],
    released_gamepad_buttons: [bool; GAMEPAD_BUTTON_COUNT],
    gamepad_buttons:          [bool; GAMEPAD_BUTTON_COUNT],
    gamepad_axes:             [f32; GAMEPAD_AXIS_COUNT],
}

impl InputComponent {
    pub fn new() -> Self {
        Self {
            pressed_keyboard_keys:  [false; 163],
            released_keyboard_keys: [false; 163],
            keyboard_keys:          [false; 163],

            pressed_mouse_buttons:  [false; 3],
            released_mouse_buttons: [false; 3],
            mouse_buttons:          [false; 3],

            current_mouse_delta:           Vector2f::new(0.0, 0.0),
            current_mouse_position:        Vector2f::new(0.0, 0.0),
            current_mouse_scroll_delta:    Vector2f::new(0.0, 0.0),
            current_mouse_scroll_pixel_delta: Vector2f::new(0.0, 0.0),

            pressed_gamepad_buttons:  [false; GAMEPAD_BUTTON_COUNT],
            released_gamepad_buttons: [false; GAMEPAD_BUTTON_COUNT],
            gamepad_buttons:          [false; GAMEPAD_BUTTON_COUNT],
            gamepad_axes:             [0.0;  GAMEPAD_AXIS_COUNT],
        }
    }

    // ───────── frame-reset ─────────
    pub fn clear_transients(&mut self) {
        self.reset_keyboard();
        self.reset_mouse_buttons();
        self.reset_gamepad_buttons();
        self.reset_mouse_motion();
    }
    fn reset_keyboard(&mut self) {
        for i in 0..163 {
            if  self.keyboard_keys[i] { self.pressed_keyboard_keys[i]  = false; }
            if !self.keyboard_keys[i] { self.released_keyboard_keys[i] = false; }
        }
    }
    fn reset_mouse_buttons(&mut self) {
        for i in 0..3 {
            if  self.mouse_buttons[i] { self.pressed_mouse_buttons[i]  = false; }
            if !self.mouse_buttons[i] { self.released_mouse_buttons[i] = false; }
        }
    }
    fn reset_gamepad_buttons(&mut self) {
        for i in 0..GAMEPAD_BUTTON_COUNT {
            if  self.gamepad_buttons[i] { self.pressed_gamepad_buttons[i]  = false; }
            if !self.gamepad_buttons[i] { self.released_gamepad_buttons[i] = false; }
        }
    }
    fn reset_mouse_motion(&mut self) {
        self.current_mouse_delta            = Vector2f::new(0.0, 0.0);
        self.current_mouse_scroll_delta     = Vector2f::new(0.0, 0.0);
        self.current_mouse_scroll_pixel_delta = Vector2f::new(0.0, 0.0);
    }

    // ───────── keyboard API ─────────
    pub fn set_key(&mut self, key: KeyboardKey, state: ElementState) {
        let i = key as usize;
        match state {
            ElementState::Pressed => {
                if self.keyboard_keys[i] {
                    self.pressed_keyboard_keys[i] = false;
                } else {
                    self.pressed_keyboard_keys[i] = true;
                    self.keyboard_keys[i] = true;
                }
            }
            ElementState::Released => {
                self.released_keyboard_keys[i] = true;
                self.keyboard_keys[i] = false;
            }
        }
    }
    pub fn get_key          (&self, k: KeyboardKey) -> bool { self.keyboard_keys[k as usize] }
    pub fn get_key_pressed  (&self, k: KeyboardKey) -> bool { self.pressed_keyboard_keys[k as usize] }
    pub fn get_key_released (&self, k: KeyboardKey) -> bool { self.released_keyboard_keys[k as usize] }

    // ───────── mouse API ─────────
    pub fn set_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        let idx = match button {
            MouseButton::Left   => 0,
            MouseButton::Middle => 1,
            MouseButton::Right  => 2,
            _ => return,
        };
        match state {
            ElementState::Pressed => {
                if self.mouse_buttons[idx] {
                    self.pressed_mouse_buttons[idx] = false;
                } else {
                    self.pressed_mouse_buttons[idx] = true;
                    self.mouse_buttons[idx] = true;
                }
            }
            ElementState::Released => {
                self.released_mouse_buttons[idx] = true;
                self.mouse_buttons[idx] = false;
            }
        }
    }
    pub fn get_mouse_button(&self, b: MouseButton) -> bool {
        match b { MouseButton::Left => self.mouse_buttons[0],
                  MouseButton::Middle => self.mouse_buttons[1],
                  MouseButton::Right => self.mouse_buttons[2],
                  _ => false }
    }
    pub fn get_mouse_button_pressed(&self, b: MouseButton) -> bool {
        match b { MouseButton::Left => self.pressed_mouse_buttons[0],
                  MouseButton::Middle => self.pressed_mouse_buttons[1],
                  MouseButton::Right => self.pressed_mouse_buttons[2],
                  _ => false }
    }
    pub fn get_mouse_button_released(&self, b: MouseButton) -> bool {
        match b { MouseButton::Left => self.released_mouse_buttons[0],
                  MouseButton::Middle => self.released_mouse_buttons[1],
                  MouseButton::Right => self.released_mouse_buttons[2],
                  _ => false }
    }

    pub fn set_mouse_scroll_delta(&mut self, d: Vector2f) { self.current_mouse_scroll_delta = d; }
    pub fn set_mouse_scroll_pixel_delta(&mut self, d: Vector2f){ self.current_mouse_scroll_pixel_delta = d; }
    pub fn set_mouse_delta(&mut self, d: Vector2f) { self.current_mouse_delta = d; }
    pub fn set_mouse_position(&mut self, p: Vector2f){ self.current_mouse_position = p; }

    pub fn get_mouse_delta(&self) -> Vector2f { self.current_mouse_delta }
    pub fn get_mouse_position(&self) -> Vector2f { self.current_mouse_position }
    pub fn get_mouse_scroll_delta(&self) -> Vector2f { self.current_mouse_scroll_delta }
    pub fn get_mouse_scroll_pixel_delta(&self) -> Vector2f { self.current_mouse_scroll_pixel_delta }

    // ───────── game-pad API ─────────
    pub fn set_gamepad_button(&mut self, b: GamepadButton, state: ElementState) {
        let i = b as usize;
        match state {
            ElementState::Pressed => {
                if self.gamepad_buttons[i] {
                    self.pressed_gamepad_buttons[i] = false;
                } else {
                    self.pressed_gamepad_buttons[i] = true;
                    self.gamepad_buttons[i] = true;
                }
            }
            ElementState::Released => {
                self.released_gamepad_buttons[i] = true;
                self.gamepad_buttons[i] = false;
            }
        }
    }
    pub fn set_gamepad_axis(&mut self, a: GamepadAxis, raw: f32) {
        let v = if raw.abs() < 0.05 { 0.0 } else { raw };
        self.gamepad_axes[a as usize] = v;
    }
    pub fn get_gamepad_axis(&self, a: GamepadAxis) -> f32 { self.gamepad_axes[a as usize] }
    pub fn get_gamepad_button(&self, b: GamepadButton) -> bool { self.gamepad_buttons[b as usize] }
    pub fn get_gamepad_button_pressed(&self, b: GamepadButton) -> bool { self.pressed_gamepad_buttons[b as usize] }
    pub fn get_gamepad_button_released(&self, b: GamepadButton) -> bool { self.released_gamepad_buttons[b as usize] }
}

// ───────── ECS glue ─────────
impl PillTypeMapKey for InputComponent {
    type Storage = GlobalComponentStorage<InputComponent>;
}
impl GlobalComponent for InputComponent {}

