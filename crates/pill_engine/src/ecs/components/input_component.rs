use crate::ecs::component_storage::GlobalComponentStorage;
use crate::ecs::{ Component, ComponentStorage };

use std::{ 
    any::Any,
    cell::RefCell,
    collections::HashMap,
};
    
use pill_core::PillTypeMapKey;
use winit::dpi::PhysicalPosition;
use winit::event::{VirtualKeyCode, ElementState, MouseButton, MouseScrollDelta};
use anyhow::{Result, Context, Error};

pub enum InputEvent {
    KeyboardKey { key: VirtualKeyCode, state: ElementState },
    MouseKey {  key: MouseButton, state: ElementState },
    MouseWheel { delta: MouseScrollDelta },
    MouseMotion { position: PhysicalPosition<f64> }
}

pub struct InputComponent {
    // Keyboard arrays
    pub current_keyboard_keys: [bool; 163],
    pub previous_keyboard_keys: [bool; 163],

    // Mouse buttons arrays
    pub current_mouse_buttons: [bool; 3],
    pub previous_mouse_buttons: [bool; 3],

    // Mouse positions
    pub current_mouse_pos: PhysicalPosition<f64>,
    pub previous_mouse_pos: PhysicalPosition<f64>,

    // Mouse scrolls wheel deltas
    pub current_mouse_line_delta: (f32, f32),
    pub previous_mouse_line_delta: (f32, f32),

    pub current_mouse_pixel_delta: PhysicalPosition<f64>,
    pub previous_mouse_pixel_delta: PhysicalPosition<f64>,
}

impl InputComponent {

    // - All Input Types Functionalities

    pub fn overwrite_prev_keys(&mut self) {
        for i in 0..163 {
            self.previous_keyboard_keys[i] = self.current_keyboard_keys[i];
        }

        for i in 0..3 {
            self.previous_mouse_buttons[i] = self.current_mouse_buttons[i];
        }
        
        self.previous_mouse_pos = self.current_mouse_pos;

        self.previous_mouse_pixel_delta = self.current_mouse_pixel_delta;
        self.previous_mouse_line_delta = self.current_mouse_line_delta;
    }

    // - Keyboard Key Functionalities

    pub fn press_key(&mut self, key: usize) {
        self.current_keyboard_keys[key] = true;
    }

    pub fn release_key(&mut self, key: usize) {
        self.current_keyboard_keys[key] = false;
    }

    pub fn is_key_pressed(&self, key: VirtualKeyCode) -> bool {
        &self.current_keyboard_keys[key as usize] == &true && &self.previous_keyboard_keys[key as usize] == &true 
    }

    pub fn is_key_clicked(&self, key: VirtualKeyCode) -> bool {
        &self.current_keyboard_keys[key as usize] == &true && &self.previous_keyboard_keys[key as usize] == &false 
    }

    pub fn is_key_released(&self, key: VirtualKeyCode) -> bool {
        &self.current_keyboard_keys[key as usize] == &false && &self.previous_keyboard_keys[key as usize] == &true
    }

    // - Mouse Buttons Functionalities

    pub fn press_left_mouse_button(&mut self) {
        self.current_mouse_buttons[0] = true;
    } 

    pub fn release_left_mouse_button(&mut self) {
        self.current_mouse_buttons[0] = false;
    }

    pub fn press_middle_mouse_button(&mut self) {
        self.current_mouse_buttons[1] = true;
    }

    pub fn release_middle_mouse_button(&mut self) {
        self.current_mouse_buttons[1] = false;
    }

    pub fn press_right_mouse_button(&mut self) {
        self.current_mouse_buttons[2] = true;
    }

    pub fn release_right_mouse_button(&mut self) {
        self.current_mouse_buttons[2] = false;
    }

    pub fn is_mouse_button_pressed(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => {
                if &self.current_mouse_buttons[0] == &true && &self.previous_mouse_buttons[0] == &true {
                    return true
                }
                else {
                    return false
                }
            },
            MouseButton::Middle => {
                if &self.current_mouse_buttons[1] == &true && &self.previous_mouse_buttons[1] == &true {
                    return true
                }
                else {
                    return false
                }
            },
            MouseButton::Right => {
                if &self.current_mouse_buttons[2] == &true && &self.previous_mouse_buttons[2] == &true {
                    return true
                }
                else {
                    return false
                }
            },
            _ => false
        }
    }

    pub fn is_mouse_button_clicked(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => {
                if &self.current_mouse_buttons[0] == &true && &self.previous_mouse_buttons[0] == &false {
                    return true
                }
                else {
                    return false
                }
            },
            MouseButton::Middle => {
                if &self.current_mouse_buttons[1] == &true && &self.previous_mouse_buttons[1] == &false {
                    return true
                }
                else {
                    return false
                }
            },
            MouseButton::Right => {
                if &self.current_mouse_buttons[2] == &true && &self.previous_mouse_buttons[2] == &false {
                    return true
                }
                else {
                    return false
                }
            },
            _ => false
        }
    }

    pub fn is_mouse_button_released(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => {
                if &self.current_mouse_buttons[0] == &false && &self.previous_mouse_buttons[0] == &true {
                    return true
                }
                else {
                    return false
                }
            },
            MouseButton::Middle => {
                if &self.current_mouse_buttons[1] == &false && &self.previous_mouse_buttons[1] == &true {
                    return true
                }
                else {
                    return false
                }
            },
            MouseButton::Right => {
                if &self.current_mouse_buttons[2] == &false && &self.previous_mouse_buttons[2] == &true {
                    return true
                }
                else {
                    return false
                }
            },
            _ => false
        }
    }

    // - Mouse Line Delta Functionality

    pub fn get_current_mouse_line_delta(&self) -> (f32, f32) {
        self.current_mouse_line_delta
    }

    pub fn set_current_mouse_line_delta(&mut self, x: f32, y: f32) {
        self.current_mouse_line_delta = (x, y);
    }

    pub fn get_current_mouse_pixel_delta(&self) -> PhysicalPosition<f64> {
        self.current_mouse_pixel_delta
    }

    pub fn set_current_mouse_pixel_delta(&mut self, pos: PhysicalPosition<f64>) {
        self.current_mouse_pixel_delta = pos;
    }

    // - Mouse Motion Functionality

    pub fn get_current_mouse_pos(&self) -> PhysicalPosition<f64> {
        self.current_mouse_pos
    }

    pub fn set_current_mouse_pos(&mut self, pos: PhysicalPosition<f64>) {
        self.current_mouse_pos = pos;
    }
}

impl Default for InputComponent {
    fn default() -> Self {
        Self { 
            current_keyboard_keys: [false; 163],
            previous_keyboard_keys: [false; 163],

            current_mouse_buttons: [false; 3],
            previous_mouse_buttons: [false; 3],

            current_mouse_pos: PhysicalPosition { x: 0.0, y: 0.0},
            previous_mouse_pos: PhysicalPosition { x: 0.0, y: 0.0},

            current_mouse_line_delta: (0.0, 0.0),
            previous_mouse_line_delta: (0.0, 0.0),

            current_mouse_pixel_delta: PhysicalPosition {x: 0.0, y: 0.0},
            previous_mouse_pixel_delta: PhysicalPosition {x: 0.0, y: 0.0},
        }
    }
}

impl PillTypeMapKey for InputComponent {
    type Storage = GlobalComponentStorage<InputComponent>; 
}

impl Component for InputComponent {
   
}
