use crate::{
    ecs::{ Component, GlobalComponentStorage, GlobalComponent }, 
};

use pill_core::PillTypeMapKey;

use anyhow::{Result, Error, Context};

#[readonly::make]
pub struct TimeComponent {
    #[readonly]
    pub delta_time: f32
}

impl TimeComponent {
    pub fn new() -> Self {
        Self { 
            delta_time: 0.0
        }
    }

    pub(crate) fn update_delta_time(&mut self, new_delta_time: f32) -> Result<()> {
        self.delta_time = new_delta_time;
        Ok(())
    }
}

impl PillTypeMapKey for TimeComponent {
    type Storage = GlobalComponentStorage<TimeComponent>; 
}

impl GlobalComponent for TimeComponent {
   
}