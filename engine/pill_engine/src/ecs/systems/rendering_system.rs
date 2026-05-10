use crate::{
    config::RENDERING_SYSTEM,
    ecs::{CameraAspectRatio, CameraComponent, EntityHandle, RenderStateComponent},
    engine::Engine,
    graphics::{PassPBROpaque, PassTonemap, RendererTargetDesc},
};

use pill_core::{EngineError, RendererError, Timer};

use pill_core::Result;

pub fn rendering_system(engine: &mut Engine) -> Result<()> {
    let mut timer = Timer::new();
    timer.begin_context("rendering_system update");

    // First-frame bootstrap: install default pass chain
    let boot_done = engine
        .get_global_component::<RenderStateComponent>()?
        .boot_done;

    if !boot_done {
        // Bootstrap wires only GPU resources (render targets). Per-frame state (bg/IBL/fog) is read
        // from RenderStateComponent each frame inside the passes via WorldQuery::get_global.
        let (w, h) = engine.renderer.get_surface_size();
        let hdr = engine.renderer.create_render_target(RendererTargetDesc {
            name: "hdr_target".to_string(),
            format: wgpu::TextureFormat::Rgba16Float,
            width: w,
            height: h,
        })?;
        #[cfg_attr(not(feature = "ui"), allow(unused_mut))]
        let mut passes: Vec<Box<dyn crate::graphics::Pass>> = vec![
            Box::new(PassPBROpaque::new(Some(hdr))),
            Box::new(PassTonemap::new(hdr)),
        ];
        #[cfg(feature = "ui")]
        {
            use crate::{ecs::EguiComponent, graphics::PassEgui};
            let client = engine
                .get_global_component::<EguiComponent>()?
                .egui_client
                .clone();
            let window = engine.renderer.get_window();
            passes.push(Box::new(PassEgui::new(window, client)));
        }
        engine.renderer.set_passes(passes)?;
        engine
            .get_global_component_mut::<RenderStateComponent>()?
            .boot_done = true;
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

    // Stateless: the engine hands the active scene + globals to the renderer; each pass queries the
    // per-entity components (WorldQuery::query::<T>()) and globals (WorldQuery::get_global::<T>())
    // it needs, and builds its own draw list.
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
