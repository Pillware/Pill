use crate::ecs::Component;
use crate::input::input_component::GlobalComponent;
use anyhow::{Result, Error, Context};
pub struct TimeComponent {
    pub delta_time: f32
}

impl TimeComponent {

    pub fn update_delta_time(&mut self, new_delta_time: f32) -> Result<()> {
        self.delta_time = new_delta_time;
        Ok(())
    }

    pub fn get_delta_time(&self) -> Result<f32> {
        Ok(self.delta_time)
    }
}

impl Default for TimeComponent {
    
    fn default() -> Self {
        Self { 
            delta_time: 0.0
         }
    }
}


impl Component for TimeComponent {
    type Storage = GlobalComponent<Self>;
}