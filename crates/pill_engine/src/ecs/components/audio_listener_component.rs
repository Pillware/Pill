use crate::ecs::{ Component, ComponentStorage, GlobalComponentStorage };

use pill_core::PillTypeMapKey;

use cgmath::Vector3;

pub struct AudioListenerComponent {

    left_ear_position: [f32; 3],
    right_ear_position: [f32; 3],
    enabled: bool
}

impl AudioListenerComponent {

    pub fn new(left_ear_position: [f32; 3], right_ear_position: [f32; 3]) -> Self {
        Self {
            left_ear_position,
            right_ear_position,
            enabled: false
        }
    }

    pub fn set_left_ear_position(&mut self, new_position: Vector3<f32>) {
        self.left_ear_position[0] = new_position[0];
        self.left_ear_position[1] = new_position[1];
        self.left_ear_position[2] = new_position[2];
    }

    pub fn get_left_ear_position(&self) -> [f32; 3] {
        self.left_ear_position.clone()
    }

    pub fn set_right_ear_position(&mut self, new_position: Vector3<f32>) {
        self.right_ear_position[0] = new_position[0];
        self.right_ear_position[1] = new_position[1];
        self.right_ear_position[2] = new_position[2];
    }

    pub fn get_right_ear_position(&self) -> [f32; 3] {
        self.right_ear_position
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn get_enabled(&self) -> bool {
        self.enabled.clone()
    }
}

impl Default for AudioListenerComponent {

    fn default() -> Self {
        Self { 
            left_ear_position: [-1.0, 0.0, 0.0],
            right_ear_position: [1.0, 0.0, 0.0],
            enabled: false
        }
    }
}

impl PillTypeMapKey for AudioListenerComponent {
    type Storage = ComponentStorage<AudioListenerComponent>; 
}

impl Component for AudioListenerComponent { }