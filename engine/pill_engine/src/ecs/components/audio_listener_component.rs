use crate::ecs::{ Component, ComponentStorage };

use pill_core::PillTypeMapKey;

// --- Builder ---

pub struct AudioListenerComponentBuilder {
    component: AudioListenerComponent,
}

impl AudioListenerComponentBuilder {
    pub fn default() -> Self {
        Self {
            component: AudioListenerComponent::new(),
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.component.enabled = enabled;
        self
    }

    pub fn build(self) -> AudioListenerComponent {
        self.component
    }
}

// --- Audio Listener Component ---
pub struct AudioListenerComponent {
    pub enabled: bool
}

impl Default for AudioListenerComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioListenerComponent {
    pub fn builder() -> AudioListenerComponentBuilder {
        AudioListenerComponentBuilder::default()
    }

    pub fn new() -> Self {
        Self {
            enabled: false
        }
    }
}

impl PillTypeMapKey for AudioListenerComponent {
    type Storage = ComponentStorage<AudioListenerComponent>;
}

impl Component for AudioListenerComponent { }
