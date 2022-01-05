use crate::{
    engine::Engine,
    graphics::{ RenderQueueKey, compose_render_queue_key, RenderQueueItem, RendererError }, 
    resources::{ Material, MaterialHandle, Mesh, MeshHandle, ResourceManager },
    ecs::{ ComponentStorage, Component, CameraComponent, EntityHandle, TransformComponent, MeshRenderingComponent, CameraAspectRatio, scene }
};

use pill_core::{ EngineError, PillStyle, PillSlotMapKey };

use std::{ops::Range, borrow::BorrowMut, num::NonZeroU32};
use anyhow::{ Result, Context, Error };
use boolinator::Boolinator;
use log::{ debug };

pub fn rendering_system(engine: &mut Engine) -> Result<()> {
    let active_scene_handle = engine.scene_manager.get_active_scene_handle()?;

    // - Find active camera and update its aspect ratio if needed

    // Find first enabled camera and use it as active
    let mut active_camera_entity_handle_result: Option<EntityHandle> = None;
    for (entity_handle, camera_component) in (&*engine).iterate_one_component_with_entities::<CameraComponent>().unwrap() {
        let mut camera_component_mut_slot = camera_component.borrow_mut();
        let camera_component = camera_component_mut_slot.as_mut().unwrap();
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
    let active_camera_entity_handle = active_camera_entity_handle_result.ok_or(Error::new(EngineError::NoActiveCamera))?.clone();

    
    // - Prepare rendering data

    // Clear and fill render queue
    let render_queue = &mut engine.render_queue;
    render_queue.clear();

    // Iterate mesh rendering components
    for (entity_handle, mesh_rendering_component) in (engine).scene_manager.fetch_one_component_storage_with_entity_handles::<MeshRenderingComponent>(active_scene_handle).unwrap() {

        // Add valid mesh rendering components to render queue
        if let Some(render_queue_key) = mesh_rendering_component.borrow().as_ref().unwrap().render_queue_key {
            let render_queue_item = RenderQueueItem {
                key: render_queue_key,
                entity_index: entity_handle.data().index as u32, 
            };
    
            render_queue.push(render_queue_item);
        }
        else {
            debug!("Invalid render queue key");
            continue;
        } 
    }

    // Sort render queue
    render_queue.sort();

    // Get scene handle
    let active_scene = engine.scene_manager.get_active_scene_mut()?;

    // Get storages
    let camera_component_storage = active_scene.get_component_storage::<CameraComponent>()
        .context(format!("{}: Cannot get active {}", "RenderingSystem".sobj_style(), "Camera".gobj_style()))?;
    let transform_component_storage = active_scene.get_component_storage::<TransformComponent>()
        .context(format!("{}: Cannot get {}", "RenderingSystem".sobj_style(), "TransformComponents".sobj_style())).unwrap();

    // Render
    match engine.renderer.render(
        active_camera_entity_handle, 
        render_queue, 
        camera_component_storage,
        transform_component_storage
    ) {
        Ok(_) => Ok(()),
        // Recreate the surface if it is lost
        Err(RendererError::SurfaceLost) => Ok(engine.renderer.resize(engine.window_size)),
        // The system is out of memory, we should probably quit
        //Err(RendererError::SurfaceOutOfMemory) => *control_flow = ControlFlow::Exit, // [TODO]
        // All other errors (Outdated, Timeout) should be resolved by the next frame
        Err(renderer_error) => Err(Error::new(renderer_error)),
    }
}