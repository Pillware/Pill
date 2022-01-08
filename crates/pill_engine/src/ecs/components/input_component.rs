use crate::ecs::{ Component, ComponentStorage, GlobalComponentStorage };

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
    MouseKey {  key: MouseButton, state: ElementState },
    MouseWheel { delta: MouseScrollDelta },
    MouseMotion { position: PhysicalPosition<f64> }
}

pub struct InputComponent {
    // Keyboard arrays
    pub(crate) pressed_keyboard_keys: [bool; 163],
    pub(crate) released_keyboard_keys: [bool; 163],
    pub(crate) held_keyboard_keys: [bool; 163],

    // Mouse buttons arrays
    pub(crate) pressed_mouse_buttons: [bool; 3],
    pub(crate) released_mouse_buttons: [bool; 3],
    pub(crate) held_mouse_buttons: [bool; 3],

    // Mouse positions
    pub(crate) current_mouse_position: PhysicalPosition<f64>,
    pub(crate) previous_mouse_position: PhysicalPosition<f64>,

    // Mouse scrolls wheel deltas
    pub(crate) current_mouse_line_delta: (f32, f32),
    pub(crate) previous_mouse_line_delta: (f32, f32),

    pub(crate) current_mouse_pixel_delta: PhysicalPosition<f64>,
    pub(crate) previous_mouse_pixel_delta: PhysicalPosition<f64>,
}

impl InputComponent {
    pub fn new() -> Self {
        Self { 
            pressed_keyboard_keys: [false; 163],
            released_keyboard_keys: [false; 163],
            held_keyboard_keys: [false; 163],

            pressed_mouse_buttons: [false; 3],
            released_mouse_buttons: [false; 3],
            held_mouse_buttons: [false; 3],
    
            current_mouse_position: PhysicalPosition { x: 0.0, y: 0.0},
            previous_mouse_position: PhysicalPosition { x: 0.0, y: 0.0},
    
            current_mouse_line_delta: (0.0, 0.0),
            previous_mouse_line_delta: (0.0, 0.0),
    
            current_mouse_pixel_delta: PhysicalPosition {x: 0.0, y: 0.0},
            previous_mouse_pixel_delta: PhysicalPosition {x: 0.0, y: 0.0},
        }
    }

    // - All Input Types Functionalities

    pub fn overwrite_previous_positions(&mut self) {
        self.previous_mouse_position = self.current_mouse_position;
        self.previous_mouse_pixel_delta = self.current_mouse_pixel_delta;
        self.previous_mouse_line_delta = self.current_mouse_line_delta;
    }

    pub fn overwrite_buttons(&mut self) {
        for i in 0..163 {
            if self.held_keyboard_keys[i] && self.pressed_keyboard_keys[i] {
                self.pressed_keyboard_keys[i] = false;
            }
            if !self.held_keyboard_keys[i] && self.released_keyboard_keys[i] {
                self.released_keyboard_keys[i] = false;
            }
        }
        for i in 0..3 {
            if self.held_mouse_buttons[i] && self.pressed_mouse_buttons[i] {
                self.pressed_mouse_buttons[i] = false;
            }
            if !self.held_mouse_buttons[i] && self.released_mouse_buttons[i] {
                self.released_mouse_buttons[i] = false;
            }
        }
        
    }

    // - Keyboard Key Functionalities

    pub fn set_key_pressed(&mut self, key: usize) {
        if self.held_keyboard_keys[key] {
            self.pressed_keyboard_keys[key] = false;
        }
        else {
            self.pressed_keyboard_keys[key] = true;
            self.held_keyboard_keys[key] = true;
        }
    }

    pub fn set_key_released(&mut self, key: usize) {
        self.released_keyboard_keys[key] = true;
        self.held_keyboard_keys[key] = false;
    }

    pub fn get_key_pressed(&self, key: VirtualKeyCode) -> bool {
        self.pressed_keyboard_keys[key as usize]
    }

    pub fn get_key_held(&self, key: VirtualKeyCode) -> bool {
        self.held_keyboard_keys[key as usize]
    }

    pub fn get_key_released(&self, key: VirtualKeyCode) -> bool {
        self.released_keyboard_keys[key as usize]
    }

    // - Mouse Buttons Functionalities

    pub fn set_left_mouse_button_pressed(&mut self) {
        if self.held_mouse_buttons[0] {
            self.pressed_mouse_buttons[0] = false;
        }
        else {
            self.pressed_mouse_buttons[0] = true;
            self.held_mouse_buttons[0] = true;
        }
    } 

    pub fn set_left_mouse_button_released(&mut self) {
        self.released_mouse_buttons[0] = true;
        self.held_mouse_buttons[0] = false;
    }

    pub fn set_middle_mouse_button_pressed(&mut self) {
        if self.held_mouse_buttons[1] {
            self.pressed_mouse_buttons[1] = false;
        }
        else {
            self.pressed_mouse_buttons[1] = true;
            self.held_mouse_buttons[1] = true;
        }
    }

    pub fn set_middle_mouse_button_released(&mut self) {
        self.released_mouse_buttons[1] = true;
        self.held_mouse_buttons[1] = false;
    }

    pub fn set_right_mouse_button_pressed(&mut self) {
        if self.held_mouse_buttons[2] {
            self.pressed_mouse_buttons[2] = false;
        }
        else {
            self.pressed_mouse_buttons[2] = true;
            self.held_mouse_buttons[2] = true;
        }
    }

    pub fn set_right_mouse_button_released(&mut self) {
        self.released_mouse_buttons[2] = true;
        self.held_mouse_buttons[2] = false;
    }

    pub fn get_mouse_button_pressed(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left => self.pressed_mouse_buttons[0],

            MouseButton::Middle =>  self.pressed_mouse_buttons[1],

            MouseButton::Right => self.pressed_mouse_buttons[2],

            _ => false
        }
    }

    pub fn get_mouse_button_held(&self, button: MouseButton) -> bool {
        match button {
            MouseButton::Left =>  self.held_mouse_buttons[0],

            MouseButton::Middle => self.held_mouse_buttons[1],

            MouseButton::Right =>  self.held_mouse_buttons[2],

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

    pub fn set_current_mouse_pixel_delta(&mut self, new_position: PhysicalPosition<f64>) {
        self.current_mouse_pixel_delta = new_position;
    }

    // - Mouse Motion Functionality

    pub fn get_current_mouse_position(&self) -> PhysicalPosition<f64> {
        self.current_mouse_position
    }

    pub fn set_current_mouse_position(&mut self, new_position: PhysicalPosition<f64>) {
        self.current_mouse_position = new_position;
    }
}

impl PillTypeMapKey for InputComponent {
    type Storage = GlobalComponentStorage<InputComponent>; 
}

impl Component for InputComponent { } 
