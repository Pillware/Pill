use std::any::Any;
use std::collections::HashMap;
use winit::event::{VirtualKeyCode, ElementState, MouseButton, MouseScrollDelta};

use crate::input::input_event;
use crate::ecs::Component;

use super::InputEvent;

pub struct InputComponent {
    pub current_keys: HashMap<VirtualKeyCode, bool>,
    pub previous_keys: HashMap<VirtualKeyCode, bool>,
}

impl InputComponent {
    pub fn press_key(&mut self, key: VirtualKeyCode) {
        if self.current_keys.contains_key(&key) == false {
            self.current_keys.insert(key, true);
            self.previous_keys.insert(key, false);
        }
        else {
            if let Some(x) = self.current_keys.get_mut(&key) {
                *x = true;
            }
        }
    }

    pub fn release_key(&mut self, key: VirtualKeyCode) {
        if self.current_keys.contains_key(&key) == false {
            self.current_keys.insert(key, false);
            self.previous_keys.insert(key, false);
        }
        else {
            if let Some(x) = self.current_keys.get_mut(&key) {
                *x = false;
            }
        }
    }

    pub fn is_pressed(&self, key: VirtualKeyCode) -> &bool {
        if self.current_keys.contains_key(&key) == false {
            &false
        }
        else {
            self.current_keys.get(&key).unwrap_or(&false)
        }
    }

    pub fn overwrite_prev_keys(&mut self) {
        for (key, value) in &self.current_keys {
            if let Some(x) = self.previous_keys.get_mut(&key) {
                *x = *value;
            }
        }
    }
}

impl Default for InputComponent {
    fn default() -> Self {
        Self { current_keys: Default::default(), previous_keys: Default::default()}
    }
}

impl Component for InputComponent {
    type Storage = InputComponent;
}