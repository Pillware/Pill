use crate::{
    config::RENDERING_SYSTEM, 
    ecs::{ 
        components::{postprocessing_volume_component, transform_component, volume::Volume3D}, 
        scene, update_transform_matrices, CameraAspectRatio, CameraComponent, Component, ComponentStorage, EguiManagerComponent, EntityHandle, MeshRenderingComponent, PostprocessingVolumeComponent, TransformComponent, UpdatePhase 
    }, 
    engine::{self, Engine}, 
    graphics::{ compose_render_queue_key, PostprocessingEffect, PostprocessingEffectRendererData, PostprocessingVolumeRendererData, RenderQueueItem, RenderQueueKey, RendererMaterialHandle }, 
    internal::MaterialParameter, 
    resources::{ Material, MaterialHandle, Mesh, MeshHandle, ResourceManager }
};
use pill_core::{ warn, EngineError, LogContext, PillSlotMapKey, PillStyle, RendererError, Timer, Vector3f };
use std::{ collections::HashMap, ops::Range, time::Instant };
use anyhow::{ Result, Context, Error };
use boolinator::Boolinator;

pub fn get_active_camera(engine: &mut Engine) -> Result<(&mut CameraComponent, &mut TransformComponent)> {
    let active_scene = engine.scene_manager.get_active_scene_mut()?;

    // Find first enabled camera and use it as active
    for (entity_handle, camera_component, transform_component) in active_scene.get_two_component_iterator_mut::<CameraComponent, TransformComponent>()? {
        if camera_component.enabled {
            return Ok((camera_component, transform_component));
        }
    }

    Err(EngineError::NoActiveCamera.into())
}

pub fn rendering_system(engine: &mut Engine) -> Result<()> {
    let mut timer = Timer::new();
    timer.begin_context("rendering_system update");
    timer.record("Get active camera");

    // 1. Get active camera and update it
    let aspect_ratio = engine.window_size.width as f32 / engine.window_size.height as f32;
    
    let (camera_component, transform_component) = get_active_camera(engine)?;

    // Update active camera component
    if let CameraAspectRatio::Automatic(_) = camera_component.aspect {
        camera_component.aspect = CameraAspectRatio::Automatic(aspect_ratio);
    }

    let active_camera_renderer_handle = camera_component.renderer_resource_handle
        .ok_or(Error::new(RendererError::RendererResourceNotFound))?;

    // Extract values before calling renderer
    let active_camera_position = transform_component.position.clone();
    let active_camera_rotation = transform_component.rotation.clone();
    let active_camera_fov = camera_component.fov;
    let active_camera_aspect_value = camera_component.aspect.get_value().clone();
    let active_camera_range = camera_component.range.clone();
    let active_camera_clear_color = camera_component.clear_color.clone();

    // Update camera in the renderer
    engine.renderer.update_camera(
        active_camera_renderer_handle,
        active_camera_position,
        active_camera_rotation,
        active_camera_fov,
        active_camera_aspect_value,
        active_camera_range,
        active_camera_clear_color
    )?;

    // 2. Get postprocessing effects from postprocessing volumes
    timer.record("Get postprocessing volumes");


     // Find all postprocessing volumes affecting active camera.
    // Get their effects (iterate over them) and for each effect update its material using its parameters

    // Pass to render method array of effects, 
    // their intensity depening on the camera position (caluclated here)
    // Then in render pass, iterate over effects, get material from each and update its parameters buffer
    // Then bind material and draw full screen triangle
   let postprocessing_volumes_renderer_data: Vec<PostprocessingVolumeRendererData> = {

        let mut postprocessing_effects_renderer_data: Vec<PostprocessingVolumeRendererData> = Vec::new();

        let active_scene = engine.scene_manager.get_active_scene()?;
        for (entity_handle, postprocessing_volume_component, transform_component) in active_scene.get_two_component_iterator::<PostprocessingVolumeComponent, TransformComponent>()? {
            let mut postprocessing_effects: Vec<PostprocessingEffectRendererData> = Vec::new();
            if postprocessing_volume_component.is_enabled {
                let postprocessing_volume_influence = if postprocessing_volume_component.is_global {
                    1.0
                } else {
                    postprocessing_volume_component.contains_point_falloffed(active_camera_position)
                };

                if postprocessing_volume_influence <= 0.0 {
                    continue;
                }

                // Process enabled effects
                for (type_id, storage) in postprocessing_volume_component.effects.iter_storages() {
                    let effect = storage.get_dyn().unwrap();
                    if !effect.is_enabled() {
                        continue;
                    }

                    let material_resource_handle = effect.get_material_handle(engine);
                    let material = engine.get_resource::<Material>(&material_resource_handle)?;
                    let material_renderer_resource_handle = material
                        .renderer_resource_handle
                        .unwrap();

                    postprocessing_effects.push(PostprocessingEffectRendererData {
                        material_handle: material_renderer_resource_handle,
                        material_parameters: effect.get_parameters(),
                        influence: postprocessing_volume_influence,
                    });
                }
            }

            if !postprocessing_effects.is_empty() {
                postprocessing_effects_renderer_data.push(
                    PostprocessingVolumeRendererData {
                        effect_data: postprocessing_effects
                    }
                );
            }
        }

        postprocessing_effects_renderer_data
    };

    // 3. Clear the render queue
    timer.record("Clear render queue");

    engine.render_queue.clear();
    engine.render_queue.reserve(200000); // Reserve space for 1000 items

    // 4. Prepare render queue
    timer.record("Prepare render queue");

    let mut _matrix_calculation_duration: f32 = 0.0;
    let mut add_to_render_queue_duration: f32 = 0.0;

    // Iterate mesh rendering components
    let active_scene_handle = engine.scene_manager.get_active_scene_handle()?;
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

    // 5. Prepare other rendering resources
    timer.record("Get storages and egui ui");

    let egui_ui = EguiManagerComponent::get_ui(engine);// egui_manager_component.get_ui(engine);

    let active_scene = engine.scene_manager.get_active_scene_mut()?;
    let transform_component_storage = active_scene.get_component_storage::<TransformComponent>()
        .context(format!("{}: Cannot get {}", "rendering_system".specific_object_style(), "TransformComponents".specific_object_style())).unwrap();

    // 6. Render
    timer.begin_context("Render");

    // Render
    match engine.renderer.render(
        active_camera_renderer_handle, 
        &engine.render_queue, 
        transform_component_storage,
        &postprocessing_volumes_renderer_data,
        egui_ui,
        0.0,
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
