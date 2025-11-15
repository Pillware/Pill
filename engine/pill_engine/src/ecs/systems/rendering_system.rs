use crate::graphics::RendererTargetDesc;
use crate::{
    config::RENDERING_SYSTEM,
    ecs::{
        scene, update_transform_matrices, CameraAspectRatio, CameraComponent, Component,
        ComponentStorage, EguiManagerComponent, EntityHandle, MeshRenderingComponent,
        TransformComponent, UpdatePhase,
    },
    engine::Engine,
    graphics::{
        compose_render_queue_key, RenderQuery, RenderQueueFactory, RenderQueueItem, RenderQueueKey,
    },
    resources::{Mesh, MeshHandle, Model, PBRMaterial, PBRMaterialHandle, ResourceManager},
};

use pill_core::{EngineError, PillSlotMapKey, PillStyle, RendererError, Timer};

use anyhow::{Context, Error, Result};
use boolinator::Boolinator;
use log::debug;
use std::{ops::Range, time::Instant};

use crate::config::{
    DEFAULT_COLOR_TEXTURE_NAME, DEFAULT_MATERIAL_NAME, DEFAULT_NORMAL_TEXTURE_NAME, MAX_MATERIALS,
    MAX_MESHES, MAX_MODELS, MAX_SOUNDS, MAX_TEXTURES,
};
use crate::ecs::components::render_state_component::RenderStateComponent;
use crate::graphics::{Pass, PassCompose, PassLogo, PassOverlayDepth, PassOverlayUV};
use crate::resources::{Resource, ResourceLoadType, Texture, TextureHandle, TextureType};

// Constants for hot path optimization
const MAX_RENDERABLES_CAPACITY: usize = 100000; // Maximum expected renderables per frame

// Static preallocated Vec for dirty entities to avoid per-frame allocation
// SAFETY: Single-threaded rendering system - no concurrency issues
static mut DIRTY_ENTITIES: Vec<EntityHandle> = Vec::new();

