use std::ops::Range;
use anyhow::{Result, Context, Error};
use log::{debug, error};
use crate::{internal::{Engine, MeshRenderingComponent, TransformComponent}, graphics::{RenderQueueItem, RenderQueueKey}, resources::{Material, MaterialHandle, Mesh, MeshHandle}};

// [TODO] Use iterators once they are implemented
pub fn rendering_system(engine: &mut Engine) {

    debug!("Rendering system starting"); 
  
    let active_scene_handle = engine.get_active_scene().unwrap();
    let active_scene = engine.scene_manager.get_scene(active_scene_handle).unwrap();
    let transform_component_storage = active_scene.get_component_storage::<TransformComponent>().unwrap();
    let mesh_rendering_component_storage = active_scene.get_component_storage::<MeshRenderingComponent>().unwrap();

    // Clear and fill render queue
    let render_queue = &mut engine.render_queue;
    render_queue.clear();



    println!("Mesh rendering components to process: {}", mesh_rendering_component_storage.data.len());
    for i in 0..mesh_rendering_component_storage.data.len() { //[TODO] Proper iteration
        debug!("Processing entity {}", i);

        // [TODO] Check if render queue key is correct
        if mesh_rendering_component_storage.data[i].render_queue_key == 0 {
            debug!("Invalid render queue key");
            continue;
        }

        if mesh_rendering_component_storage.data[i].mesh_handle == None {
            debug!("Mesh is not assigned");
            continue;
        }

        if mesh_rendering_component_storage.data[i].material_handle == None {
            debug!("Material is not assigned");
            continue;
        }

        let render_queue_item = RenderQueueItem {
            key: mesh_rendering_component_storage.data[i].render_queue_key,
            entity_index: i as u32,
        };

        render_queue.push(render_queue_item);
    }

    // Sort render queue
    render_queue.sort();

    // Render
    match engine.renderer.render(render_queue, transform_component_storage) {
        Ok(_) => {}
        // Recreate the swap_chain if lost
        //Err(RendererError::SwapChainLost) => self.renderer.resize(self.renderer.state.window_size),
        // The system is out of memory, we should probably quit
        //Err(RendererError::SwapChainOutOfMemory) => *control_flow = ControlFlow::Exit,
        // All other errors (Outdated, Timeout) should be resolved by the next frame
        Err(e) => error!("{:?}", e),
    }
}