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
            // Convert engine Texture -> renderer texture handle
            let tex_logo_rt = tex_logo
                .renderer_resource_handle
                .expect("renderer handle for logo texture");

            // Ensure EguiClient exists in global render state
            let egui_client = {
                let rs = engine.get_global_component_mut::<RenderStateComponent>()?;
                if rs.egui_client.is_none() {
                    rs.egui_client = Some(crate::ecs::EguiClient::new());
                }
                rs.egui_client.as_ref().unwrap().clone()
            };

            // Logo overlay
            let h: f32 = 0.04;
            let rect_logo = [0.98 - 3.0 * h, 0.02, 0.98, 0.02 + h];

            let offscreen_color_texture =
                engine.renderer.create_render_target(RendererTargetDesc {
                    name: "offscreen_color".to_string(),
                    format: fmt,
                    width: engine.window_size.width,
                    height: engine.window_size.height,
                })?;

            // Create depth and color texture
            let depth_texture = engine.renderer.create_depth_texture("depth_texture")?;

            // Linear-depth render target for post-processing
            let linear_depth_rt = engine.renderer.create_render_target(RendererTargetDesc {
                name: "linear_depth".to_string(),
                format: wgpu::TextureFormat::R16Float,
                width: engine.window_size.width,
                height: engine.window_size.height,
            })?;

            // Build passes as an array with elements, then convert to Vec
            // Load equirectangular HDR environment map (as 2D)
            let env_tex_handle = {
                // Add if not already present
                match engine.get_resource_handle::<Texture>("ibl_env_equirect") {
                    Ok(h) => h,
                    Err(_) => {
                        let tex = Texture::new(
                            "ibl_env_equirect",
                            TextureType::Linear,
                            ResourceLoadType::Path("ibl/HDR_111_Parking_Lot_2_Env.hdr".into()),
                        );
                        engine.add_resource::<Texture>(tex)?
                    }
                }
            };
            let env_tex_rt = engine
                .get_resource::<Texture>(&env_tex_handle)?
                .renderer_resource_handle
                .expect("renderer handle for env texture");

            // Create IBL output resources up front
            let ibl_irradiance_rt = engine.renderer.create_render_target(RendererTargetDesc {
                name: "ibl_irradiance_2d".to_string(),
                format: wgpu::TextureFormat::Rgba16Float,
                width: 64,
                height: 32,
            })?;
            let ibl_brdf_rt = engine.renderer.create_render_target(RendererTargetDesc {
                name: "ibl_brdf_lut".to_string(),
                format: wgpu::TextureFormat::Rgba16Float,
                width: 512,
                height: 512,
            })?;

            // Build and run diffuse IBL convolution once (init-time)
            let mut pass_ibl_diff = crate::graphics::PassIblDiffuseEquirect::new(
                "ibl_diffuse_equirect",
                env_tex_rt,
                ibl_irradiance_rt,
                wgpu::TextureFormat::Rgba16Float,
            );
            // Manually init to precompute before scene pass init
            {
                // SAFETY: renderer exposes init path; we can call with engine resources
                let self_ptr: *mut dyn crate::graphics::PillRenderer = &mut *engine.renderer;
                let rm_ptr: *mut crate::resources::ResourceManager = &mut engine.resource_manager;
                unsafe {
                    pass_ibl_diff.init(&mut *self_ptr, &mut *rm_ptr)?;
                }
            }
            // Provide BRDF target to specular pass

            // Create prefilter target (mip chain) in RenderSystem and pass to pass
            let prefilter_handle = {
                let device = engine.renderer.get_device();
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("ibl_prefilter_2d"),
                    size: wgpu::Extent3d {
                        width: 256,
                        height: 128,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 5,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
                let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                    address_mode_u: wgpu::AddressMode::Repeat,
                    address_mode_v: wgpu::AddressMode::ClampToEdge,
                    address_mode_w: wgpu::AddressMode::ClampToEdge,
                    mag_filter: wgpu::FilterMode::Linear,
                    min_filter: wgpu::FilterMode::Linear,
                    mipmap_filter: wgpu::FilterMode::Linear,
                    lod_min_clamp: 0.0,
                    lod_max_clamp: 5.0,
                    ..Default::default()
                });
                let handle = engine.resource_manager.gpu_mut().textures.insert(
                    crate::renderer::resources::RendererTexture {
                        texture: tex,
                        texture_view: view,
                        sampler,
                        format: wgpu::TextureFormat::Rgba16Float,
                    },
                );
                // Sanity: log handle id and format
                {
                    use pill_core::PillSlotMapKey;
                    let fmt = engine
                        .resource_manager
                        .gpu()
                        .textures
                        .get(handle)
                        .map(|t| t.format)
                        .unwrap_or(wgpu::TextureFormat::Rgba8UnormSrgb);
                    log::info!(
                        "rendering_system: created prefilter handle=({},{}) format={:?}",
                        handle.index(),
                        handle.generation(),
                        fmt
                    );
                }
                handle
            };

            // Specular IBL prefilter + BRDF LUT
            let mut pass_ibl_spec = crate::graphics::PassIblSpecularEquirect::new(
                "ibl_specular_equirect",
                env_tex_rt,
                prefilter_handle,
                ibl_brdf_rt,
                5,
            );
            {
                let self_ptr: *mut dyn crate::graphics::PillRenderer = &mut *engine.renderer;
                let rm_ptr: *mut crate::resources::ResourceManager = &mut engine.resource_manager;
                unsafe {
                    pass_ibl_spec.init(&mut *self_ptr, &mut *rm_ptr)?;
                }
            }
            let ibl_prefilter_rt = pass_ibl_spec.prefilter_texture_handle();

            let passes: Vec<Box<dyn Pass>> = vec![
                // Keep the pass in list so it's a no-op afterwards (done flag)
                Box::new(pass_ibl_diff),
                Box::new(pass_ibl_spec),
                // Skybox (draws into offscreen color); must come before scene
                // TODO: move after all geometry was drawn and draw at end of depth
                Box::new(crate::graphics::PassSkyboxEquirect::new(
                    "skybox_equirect",
                    offscreen_color_texture,
                    fmt,
                    env_tex_rt,
                )),
                Box::new(crate::graphics::pass_scene::PassScene::new(
                    "scene",
                    offscreen_color_texture,
                    depth_texture,
                    fmt,
                    true, // load offscreen color (skybox already cleared/drew background)
                    Some(ibl_irradiance_rt),
                    Some(ibl_prefilter_rt),
                    Some(ibl_brdf_rt),
                )),
                // Linearize scene depth into a color RT for later passes
                Box::new(crate::graphics::PassLinearizeDepth::new(
                    "linearize_depth",
                    depth_texture,
                    linear_depth_rt,
                    wgpu::TextureFormat::R16Float,
                )),
                Box::new(crate::graphics::PassCompose::new(
                    "compose",
                    offscreen_color_texture,
                    fmt,
                )),
                Box::new(crate::graphics::PassOverlayUV::new(
                    "overlay_uv",
                    [0.75, 0.75, 0.95, 0.95],
                    fmt,
                )),
                Box::new(crate::graphics::PassOverlayDepth::new(
                    "overlay_depth",
                    [0.75, 0.50, 0.95, 0.70],
                    [1.0, 1.0, 1.0, 1.0],
                    fmt,
                    linear_depth_rt,
                )),
                Box::new(PassLogo::new(
                    "overlay_logo",
                    rect_logo,
                    [1.0, 1.0, 1.0, 1.0],
                    tex_logo_rt,
                    fmt,
                )),
                Box::new(crate::graphics::PassEgui::new(
                    "egui",
                    engine.window.clone(),
                    egui_client,
                )),
            ];
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

    let egui_ui: Box<dyn Fn(&egui::Context) + Send> = EguiManagerComponent::get_ui(engine); // egui_manager_component.get_ui(engine)
    if let Ok(rs) = engine.get_global_component_mut::<RenderStateComponent>() {
        if let Some(ref client) = rs.egui_client {
            client.set_ui(egui_ui);
        }
    }

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
    match crate::graphics::render_with_factory(engine.renderer.as_mut(), &factory, &mut timer) {
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
        "../../../res/textures/pill_logo_horizontal_white.png"
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
