use crate::{
    config::RENDERING_SYSTEM, 
    ecs::{ scene, update_transform_matrices, CameraAspectRatio, CameraComponent, Component, ComponentStorage, EguiManagerComponent, EntityHandle, MeshRenderingComponent, TransformComponent, UpdatePhase }, 
    engine::Engine, 
    graphics::{ compose_render_queue_key, RenderQueueItem, RenderQueueKey }, 
    resources::{ Material, MaterialHandle, Mesh, MeshHandle, ResourceManager }
};

use pill_core::{ warn, EngineError, LogContext, PillSlotMapKey, PillStyle, RendererError, Timer };

use std::{ ops::Range, time::Instant };
use anyhow::{ Result, Context, Error };
use boolinator::Boolinator;

pub fn rendering_system(engine: &mut Engine) -> Result<()> {
    let mut timer = Timer::new();
    timer.begin_context("rendering_system update");
    timer.record("Get active camera");

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
    timer.record("Clear render queue");

    // Clear the render queue
    engine.render_queue.clear();
    engine.render_queue.reserve(200000); // Reserve space for 1000 items

    timer.record("Prepare render queue");

    let mut _matrix_calculation_duration: f32 = 0.0;
    let mut add_to_render_queue_duration: f32 = 0.0;

    // Iterate mesh rendering components
    for (entity_handle, transform_component, mesh_rendering_component) in
        engine.scene_manager.get_two_component_iterator_mut::<TransformComponent, MeshRenderingComponent>(active_scene_handle)?
    {
        // Update transform matrices if required

        // Add valid mesh rendering components to render queue
        let add_to_render_queue_start_time = Instant::now();
        if let Some(render_queue_key) = mesh_rendering_component.render_queue_key {
            let render_queue_item = RenderQueueItem {
                key: render_queue_key,
                entity_index: entity_handle.data().index as u32,
            };
            engine.render_queue.push(render_queue_item);
        } else {
            warn!(LogContext::Rendering => "Invalid render queue key");
            continue;
        }
        add_to_render_queue_duration += add_to_render_queue_start_time.elapsed().as_secs_f32() * 1000.0;
    }

    timer.record(&format!("Matrix calculation {} ms", _matrix_calculation_duration));
    timer.record(&format!("Add to render queue {} ms", add_to_render_queue_duration));

    timer.record("Sort render queue");

    // Sort render queue
    engine.render_queue.sort();

    timer.record("Get component storages");

    let egui_ui = EguiManagerComponent::get_ui(engine);// egui_manager_component.get_ui(engine);

    let active_scene = engine.scene_manager.get_active_scene_mut()?;
    // Get storages
    let camera_component_storage = active_scene.get_component_storage::<CameraComponent>()
        .context(format!("{}: Cannot get active {}", "rendering_system".specific_object_style(), "Camera".general_object_style()))?;
    let transform_component_storage = active_scene.get_component_storage::<TransformComponent>()
        .context(format!("{}: Cannot get {}", "rendering_system".specific_object_style(), "TransformComponents".specific_object_style())).unwrap();

    timer.begin_context("Render");

    // Render
    match engine.renderer.render(
        active_camera_entity_handle, 
        &engine.render_queue, 
        camera_component_storage,
        transform_component_storage,
        egui_ui,
        &mut timer
    ) {
        Ok(_) => {
            timer.end_context()?; // End "Render" context
            engine.system_manager.update_system_timer(RENDERING_SYSTEM.name, RENDERING_SYSTEM.update_phase, timer)?;
            Ok(())
        } 
        Err(e) => {
            match e.downcast_ref::<RendererError>() {
                Some(RendererError::SurfaceLost) => {
                    // Recreate lost surface
                    timer.end_context()?; // End "Render" context
                    engine.system_manager.update_system_timer(RENDERING_SYSTEM.name, RENDERING_SYSTEM.update_phase, timer)?;
                    Ok(engine.renderer.resize(engine.window_size))
                },
                Some(RendererError::SurfaceOutOfMemory) => {
                    panic!("Critical: Renderer error, system out of memory");
                },
                _ => Err(e),
            }
        }
    }
}