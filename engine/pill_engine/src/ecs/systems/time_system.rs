use crate::{ecs::TimeComponent, engine::Engine};

use pill_core::Result;

pub fn time_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.frame_delta_time;

    let component = engine.get_global_component_mut::<TimeComponent>()?;

    component.update(delta_time)?;

    Ok(())
}
