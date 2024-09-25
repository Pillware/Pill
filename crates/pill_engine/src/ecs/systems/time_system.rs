use crate::{
    engine::Engine,
    ecs::TimeComponent,
};

use anyhow::{Result, Context, Error};

pub fn time_system(engine: &mut Engine) -> Result<()> {
    let delta_time = (&*engine).frame_delta_time.clone();

    let component = engine.get_global_component_mut::<TimeComponent>()?;

    component.update(delta_time)?;

    Ok(())
}