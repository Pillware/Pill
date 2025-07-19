use crate::{
    ecs::{ scene, CameraAspectRatio, CameraComponent, Component, ComponentStorage, EguiManagerComponent, EntityHandle, MeshRenderingComponent, TransformComponent }, 
    engine::Engine, graphics::{ compose_render_queue_key, RenderQueueItem, RenderQueueKey, RendererError }, 
    resources::{ Material, MaterialHandle, Mesh, MeshHandle, ResourceManager }
};

use pill_core::{ EngineError, PillStyle, PillSlotMapKey };

use std::{ ops::Range };
use anyhow::{ Result, Context, Error };
use boolinator::Boolinator;
use log::{ debug };

pub fn rendering_system(engine: &mut Engine) -> Result<()> {
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

    // Clear the render queue
    engine.render_queue.clear();
    // Iterate mesh rendering components
    for (entity_handle, transform_component, mesh_rendering_component) in
        engine.scene_manager.get_two_component_iterator::<TransformComponent, MeshRenderingComponent>(active_scene_handle)?
    {
        // Add valid mesh rendering components to render queue
        if let Some(render_queue_key) = mesh_rendering_component.render_queue_key {
            let render_queue_item = RenderQueueItem {
                key: render_queue_key,
                entity_index: entity_handle.data().index as u32,
            };
            engine.render_queue.push(render_queue_item);
        } else {
            debug!("Invalid render queue key");
            continue;
        }
    }

    // Sort render queue
    engine.render_queue.sort();

    let egui_ui = EguiManagerComponent::get_ui(engine);// egui_manager_component.get_ui(engine);

    let active_scene = engine.scene_manager.get_active_scene_mut()?;
    // Get storages
    let camera_component_storage = active_scene.get_component_storage::<CameraComponent>()
        .context(format!("{}: Cannot get active {}", "RenderingSystem".sobj_style(), "Camera".gobj_style()))?;
    let transform_component_storage = active_scene.get_component_storage::<TransformComponent>()
        .context(format!("{}: Cannot get {}", "RenderingSystem".sobj_style(), "TransformComponents".sobj_style())).unwrap();


    // Render
    match engine.renderer.render(
        active_camera_entity_handle, 
        &engine.render_queue, 
        camera_component_storage,
        transform_component_storage,
        egui_ui
    ) {
        Ok(_) => Ok(()),
        // Recreate lost surface
        Err(RendererError::SurfaceLost) => Ok(engine.renderer.resize(engine.window_size)),
        // System is out of memory
        Err(RendererError::SurfaceOutOfMemory) => { panic!("Critical: Renderer error, system out of memory")}
        // All other errors (Outdated, Timeout)
        Err(renderer_error) => Err(Error::new(renderer_error)),
    }
}