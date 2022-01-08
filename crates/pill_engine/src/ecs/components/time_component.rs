use crate::{
    ecs::{ Component, GlobalComponentStorage, GlobalComponent }, 
};

use pill_core::PillTypeMapKey;

use anyhow::{Result, Error, Context};

#[readonly::make]
pub struct TimeComponent {
    #[readonly]
    pub time: f32, // Time elapsed from the start of the engine/game in milliseconds
    #[readonly]
    pub delta_time: f32, // Time of last frame in milliseconds
}

impl TimeComponent {
    pub fn new() -> Self {
        Self { 
            time: 0.0,
            delta_time: 0.0,
        }
    }

    pub(crate) fn update(&mut self, delta_time: f32) -> Result<()> {
        self.time += delta_time;
        self.delta_time = delta_time;
        
        Ok(())
    }
}

impl PillTypeMapKey for TimeComponent {
    type Storage = GlobalComponentStorage<TimeComponent>; 
}

impl GlobalComponent for TimeComponent {
   
}