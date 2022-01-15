use crate::ecs::{ GlobalComponent, GlobalComponentStorage };

use pill_core::PillTypeMapKey;

use std::{ 
    any::Any,
    cell::RefCell,
    collections::HashMap,
};
use winit::dpi::PhysicalPosition;
use winit::event::{ VirtualKeyCode, ElementState, MouseButton, MouseScrollDelta };
use anyhow::{Result, Context, Error};

pub enum InputEvent {
    KeyboardKey { key: VirtualKeyCode, state: ElementState },
    MouseButton {  key: MouseButton, state: ElementState },
    MouseWheel { delta: MouseScrollDelta },
    MouseMotion { delta: (f64, f64) },
    MousePosition { position: PhysicalPosition<f64> }
}

pub struct InputComponent {
    // Keyboard arrays
    pub(crate) pressed_keyboard_keys: [bool; 163],
    pub(crate) released_keyboard_keys: [bool; 163],
    pub(crate) keyboard_keys: [bool; 163],

    // Mouse buttons arrays
    pub(crate) pressed_mouse_buttons: [bool; 3],
    pub(crate) released_mouse_buttons: [bool; 3],
    pub(crate) mouse_buttons: [bool; 3],

    // Mouse positions
    pub(crate) current_mouse_position: PhysicalPosition<f64>,

    pub(crate) current_mouse_motion: (f64, f64),

    // Mouse scroll wheels delta
    pub(crate) current_mouse_scroll_line_delta: (f32, f32),

    pub(crate) current_mouse_scroll_delta: PhysicalPosition<f64>,
}

impl InputComponent {
    pub(crate) fn new() -> Self {
        Self { 
            pressed_keyboard_keys: [false; 163],
            released_keyboard_keys: [false; 163],
            keyboard_keys: [false; 163],

            pressed_mouse_buttons: [false; 3],
            released_mouse_buttons: [false; 3],
            mouse_buttons: [false; 3],
    
            current_mouse_motion: (0.0, 0.0),

            current_mouse_position: PhysicalPosition { x: 0.0, y: 0.0 },

            current_mouse_scroll_line_delta: (0.0, 0.0),
    
            current_mouse_scroll_delta: PhysicalPosition { x: 0.0, y: 0.0 },
        }
    }

    pub(crate) fn set_keys(&mut self) {
        for i in 0..163 {
            if self.keyboard_keys[i] && self.pressed_keyboard_keys[i] {
                self.pressed_keyboard_keys[i] = false;
            }
            if !self.keyboard_keys[i] && self.released_keyboard_keys[i] {
                self.released_keyboard_keys[i] = false;
            }
        }
    }

    pub(crate) fn set_mouse_buttons(&mut self) {
        for i in 0..3 {
            if self.mouse_buttons[i] && self.pressed_mouse_buttons[i] {
                self.pressed_mouse_buttons[i] = false;
            }
            if !self.mouse_buttons[i] && self.released_mouse_buttons[i] {
                self.released_mouse_buttons[i] = false;
            }
        }
    }

    // Keyboard keys
    pub(crate) fn set_key(&mut self, key: VirtualKeyCode, state: ElementState) {
        match state {
            ElementState::Pressed => {
                if self.keyboard_keys[key as usize] {
                    self.pressed_keyboard_keys[key as usize] = false;
                }
                else {
                    self.pressed_keyboard_keys[key as usize] = true;
                    self.keyboard_keys[key as usize] = true;
                }
            },
            ElementState::Released => {
                self.released_keyboard_keys[key as usize] = true;
                self.keyboard_keys[key as usize] = false;
            },
        }
    }

    pub fn get_key_pressed(&self, key: VirtualKeyCode) -> bool {
        self.pressed_keyboard_keys[key as usize]
    }

    pub fn get_key(&self, key: VirtualKeyCode) -> bool {
        self.keyboard_keys[key as usize]
    }

    pub fn get_key_released(&self, key: VirtualKeyCode) -> bool {
        self.released_keyboard_keys[key as usize]
    }

    // Mouse buttons
    pub(crate) fn set_mouse_button(&mut self, button: MouseButton, state: ElementState) {
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
    pub fn get_mouse_scroll_line_delta(&self) -> (f32, f32) {
        self.current_mouse_scroll_line_delta
    }

    pub(crate) fn set_mouse_scroll_line_delta(&mut self, x: f32, y: f32) {
        self.current_mouse_scroll_line_delta = (x, y);
    }

    pub fn get_mouse_scroll_delta(&self) -> PhysicalPosition<f64> {
        self.current_mouse_scroll_delta
    }

    pub(crate) fn set_mouse_scroll_delta(&mut self, delta: PhysicalPosition<f64>) {
        self.current_mouse_scroll_delta = delta;
    }

    // - Mouse motion
      
    pub fn get_mouse_motion(&self) -> (f64, f64) {
        self.current_mouse_motion
    }

    pub(crate) fn set_mouse_motion(&mut self, delta: (f64, f64)) {
        self.current_mouse_motion = delta;
    }

    pub fn get_mouse_position(&self) -> PhysicalPosition<f64> {
        self.current_mouse_position
    }

    pub(crate) fn set_mouse_position(&mut self, position: PhysicalPosition<f64>) {
        self.current_mouse_position = position;
    }
}

impl PillTypeMapKey for InputComponent {
    type Storage = GlobalComponentStorage<InputComponent>; 
}

impl GlobalComponent for InputComponent { } 
