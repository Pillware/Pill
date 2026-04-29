use crate::{ecs::TimeComponent, engine::Engine, scripting::script_component::ScriptComponent};
use anyhow::Result;

pub fn script_system(engine: &mut Engine) -> Result<()> {
    // start the scripts, (later reload them if needed), update them (maybe read their inputs
    // synchroneously?
    let delta_time = engine.frame_delta_time;

    for (_entity_handle, script_component) in engine.iterate_one_component::<ScriptComponent>()? {
        if !script_component.started {
            //engine.managed_runtime.create_script(entity_handle.0 as u64, &script_component.script_type)?; // TODO: cast <- for now hardcode 0
            engine
                .managed_runtime
                .create_script(0, &script_component.script_type)?; // TODO: create
                                                                   // or just
                                                                   // start?
            engine.managed_runtime.start_script(0)?;
            //script_component.started = true; // TODO: cannot modify it because borrow
            //checker complains
        }

        engine.managed_runtime.update_script(0, delta_time)?;
    }

    // TODO: dumb workaround...
    for (_entity_handle, script_component) in
        engine.iterate_one_component_mut::<ScriptComponent>()?
    {
        if !script_component.started {
            script_component.started = true;
        }
    }

    Ok(())
}
