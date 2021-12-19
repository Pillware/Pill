use std::ops::Range;
use anyhow::{Result, Context, Error};
use boolinator::Boolinator;
use log::{debug, error, info, warn};
use pill_core::{EngineError, PillStyle};
use crate::{internal::{Engine, MeshRenderingComponent, TransformComponent}, graphics::{RenderQueueItem, RenderQueueKey}, resources::{Material, MaterialHandle, Mesh, MeshHandle}, ecs::{CameraComponent, EntityHandle}};
use thiserror::Error;



// [TODO] Use iterators once they are implemented
pub fn rendering_system(engine: &mut Engine) -> Result<()> {

    debug!("Rendering system starting"); 
  
    let active_scene = engine.scene_manager.get_active_scene()?;

    let camera_component_storage = active_scene.get_component_storage::<CameraComponent>()
        .context(format!("{}: Cannot get active {}", "RenderingSystem".sobj_style(), "Camera".gobj_style()))?;
    let transform_component_storage = active_scene.get_component_storage::<TransformComponent>()
        .context(format!("{}: Cannot get {}", "RenderingSystem".sobj_style(), "TransformComponents".sobj_style())).unwrap();
    let mesh_rendering_component_storage = active_scene.get_component_storage::<MeshRenderingComponent>()
        .context(format!("{}: Cannot get {}", "RenderingSystem".sobj_style(), "MeshRenderingComponents".sobj_style())).unwrap();

    // Active camera
    //let active_camera_entity_handle = active_scene.get_active_camera_entity_handle()?;
    //let active_camera = camera_component_storage.data.get(active_camera_entity_handle.index).unwrap().as_ref().ok_or(Error::msg("Camera component not found"))?;
    
    // Find first enabled camera and use it as active
    let mut active_camera_entity_handle_result: Option<EntityHandle> = None;
    let mut i = 0;
    for camera in active_scene.get_one_component_storage::<CameraComponent>() {
        match camera.borrow_mut().as_mut() {
            Some(camera_component) => {
                if camera_component.enabled {
                    active_camera_entity_handle_result = Some(EntityHandle::new(i, i as u32));
                    break;
                }
            },
            None => i += 1
        };
    }

    // let mut active_camera_entity_handle_result: Option<EntityHandle> = None;
    // for i in 0..camera_component_storage.data.len() { //[TODO] Proper iteration
    //     match camera_component_storage.data[i] {
    //         Some(camera_component) => {
    //             if camera_component.enabled {
    //                 active_camera_entity_handle_result = Some(EntityHandle::new(i, 1));
    //                 break;
    //             }
    //         },
    //         None => continue,
    //     };
    // }

    let active_camera_entity_handle = active_camera_entity_handle_result.ok_or(Error::new(EngineError::NoActiveCamera))?;

    // Clear and fill render queue
    let render_queue = &mut engine.render_queue;
    render_queue.clear();

    println!("Mesh rendering components to process: {}", mesh_rendering_component_storage.data.len());

    // for mesh in active_scene.get_one_component_storage::<MeshRenderingComponent>() {
    //     if mesh.borrow().as_ref().is_none() {
    //         debug!("No mesh component found");
    //         continue;
    //     }

    //     // [TODO] Check if render queue key is correct
    //     if mesh.borrow().as_ref().unwrap().render_queue_key == 0 {
    //         debug!("Invalid render queue key");
    //         continue;
    //     }

    //     if mesh.borrow().as_ref().unwrap().mesh_handle == None {
    //         debug!("Mesh is not assigned");
    //         continue;
    //     }

    //     if mesh.borrow().as_ref().unwrap().material_handle == None {
    //         debug!("Material is not assigned");
    //         continue;
    //     }

    //     let render_queue_item = RenderQueueItem {
    //         key: mesh.borrow().as_ref().unwrap().render_queue_key,
    //         entity_index: i as u32,
    //     };

    //     render_queue.push(render_queue_item);
    // }

    for i in 0..mesh_rendering_component_storage.data.len() { //[TODO] Proper iteration
       
        //debug!("Processing entity {}", i);
        //let mesh_rendering_component = mesh_rendering_component_storage.data[i].as_ref().unwrap();

        let borrowed_rendering_component = mesh_rendering_component_storage.data[i].borrow();
        let mesh_rendering_component = match borrowed_rendering_component.as_ref() {
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
        if mesh_rendering_component.render_queue_key == 0 {
            debug!("Invalid render queue key");
            continue;
        }

        if mesh_rendering_component.mesh_handle == None {
            debug!("Mesh is not assigned");
            continue;
        }

        if mesh_rendering_component.material_handle == None {
            debug!("Material is not assigned");
            continue;
        }

        let render_queue_item = RenderQueueItem {
            key: mesh_rendering_component.render_queue_key,
            entity_index: i as u32,
        };

        render_queue.push(render_queue_item);
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
        // Recreate the swap_chain if lost
        //Err(RendererError::SwapChainLost) => self.renderer.resize(self.renderer.state.window_size),
        // The system is out of memory, we should probably quit
        //Err(RendererError::SwapChainOutOfMemory) => *control_flow = ControlFlow::Exit,
        // All other errors (Outdated, Timeout) should be resolved by the next frame
        Err(renderer_error) => Err(Error::new(renderer_error)), //error!("{:?}", e)
    }
}