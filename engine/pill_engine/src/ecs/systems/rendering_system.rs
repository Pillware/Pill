use crate::{
    config::{default_pass_chain, RENDERING_SYSTEM},
    ecs::{CameraAspectRatio, CameraComponent, EntityHandle, PassChain, RenderStateComponent},
    engine::Engine,
};

use pill_core::{EngineError, RendererError, Timer};

use pill_core::Result;

pub fn rendering_system(engine: &mut Engine) -> Result<()> {
    let mut timer = Timer::new();
    timer.begin_context("rendering_system update");

    // First-frame bootstrap: install the pass chain the game selected
    let (boot_done, pass_chain) = {
        let render_state = engine.get_global_component::<RenderStateComponent>()?;
        (render_state.boot_done, render_state.pass_chain)
    };

    if !boot_done {
        // Marked first so a failing boot is not retried (each try allocates new targets)
        engine
            .get_global_component_mut::<RenderStateComponent>()?
            .boot_done = true;
        let passes = match pass_chain {
            PassChain::Default => default_pass_chain(engine)?,
            #[cfg(feature = "post_fx")]
            PassChain::PostFx => crate::config::post_fx_pass_chain(engine)?,
        };
        engine.renderer.set_passes(passes)?;
        return Ok(());
    }

    timer.record("Get active camera");

    let mut active_camera_entity_handle_result: Option<EntityHandle> = None;

    {
        let active_scene = engine.scene_manager.get_active_scene_mut()?;

        for (entity_handle, camera_component) in
            active_scene.get_one_component_iterator_mut::<CameraComponent>()?
        {
            if camera_component.enabled {
                if let CameraAspectRatio::Automatic(_) = camera_component.aspect {
                    let aspect_ratio =
                        engine.window_size.width as f32 / engine.window_size.height as f32;
                    camera_component.aspect = CameraAspectRatio::Automatic(aspect_ratio);
                }
                active_camera_entity_handle_result = Some(entity_handle);
                break;
            }
        }
    }

    let active_camera_entity_handle =
        active_camera_entity_handle_result.ok_or_else(|| -> pill_core::PillError {
            pill_core::PillError::from(EngineError::NoActiveCamera)
        })?;

    timer.begin_context("Render");

    // Hands the active scene and globals to the renderer; each pass queries what it needs.
    let active_scene = engine.scene_manager.get_active_scene()?;
    let globals = &engine.global_components;
    let delta_time = engine.frame_delta_time;

    let render_result = engine.renderer.render(
        active_camera_entity_handle,
        active_scene,
        globals,
        delta_time,
        &mut timer,
        &engine.resource_manager,
    );
    match render_result {
        Ok(_) => {
            timer.end_context()?; // end "Render"
            timer.end_context()?; // end "rendering_system update"
            engine.system_manager.update_system_timer(
                RENDERING_SYSTEM.name,
                RENDERING_SYSTEM.update_phase,
                timer,
            )?;
            Ok(())
        }
        Err(error) => match error.downcast_ref::<RendererError>() {
            Some(RendererError::SurfaceLost) => {
                timer.end_context()?; // end "Render"
                timer.end_context()?; // end "rendering_system update"
                engine.system_manager.update_system_timer(
                    RENDERING_SYSTEM.name,
                    RENDERING_SYSTEM.update_phase,
                    timer,
                )?;
                engine.renderer.resize(engine.window_size);
                Ok(())
            }
            Some(RendererError::SurfaceOutOfMemory) => {
                panic!("Critical: Renderer error, system out of memory");
            }
            _ => Err(error),
        },
    }
}
