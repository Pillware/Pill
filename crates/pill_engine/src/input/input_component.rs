use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use winit::event::{VirtualKeyCode, ElementState, MouseButton, MouseScrollDelta};
use anyhow::{Result, Context, Error};

use crate::input::input_event;
use crate::ecs::Component;

use super::InputEvent;

pub struct InputComponent {
    // Key arrays
    pub current_keys: [bool; 163],
    pub previous_keys: [bool; 163]
}

impl InputComponent {
    pub fn press_key(&mut self, key: usize) {
        self.current_keys[key] = true;
    }

    pub fn release_key(&mut self, key: usize) {
        self.current_keys[key] = false;
    }

    pub fn overwrite_prev_keys(&mut self) {
        for i in 0..163 {
            self.previous_keys[i] = self.current_keys[i];
        }
    }

    pub fn is_key_pressed(&self, key: usize) -> &bool {
        &self.current_keys[key]
    }

    pub fn is_key_clicked(&self, key: usize) -> &bool {
        if &self.current_keys[key] == &true && &self.current_keys[key] == &false {
            return &true
        }
        else {
            return &false
        }
    }

    pub fn is_key_released(&self, key: usize) -> &bool {
        if &self.current_keys[key] == &false && &self.current_keys[key] == &true {
            return &true
        }
        else {
            return &false
        }
    }
}

impl Default for InputComponent {
    fn default() -> Self {
        Self { 
            current_keys: [false; 163],
            previous_keys: [false; 163]
        }
    }
}

pub struct GlobalComponent<T> {
    pub component: Option<T>
}

impl<T> GlobalComponent<T> {
    pub fn new() -> Self {
        Self {
            component: None
        }
    }

    pub fn set_component(&mut self, comp: T) -> Result<()>  {
        self.component = Some(comp);
        Ok(())
    }
}

unsafe impl<T> Sync for GlobalComponent<T> {}

impl Component for InputComponent {
    type Storage = GlobalComponent<Self>;
}