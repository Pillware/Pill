use crate::{
    ecs::{ Component, GlobalComponentStorage }, 
};

use pill_core::PillTypeMapKey;

use anyhow::{Result, Error, Context};


pub struct TimeComponent {
    pub delta_time: f32
}

impl TimeComponent {
    pub fn new() -> Self {
        Self { 
            delta_time: 0.0
        }
    }

    pub fn update_delta_time(&mut self, new_delta_time: f32) -> Result<()> {
        self.delta_time = new_delta_time;
        Ok(())
    }

    pub fn get_delta_time(&self) -> Result<f32> {
        Ok(self.delta_time)
    }
}

impl PillTypeMapKey for TimeComponent {
    type Storage = GlobalComponentStorage<TimeComponent>; 
}

impl Component for TimeComponent {
   
}