use crate::ecs::{ Component, ComponentStorage, GlobalComponentStorage };

use pill_core::PillTypeMapKey;

use cgmath::Vector3;

pub struct AudioListenerComponent {

    enabled: bool
    
}

impl AudioListenerComponent {

    pub fn new(left_ear_position: [f32; 3], right_ear_position: [f32; 3]) -> Self {
        Self {
            enabled: false
        }
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
            enabled: false
        }
    }
}

impl PillTypeMapKey for AudioListenerComponent {
    type Storage = ComponentStorage<AudioListenerComponent>; 
}

impl Component for AudioListenerComponent { }