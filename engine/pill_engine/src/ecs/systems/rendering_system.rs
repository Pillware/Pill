use crate::{
    config::RENDERING_SYSTEM,
    ecs::{
        scene, update_transform_matrices, CameraAspectRatio, CameraComponent, Component,
        ComponentStorage, EguiManagerComponent, EntityHandle, MeshRenderingComponent,
        TransformComponent, UpdatePhase,
    },
    engine::Engine,
    graphics::{compose_render_queue_key, RenderQueueItem, RenderQueueKey},
    resources::{Material, MaterialHandle, Mesh, MeshHandle, ResourceManager},
};

use pill_core::{EngineError, PillSlotMapKey, PillStyle, RendererError, Timer};

use anyhow::{Context, Error, Result};
use boolinator::Boolinator;
use log::debug;
use std::{ops::Range, time::Instant};

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
        for (entity_handle, camera_component) in
            active_scene.get_one_component_iterator_mut::<CameraComponent>()?
        {
            if camera_component.enabled {
                // Update active camera aspect ratio if it is set to automatic
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

    let active_camera_entity_handle = active_camera_entity_handle_result
        .ok_or(Error::new(EngineError::NoActiveCamera))?
        .clone();

    // - Prepare rendering data
    timer.record("Clear render queue");

    // Clear the render queue
    // [SIMILAR] Build and sort a render queue ahead of draw; separates data prep from draw per TALK
    engine.render_queue.clear();
    engine.render_queue.reserve(200000); // Reserve space for 1000 items

    timer.record("Prepare render queue");

    let mut dirty_entities: Vec<EntityHandle> = Vec::new();

    // Phase 1: Sweep components; route transforms needing matrix update to a batch, push clean directly
    // [SIMILAR] Batch transform updates; avoid per-draw matrix work
    for (entity_handle, transform_component, mesh_rendering_component) in engine
        .scene_manager
        .get_two_component_iterator_mut::<TransformComponent, MeshRenderingComponent>(
        active_scene_handle,
    )? {
        if transform_component.matrix_update_required {
            // defer update to batch
            dirty_entities.push(entity_handle);
            continue;
        }

        // Push clean (non-dirty) items directly into the render queue
        // [SIMILAR] Use precomputed render_queue_key (pipeline/material/mesh sorting key)
        if let Some(render_queue_key) = mesh_rendering_component.render_queue_key {
            let render_queue_item = RenderQueueItem {
                key: render_queue_key,
                entity_index: entity_handle.data().index as u32,
            };
            engine.render_queue.push(render_queue_item);
        }
    }

    // Phase 2: Batch update transforms with matrix_update_required
    // [RECOMMENDED] Consolidate transform matrix updates in one batch outside of pass
    if !dirty_entities.is_empty() {
        timer.begin_context(&format!("Batch update {} transforms", dirty_entities.len()));
        for entity_handle in &dirty_entities {
            // Pull the transform component mutably and update matrices
            if let Ok(transform_component) = engine
                .scene_manager
                .get_entity_component::<TransformComponent>(*entity_handle, active_scene_handle)
            {
                if transform_component.matrix_update_required {
                    update_transform_matrices(transform_component);
                    transform_component.matrix_update_required = false;
                }
            }
        }
        timer.end_context()?;
    }

    // Phase 3: Add updated (previously dirty) items to the render queue
    for entity_handle in dirty_entities {
        if let Ok(mesh_rendering_component) = engine
            .scene_manager
            .get_entity_component::<MeshRenderingComponent>(entity_handle, active_scene_handle)
        {
            if let Some(render_queue_key) = mesh_rendering_component.render_queue_key {
                let render_queue_item = RenderQueueItem {
                    key: render_queue_key,
                    entity_index: entity_handle.data().index as u32,
                };
                engine.render_queue.push(render_queue_item);
            }
        }
    }

    timer.record("Get component storages");

    let egui_ui = EguiManagerComponent::get_ui(engine); // egui_manager_component.get_ui(engine);

    let active_scene = engine.scene_manager.get_active_scene_mut()?;
    // Get storages
    let camera_component_storage = active_scene
        .get_component_storage::<CameraComponent>()
        .context(format!(
            "{}: Cannot get active {}",
            "rendering_system".sobj_style(),
            "Camera".gobj_style()
        ))?;
    let transform_component_storage = active_scene
        .get_component_storage::<TransformComponent>()
        .context(format!(
            "{}: Cannot get {}",
            "rendering_system".sobj_style(),
            "TransformComponents".sobj_style()
        ))
        .unwrap();

    timer.begin_context("Render");

    // Render
    // [API->CLIENT] Low-level renderer expects ordered render_queue and stable storages; scene graph culling/binning lives in client/high-level
    match engine.renderer.render(
        active_camera_entity_handle,
        &engine.render_queue,
        camera_component_storage,
        transform_component_storage,
        egui_ui,
        &mut timer,
    ) {
        Ok(_) => {
            timer.end_context()?; // End "Render" context
            engine.system_manager.update_system_timer(
                RENDERING_SYSTEM.name,
                RENDERING_SYSTEM.update_phase,
                timer,
            )?;
            Ok(())
        }
        Err(e) => {
            match e.downcast_ref::<RendererError>() {
                Some(RendererError::SurfaceLost) => {
                    // Recreate lost surface
                    timer.end_context()?; // End "Render" context
                    engine.system_manager.update_system_timer(
                        RENDERING_SYSTEM.name,
                        RENDERING_SYSTEM.update_phase,
                        timer,
                    )?;
                    Ok(engine.renderer.resize(engine.window_size))
                }
                Some(RendererError::SurfaceOutOfMemory) => {
                    panic!("Critical: Renderer error, system out of memory");
                }
                _ => Err(e),
            }
        }
    }
}