pub fn rendering_system(engine: &mut Engine) -> Result<()> {
    // One-time bootstrap: register resource types and create default resources
    // Require RenderStateComponent; if missing, panic via unwrap
    let need_bootstrap = !engine
        .get_global_component::<RenderStateComponent>()
        .unwrap()
        .boot_done;
    if need_bootstrap {
        init_default_resources(engine)?;

        // Preallocate render queue capacity for hot path optimization
        engine.render_queue.reserve(MAX_RENDERABLES_CAPACITY); // Reserve space for up to 100k renderables

        // Preallocate static dirty_entities Vec capacity
        unsafe {
            DIRTY_ENTITIES.reserve(MAX_RENDERABLES_CAPACITY); // Reserve space for up to 100k entities
        }

        // Install renderer passes once during bootstrap
        {
            // Resolve logo texture created in init_default_resources
            let tex_logo = engine
                .resource_manager
                .get_resource_by_name::<Texture>("pill_logo_horizontal_white")?;
            let fmt = engine.renderer.get_surface_format();

            let offscreen_color_texture =
                engine.renderer.create_render_target(RendererTargetDesc {
                    name: "offscreen_color".to_string(),
                    format: fmt,
                    width: engine.window_size.width,
                    height: engine.window_size.height,
                })?;

            // Create depth and color texture
            let depth_texture = engine.renderer.create_depth_texture("depth_texture")?;

            // Build passes
            let mut passes: Vec<Box<dyn Pass>> = Vec::new();

            // Scene (renders into offscreen targets)
            passes.push(Box::new(crate::graphics::pass_scene::PassScene::new(
                "scene",
                offscreen_color_texture,
                depth_texture,
                fmt,
            )));

            // Compose (tone map) from offscreen to swapchain (last)
            passes.push(Box::new(crate::graphics::PassCompose::new(
                "compose",
                offscreen_color_texture,
                fmt,
            )));

            // UV overlay
            passes.push(Box::new(crate::graphics::PassOverlayUV::new(
                "overlay_uv",
                [0.75, 0.75, 0.95, 0.95],
                fmt,
            )));

            // Depth overlay
            passes.push(Box::new(crate::graphics::PassOverlayDepth::new(
                "overlay_depth",
                [0.75, 0.50, 0.95, 0.70],
                [1.0, 1.0, 1.0, 1.0],
                fmt,
                depth_texture,
            )));

            // Logo overlay
            let h: f32 = 0.04;
            let rect_logo = [0.98 - 3.0 * h, 0.02, 0.98, 0.02 + h];
            // Convert engine Texture -> renderer texture handle
            let tex_logo_rt = tex_logo
                .renderer_resource_handle
                .expect("renderer handle for logo texture");
            passes.push(Box::new(PassLogo::new(
                "overlay_logo",
                rect_logo,
                [1.0, 1.0, 1.0, 1.0],
                tex_logo_rt,
                fmt,
            )));

            engine.renderer.set_passes(passes)?;
        }

        if let Ok(rs) = engine.get_global_component_mut::<RenderStateComponent>() {
            rs.boot_done = true;
        }

        // Skip the rest of the frame; camera may not exist yet during bootstrap
        return Ok(());
    }

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

    timer.record("Prepare render queue");

    // Prepass: deferred material updates (CPU -> GPU) for dirty PBR materials
    {
        // Collect updates immutably to avoid borrow conflicts
        let mut updates: Vec<(
            PBRMaterialHandle,
            crate::graphics::RendererMaterialHandle,
            crate::graphics::MaterialDesc,
        )> = Vec::new();
        if let Ok(storage) = engine
            .resource_manager
            .get_resource_storage::<PBRMaterial>()
        {
            for (h, opt_mat) in storage.data.iter() {
                if let Some(mat) = opt_mat {
                    if mat.is_dirty {
                        if let Some(rm_handle) = mat.renderer_resource_handle {
                            let map_tex = |th: &Option<TextureHandle>| -> Option<crate::graphics::RendererTextureHandle> {
                                th.as_ref().and_then(|hh| {
                                    engine
                                        .resource_manager
                                        .get_resource::<Texture>(hh)
                                        .ok()
                                        .and_then(|t| t.renderer_resource_handle)
                                })
                            };
                            let desc = crate::graphics::MaterialDesc {
                                label: "upd",
                                albedo: [mat.albedo.x, mat.albedo.y, mat.albedo.z],
                                metallic: mat.metallic,
                                roughness: mat.roughness,
                                emissive: [mat.emissive.x, mat.emissive.y, mat.emissive.z],
                                albedo_tex: map_tex(&mat.albedo_texture),
                                normal_tex: map_tex(&mat.normal_texture),
                                metallic_roughness_tex: map_tex(&mat.metallic_roughness_texture),
                                emissive_tex: map_tex(&mat.emissive_texture),
                            };
                            updates.push((h, rm_handle, desc));
                        }
                    }
                }
            }
        }
        // Apply updates and clear dirty flags
        for (ph, rmh, desc) in updates {
            let _ = engine.renderer.update_material(rmh, desc);
            if let Ok(m) = engine.resource_manager.get_resource_mut::<PBRMaterial>(&ph) {
                m.is_dirty = false;
            }
        }
    }

    // Use static preallocated dirty_entities Vec
    unsafe {
        DIRTY_ENTITIES.clear(); // Clear and reuse preallocated Vec

        // Phase 1: Sweep components; route transforms needing matrix update to a batch, push clean directly
        // [SIMILAR] Batch transform updates; avoid per-draw matrix work
        for (entity_handle, transform_component, mesh_rendering_component) in
            engine
                .scene_manager
                .get_two_component_iterator_mut::<TransformComponent, MeshRenderingComponent>(
                    active_scene_handle,
                )?
        {
            if transform_component.matrix_update_required {
                // defer update to batch
                DIRTY_ENTITIES.push(entity_handle);
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
        if !DIRTY_ENTITIES.is_empty() {
            timer.begin_context(&format!("Batch update {} transforms", DIRTY_ENTITIES.len()));
            for entity_handle in DIRTY_ENTITIES.iter() {
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
        for entity_handle in DIRTY_ENTITIES.iter() {
            if let Ok(mesh_rendering_component) = engine
                .scene_manager
                .get_entity_component::<MeshRenderingComponent>(*entity_handle, active_scene_handle)
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
    }

    timer.record("Get component storages");

    let egui_ui = EguiManagerComponent::get_ui(engine); // egui_manager_component.get_ui(engine);

    // Storages fetched inside renderer via immutable Engine

    timer.begin_context("Render");

    // Build WorldView with raw pointers to avoid borrow conflicts
    let world_view = {
        let active_scene = engine.scene_manager.get_active_scene()?;
        let camera_components =
            active_scene.get_component_storage::<CameraComponent>()? as *const _;
        let transform_components =
            active_scene.get_component_storage::<TransformComponent>()? as *const _;
        let render_queue_ptr = &engine.render_queue as *const _;
        crate::graphics::WorldView {
            active_camera: active_camera_entity_handle,
            render_queue_ptr,
            camera_components_ptr: camera_components,
            transform_components_ptr: transform_components,
        }
    };
    let factory = crate::graphics::WorldViewFactory { world: world_view };
    match crate::graphics::render_with_factory(
        engine.renderer.as_mut(),
        &factory,
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

fn init_default_resources(engine: &mut Engine) -> Result<(), Error> {
    let max_texture_count = engine
        .config
        .get_int("MAX_TEXTURES")
        .unwrap_or(MAX_TEXTURES as i64) as usize;
    let max_mesh_count = engine
        .config
        .get_int("MAX_MESHES")
        .unwrap_or(MAX_MESHES as i64) as usize;
    let max_material_count = engine
        .config
        .get_int("MAX_MATERIALS")
        .unwrap_or(MAX_MATERIALS as i64) as usize;
    let max_model_count = engine
        .config
        .get_int("MAX_MODELS")
        .unwrap_or(MAX_MODELS as i64) as usize;
    // TODO: Move to SoundSystem init
    let max_sound_count = engine
        .config
        .get_int("MAX_SOUNDS")
        .unwrap_or(MAX_SOUNDS as i64) as usize;

    engine.register_resource_type::<Texture>(max_texture_count)?;
    engine.register_resource_type::<Mesh>(max_mesh_count)?;
    engine.register_resource_type::<PBRMaterial>(max_material_count)?;
    engine.register_resource_type::<Model>(max_model_count)?;
    engine.register_resource_type::<crate::resources::Sound>(max_sound_count)?;

    let default_color = Box::new(*include_bytes!("../../../res/textures/default_color.png"));
    let default_normal = Box::new(*include_bytes!("../../../res/textures/default_normal.png"));
    let mut color = Texture::new(
        DEFAULT_COLOR_TEXTURE_NAME,
        TextureType::Gamma,
        ResourceLoadType::Bytes(default_color),
    );
    color.initialize(engine)?;
    engine.resource_manager.add_resource(color)?;
    let mut normal = Texture::new(
        DEFAULT_NORMAL_TEXTURE_NAME,
        TextureType::Linear,
        ResourceLoadType::Bytes(default_normal),
    );
    normal.initialize(engine)?;
    engine.resource_manager.add_resource(normal)?;

    // Pill logo (overlay) texture
    let pill_logo = Box::new(*include_bytes!(
        "../../../../../media/logo/pill_logo_horizontal_white.png"
    ));
    let mut tex_logo = Texture::new(
        "pill_logo_horizontal_white",
        TextureType::Gamma,
        ResourceLoadType::Bytes(pill_logo),
    );
    tex_logo.initialize(engine)?;
    engine.resource_manager.add_resource(tex_logo)?;

    let mut mat = PBRMaterial::new(DEFAULT_MATERIAL_NAME);
    mat.initialize(engine)?;
    engine.resource_manager.add_resource(mat)?;

    Ok(())
}
