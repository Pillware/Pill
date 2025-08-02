#![cfg(feature = "rendering")]

use crate::{
    config::RENDERING_SYSTEM,
    ecs::{ scene, update_transform_matrices, CameraAspectRatio, CameraComponent, Component, ComponentStorage, EguiManagerComponent, EntityHandle, MeshRenderingComponent, TransformComponent, UpdatePhase },
    engine::Engine,
    graphics::{ compose_render_queue_key, RenderQueueItem, RenderQueueKey },
    resources::{ Material, MaterialHandle, Mesh, MeshHandle, ResourceManager }
};

use pill_core::{ EngineError, PillSlotMapKey, PillStyle, RendererError, Timer };

use std::{ ops::Range };
use anyhow::{ Result, Context, Error };
use boolinator::Boolinator;
use log::{ debug };

pub fn rendering_system(engine: &mut Engine) -> Result<()> {
    let mut timer = Timer::new();
    timer.record_new_context("RenderingSystem update")?;
    timer.record("Get active camera")?;

    let active_scene_handle = engine.scene_manager.get_active_scene_handle()?;
    let mut active_camera_entity_handle_result: Option<EntityHandle> = None;

    {
        let active_scene = engine.scene_manager.get_active_scene_mut()?;

        // - Find active camera and update its aspect ratio if needed

        // Find first enabled camera and use it as active
        for (entity_handle, camera_component) in active_scene.get_one_component_iterator_mut::<CameraComponent>()? {
            if camera_component.enabled {
                // Update active camera aspect ratio if it is set to automatic
                if let CameraAspectRatio::Automatic(_) = camera_component.aspect {
                    let aspect_ratio = engine.window_size.width as f32 / engine.window_size.height as f32;
                    camera_component.aspect = CameraAspectRatio::Automatic(aspect_ratio);
                }
                active_camera_entity_handle_result = Some(entity_handle);
                break;
            }
        }
    }


    let active_camera_entity_handle = active_camera_entity_handle_result.ok_or(Error::new(EngineError::NoActiveCamera))?.clone();

    // - Prepare rendering data
    timer.record("Prepare render queue")?;



    timer.record("Get component storages")?;

    let egui_ui = EguiManagerComponent::get_ui(engine);// egui_manager_component.get_ui(engine);



    timer.record_new_context("Render")?;

    let egui_ui_with_engine = Box::new(|engine: &mut Engine, ctx: &egui::Context| {
        let ui_fn = EguiManagerComponent::get_ui(engine);
        ui_fn(engine, ctx);
    });

    // Render
    let mut renderer = engine.renderer.take().expect("Critical: Renderer is None");
    match renderer.render(
        engine,
        egui_ui_with_engine,
        &mut timer
    ) {
        Ok(_) => {
            timer.end_context()?; // End "Render" context
            engine.system_manager.update_system_timer(RENDERING_SYSTEM.name, RENDERING_SYSTEM.update_phase, timer)?;
            engine.renderer = Some(renderer); // Put renderer back to engine
            Ok(())
        }
        Err(e) => {
            match e.downcast_ref::<RendererError>() {
                Some(RendererError::SurfaceLost) => {
                    // Recreate lost surface
                    timer.end_context()?; // End "Render" context
                    engine.system_manager.update_system_timer(RENDERING_SYSTEM.name, RENDERING_SYSTEM.update_phase, timer)?;
                    Ok(renderer.resize(engine.window_size))
                },
                Some(RendererError::SurfaceOutOfMemory) => {
                    panic!("Critical: Renderer error, system out of memory");
                },
                _ => Err(e),
            }
        }
    }

}
