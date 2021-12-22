use std::ops::Range;
use anyhow::{Result, Context, Error};
use boolinator::Boolinator;
use log::{debug, error, info, warn};
use pill_core::{EngineError, PillStyle};
use crate::{internal::{Engine, MeshRenderingComponent, TransformComponent}, graphics::{RenderQueueItem, RenderQueueKey, RendererError}, resources::{Material, MaterialHandle, Mesh, MeshHandle}, ecs::{CameraComponent, EntityHandle}};
use thiserror::Error;

use super::CameraAspectRatio;

// [TODO] Use iterators once they are implemented
pub fn rendering_system(engine: &mut Engine) -> Result<()> {
    //debug!("Rendering system starting"); 
    let active_scene = engine.scene_manager.get_active_scene_mut()?;

    // - Find active camera and update its aspect ratio if needed

    let camera_component_storage = active_scene.get_component_storage_mut::<CameraComponent>()
        .context(format!("{}: Cannot get active {}", "RenderingSystem".sobj_style(), "Camera".gobj_style()))?;
  
    // Find first enabled camera and use it as active
    let mut active_camera_entity_handle_result: Option<EntityHandle> = None;
    for i in 0..camera_component_storage.data.len() { //[TODO] Proper iteration
        match camera_component_storage.data[i].as_mut() {
            Some(camera_component) => {
                if camera_component.enabled {
                    // Update active camera aspect ratio if it is set to automatic
                     if let CameraAspectRatio::Automatic(_) = camera_component.aspect {
                        let aspect_ratio = engine.window_size.width as f32 / engine.window_size.height as f32;
                        camera_component.aspect = CameraAspectRatio::Automatic(aspect_ratio);
                    }
                    active_camera_entity_handle_result = Some(EntityHandle::new(i));
                    break;
                }
            },
            None => continue,
        };
    }

    let active_camera_entity_handle = active_camera_entity_handle_result.ok_or(Error::new(EngineError::NoActiveCamera))?.clone();

    // - Prepare rendering data

    let camera_component_storage = active_scene.get_component_storage::<CameraComponent>()
        .context(format!("{}: Cannot get active {}", "RenderingSystem".sobj_style(), "Camera".gobj_style()))?;
    let transform_component_storage = active_scene.get_component_storage::<TransformComponent>()
        .context(format!("{}: Cannot get {}", "RenderingSystem".sobj_style(), "TransformComponents".sobj_style())).unwrap();
    let mesh_rendering_component_storage = active_scene.get_component_storage::<MeshRenderingComponent>()
        .context(format!("{}: Cannot get {}", "RenderingSystem".sobj_style(), "MeshRenderingComponents".sobj_style())).unwrap();

    // Clear and fill render queue
    let render_queue = &mut engine.render_queue;
    render_queue.clear();

    //println!("Mesh rendering components to process: {}", mesh_rendering_component_storage.data.len());
    for i in 0..mesh_rendering_component_storage.data.len() { //[TODO] Proper iteration
       
        //debug!("Processing entity {}", i);
        //let mesh_rendering_component = mesh_rendering_component_storage.data[i].as_ref().unwrap();

        let mesh_rendering_component = match mesh_rendering_component_storage.data[i].as_ref() {
            Some(mesh_rendering_component) => { 
                //debug!("Processing entity {} - found", i); 
                mesh_rendering_component 
            },
            None => { 
                //debug!("Processing entity {} - empty", i); 
                continue 
            },
        };

        // [TODO] Check if render queue key is correct
        if let Some(render_queue_key) = mesh_rendering_component.render_queue_key {
            let render_queue_item = RenderQueueItem {
                key: render_queue_key,
                entity_index: i as u32, 
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