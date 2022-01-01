use anyhow::{Result, Context, Error};

use crate::game::Engine;

use super::TimeComponent;

pub fn time_system(engine: &mut Engine) -> Result<()> {
    let eng = &*engine;
    let delta_time = eng.frame_delta_time;

    let component = engine.get_global_component_mut::<TimeComponent>()?;
    component.update_delta_time(delta_time)?;
    Ok(())
}

