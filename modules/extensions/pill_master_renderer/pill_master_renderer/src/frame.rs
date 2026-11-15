//! Fills the frame the renderer draws, once per update.
//!
//! # Responsibilities
//!
//! - Collect the frame from the world each update: the camera to draw through,
//!   every drawable the world offers, and the clock the shaders read
//!   ([`rendering_system`]).
//! - Turn the game's pipeline into plain passes. Entries that are switched off
//!   or point at dead assets are left out, and a chain whose surface pass
//!   breaks the renderer's contract is reported in the log.
//!
//! # Design
//!
//! [`RenderFrame`], [`RenderInstance`] and [`ResolvedPass`] are plain data
//! from `pill_renderer_api`, re-exported here. Filling them is renderer
//! behaviour, so it lives in this module, where a renderer reload can replace
//! it. The system also makes choices the world data does not spell out: which
//! of several cameras to use, what each pipeline pass turns into, and the draw
//! order each instance carries.

// Standard library
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

// External crates
use pill_core::telemetry::telemetry_target;
use pill_core::{info, warn};
use pill_engine::{
    system, AssetManager, Component, Entity, Handle, Query, Res, ResMut, SystemError,
};
pub use pill_renderer_api::frame::{
    CullMode, PassKind, PassTarget, RenderFrame, RenderInstance, ResolvedPass,
};
use pill_renderer_api::{AssetKey, ChainSource};

// Current crate
use crate::{
    assets::{asset_key, Material, Mesh},
    components::{CameraComponent, MeshRendererComponent, TransformComponent},
    resources::RenderingManager,
};

// =============================================================================
// Constants
// =============================================================================

/// Which producer filled `RenderFrame.ui` on the last frame, so the log states
/// a change once instead of on every frame.
///
/// This is module state rather than a frame field: the frame is plain data
/// shared through the renderer contract, while the producer is a fact about
/// this renderer's world. A reload gives the module fresh statics, so the next
/// frame logs which producer is feeding it again.
static UI_PRODUCER: AtomicU8 = AtomicU8::new(PRODUCER_UNSET);

/// Whether this renderer image has already asked the UI producer to send its
/// textures again.
///
/// A reload gives the module a fresh image with fresh statics, so the flag
/// answers "has this image asked yet?". It starts false, so the first frame
/// that finds a UI slot asks for every texture again.
static UI_TEXTURES_REQUESTED: AtomicBool = AtomicBool::new(false);

/// No frame has been filled since this image started.
const PRODUCER_UNSET: u8 = 0;
/// The game's own egui UI.
const PRODUCER_GAME: u8 = 1;
/// No producer: the frame carries no UI.
const PRODUCER_NONE: u8 = 2;

// =============================================================================
// Types
// =============================================================================

/// The resources [`rendering_system`] reads besides the frame, the assets and
/// the rendering manager.
///
/// They share one parameter because a system function takes at most seven.
/// They include the world's only UI producer: the slot a game's own UI
/// publishes into ([`UiFrameSlot`](pill_ui_frame::UiFrameSlot)). A world that
/// never installed a slot has no UI.
type RenderingExtras<'w> = (
    Res<'w, pill_engine::Time>,
    Res<'w, pill_renderer_api::SurfaceCameras>,
    ResMut<'w, pill_engine::DebugDraw>,
    Res<'w, pill_renderer_api::SelectionOutlines>,
    ResMut<'w, pill_renderer_api::PickRequests>,
    ResMut<'w, pill_ui_frame::UiFrameSlot>,
    ResMut<'w, pill_renderer_api::PreviewRequests>,
);

// =============================================================================
// Free Functions
// =============================================================================

/// Builds the key the frame uses for `handle`.
///
/// The asset layer packs the handle into a number; the renderer contract
/// carries that value as an [`AssetKey`].
fn frame_asset_key<T: pill_engine::Asset>(handle: Handle<T>) -> AssetKey {
    AssetKey(asset_key(handle))
}

/// Returns the pass chain this frame should run, read from the pipeline the
/// game selected.
///
/// The result keeps only the passes that are switched on and whose handle still
/// resolves, and sorts them by their `order` value. Passes with equal `order`
/// keep the pipeline's own order, so a game lists its passes in the order it
/// wants and only a pass inserted later needs an explicit `order`.
///
/// A pass can be switched off in two places, and both are the game's own
/// choice rather than a fault: on the pipeline entry, which affects only this
/// chain, or on the pass asset itself, which affects every chain that names it.
fn resolve_passes(
    assets: &pill_engine::AssetManager,
    manager: Option<&RenderingManager>,
) -> Vec<ResolvedPass> {
    let handle = manager.and_then(|manager| manager.pipeline());
    let Some(pipeline) = handle.and_then(|handle| assets.get(handle)) else {
        // The handle exists but no longer resolves: the pipeline asset was
        // removed after the game selected it. That earns a warning, because
        // the game asked for a chain and lost it. A game that never selected a
        // pipeline is not news. Both fall back to the built-in pass.
        if handle.is_some() {
            warn!(
                target: telemetry_target::RENDERING,
                "the pipeline the manager holds no longer resolves; running the built-in pass"
            );
        }
        return vec![ResolvedPass::builtin()];
    };

    let mut stale_pass_handle_count = 0usize;
    let mut passes = Vec::with_capacity(pipeline.passes.len());
    for entry in &pipeline.passes {
        // The pipeline entry is switched off, so this chain leaves the pass out.
        if !entry.enabled {
            continue;
        }
        let Some(pass) = assets.get(entry.pass) else {
            stale_pass_handle_count += 1;
            continue;
        };
        // The pass asset itself is switched off, which hides it from every
        // chain that names it.
        if !pass.enabled {
            continue;
        }

        // A pass that names a material draws with that material's shader,
        // parameters and textures. Values the pass sets itself win over the
        // material's. They are merged here, while the chain is resolved, so the
        // renderer only ever sees one plain pass. Editing the material moves
        // the asset revision, which resolves the chain again.
        let mut shader = (pass.shader != Handle::INVALID).then(|| frame_asset_key(pass.shader));
        let mut parameters = BTreeMap::new();
        let mut textures = BTreeMap::new();
        if pass.material != Handle::INVALID {
            match assets.get(pass.material) {
                Some(material) => {
                    if shader.is_none() && material.shader != Handle::INVALID {
                        shader = Some(frame_asset_key(material.shader));
                    }
                    parameters.extend(material.parameters.clone());
                    textures.extend(
                        material.textures.iter().map(|(slot, binding)| {
                            (slot.clone(), frame_asset_key(binding.texture))
                        }),
                    );
                }
                None => warn!(
                    target: telemetry_target::RENDERING,
                    pass = pass.name.as_str(),
                    "pass names a material that is not loaded"
                ),
            }
        }
        parameters.extend(pass.parameters.clone());
        textures.extend(
            pass.textures
                .iter()
                .map(|(slot, handle)| (slot.clone(), frame_asset_key(*handle))),
        );

        passes.push(ResolvedPass {
            name: pass.name.clone(),
            shader,
            settings: pass.settings.clone(),
            parameters,
            inputs: pass.inputs.clone(),
            textures,
        });
    }
    if stale_pass_handle_count > 0 {
        warn!(
            target: telemetry_target::RENDERING,
            stale_passes = stale_pass_handle_count,
            passes = pipeline.passes.len(),
            "pass handles no longer resolve and are left out of the chain"
        );
    }
    passes.sort_by_key(|pass| pass.settings.order);

    // A pass that writes the surface has to run last, or the window shows
    // something other than what the chain produced. Checking that here, where
    // the chain changes, reports it once instead of once a frame.
    let surface_positions: Vec<usize> = passes
        .iter()
        .enumerate()
        .filter(|(_, pass)| matches!(pass.settings.target, PassTarget::Surface))
        .map(|(index, _)| index)
        .collect();
    let surface_is_last = surface_positions.len() == 1 && surface_positions[0] + 1 == passes.len();
    if !surface_positions.is_empty() && !surface_is_last {
        warn!(
            target: telemetry_target::RENDERING,
            positions = %surface_positions
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            surface_passes = surface_positions.len(),
            "more than one surface pass, or one before the end; only the last pass's writes reach the window"
        );
    }
    passes
}

/// Picks the camera the primary surface draws through, out of every enabled
/// camera in the world.
///
/// The highest `priority` wins. Equal priorities go to the lowest entity id, so
/// the choice stays the same from frame to frame while the same cameras exist,
/// whatever order the query walks them in. `None` means no camera is enabled;
/// the primary surface is then drawn only if a camera is bound to it.
fn pick_camera(
    cameras: impl IntoIterator<Item = (u64, CameraComponent, TransformComponent)>,
) -> Option<(CameraComponent, TransformComponent)> {
    let mut selected: Option<(u64, CameraComponent, TransformComponent)> = None;
    for (entity, camera, transform) in cameras {
        if !camera.enabled {
            continue;
        }
        let better = match &selected {
            Some((selected_entity, selected_camera, _)) => {
                camera.priority > selected_camera.priority
                    || (camera.priority == selected_camera.priority && entity < *selected_entity)
            }
            None => true,
        };
        if better {
            selected = Some((entity, camera, transform));
        }
    }
    selected.map(|(_, camera, transform)| (camera, transform))
}

/// Returns the shared names of the components [`rendering_system`] reads.
///
/// [`RenderCapabilities::consumed_components`](crate::api::RenderCapabilities::consumed_components)
/// carries them. The host compares that list with the render components a
/// project uses and warns about any the renderer ignores. The names come from
/// the component types themselves rather than from string literals, so they
/// cannot drift from the types. Keep the list in step with the system's query
/// parameters.
pub(crate) fn consumed_component_names() -> Vec<String> {
    [
        TransformComponent::shared_name(),
        CameraComponent::shared_name(),
        MeshRendererComponent::shared_name(),
    ]
    .into_iter()
    .flatten()
    .map(str::to_owned)
    .collect()
}

/// Records which producer filled the frame, logging its name when that changed.
fn note_ui_producer(producer: u8, producer_name: &str) {
    if UI_PRODUCER.swap(producer, Ordering::Relaxed) != producer {
        info!(
            target: telemetry_target::UI,
            "[ui] producer={producer_name}"
        );
    }
}

/// Fills the frame from the current world, once per update.
///
/// It runs as the post-update `rendering` system, so gameplay has already
/// written the components it reads. The pass chain is resolved again only when
/// the asset revision or the pipeline's key moves, so most frames reuse the
/// previous result.
///
/// A world missing the frame resource or the asset store is not an error: the
/// system returns and draws nothing. A world with no enabled camera still gets
/// a frame, but the primary surface has no view unless a camera is bound to it,
/// and the renderer skips a frame that has no views at all.
#[system(name = "rendering", post_update, edit_mode)]
pub fn rendering_system(
    mut frame: ResMut<RenderFrame>,
    assets: Res<AssetManager>,
    manager: Res<RenderingManager>,
    (
        time,
        surface_cameras,
        mut debug_draw,
        outlines,
        mut pick_requests,
        mut ui_slot,
        mut preview_requests,
    ): RenderingExtras<'_>,
    mut cameras: Query<(Entity, &TransformComponent, &CameraComponent)>,
    mut drawables: Query<(Entity, &TransformComponent, &MeshRendererComponent)>,
) -> Result<(), SystemError> {
    let Some(mut frame) = frame.get_mut() else {
        return Ok(());
    };
    let Some(assets) = assets.get() else {
        return Ok(());
    };

    // Read the manager every frame, because a game can set a pipeline, or
    // switch a pass inside one, at any point between two frames. Resolve the
    // chain again only when the pipeline key or the asset revision moved:
    // resolution clones every pass's strings and maps. The generation it bumps
    // tells the renderer whether the chain it built is still this chain.
    let chain_source = ChainSource {
        revision: assets.revision(),
        pipeline: manager
            .get()
            .and_then(|manager| manager.pipeline())
            .map(frame_asset_key),
    };
    if frame.chain_source != Some(chain_source) {
        frame.passes = resolve_passes(assets, manager.get());
        frame.chain_source = Some(chain_source);
        frame.chain_generation = frame.chain_generation.wrapping_add(1);
    }

    frame.sequence = frame.sequence.wrapping_add(1);
    if let Some(time) = time.get() {
        // Add the step to the frame's own clock instead of reading the engine
        // clock here: a shader is handed this value, so it should grow by the
        // same step the game's update saw.
        frame.delta_seconds = time.delta_seconds();
        frame.seconds += frame.delta_seconds;
    }
    // The debug geometry systems drew, with the timed items aged by this
    // frame's step. Empty while debug drawing is off, and when the world has no
    // debug drawing resource.
    let delta_seconds = frame.delta_seconds;
    frame.debug = debug_draw
        .get_mut()
        .map(|mut debug_draw| debug_draw.collect_frame(delta_seconds))
        .unwrap_or_default();
    // The selection outlines an editor asked for, copied from the resource. A
    // world without the resource outlines nothing.
    frame.outlines = outlines
        .get()
        .cloned()
        .unwrap_or_else(pill_renderer_api::SelectionOutlines::default);
    // Pick requests queued since the last frame. The renderer answers them
    // after it draws.
    frame.pick_requests = pick_requests
        .get_mut()
        .map(|mut requests| std::mem::take(&mut requests.pending))
        .unwrap_or_default();
    // Preview requests, taken the same way. The renderer draws at most one per
    // frame, oldest first, and refuses the rest.
    frame.preview_requests = preview_requests
        .get_mut()
        .map(|mut requests| std::mem::take(&mut requests.pending))
        .unwrap_or_default();
    frame.instances.clear();
    let world_cameras: Vec<(Entity, CameraComponent, TransformComponent)> = cameras
        .iter_mut()
        .map(|(entity, transform, camera)| (entity, *camera, *transform))
        .collect();
    // A binding for the primary surface chooses its camera outright. Without
    // one, the camera is picked from every enabled camera in the world;
    // [`pick_camera`] says which one wins when several are enabled.
    let surface_cameras_resource = surface_cameras.get();
    let primary_binding = surface_cameras_resource.and_then(|surface_cameras| {
        surface_cameras
            .bindings
            .get(&pill_renderer_api::SurfaceId::PRIMARY)
    });
    let selected_camera = match primary_binding {
        Some(binding) => {
            pill_renderer_api::resolve_camera_binding(binding, world_cameras.iter().copied())
                .map(|view| (view.camera, view.transform))
        }
        None => pick_camera(
            world_cameras
                .iter()
                .map(|(entity, camera, transform)| (entity.id(), *camera, *transform)),
        ),
    };
    // One entry per surface that has a camera: the primary surface takes the
    // camera chosen above, and every other surface takes the camera its binding
    // resolves to. A surface with no camera is left out, and draws nothing.
    frame.surface_views.clear();
    if let Some((camera, transform)) = selected_camera {
        frame.surface_views.insert(
            pill_renderer_api::SurfaceId::PRIMARY,
            pill_renderer_api::SurfaceView { camera, transform },
        );
    }
    if let Some(surface_cameras) = surface_cameras_resource {
        for (surface, binding) in &surface_cameras.bindings {
            if *surface == pill_renderer_api::SurfaceId::PRIMARY {
                continue;
            }
            if let Some(view) =
                pill_renderer_api::resolve_camera_binding(binding, world_cameras.iter().copied())
            {
                frame.surface_views.insert(*surface, view);
            }
        }
    }
    // The scene grid's spacing, when a frontend turned the grid on and at least
    // one surface is bound to a view camera. Those view-bound surfaces are the
    // only ones that draw the grid, and they are the editor views listed below;
    // a surface drawn through a world camera never does.
    frame.grid_cell_size = surface_cameras_resource.and_then(pill_renderer_api::frame_grid);
    // The surfaces the editor draws its own gizmo handles and selection
    // outlines on. This is the view-bound set whether the grid is on or off, so
    // switching the grid off never moves the gizmo onto the game's surface.
    frame.editor_views = surface_cameras_resource
        .map(pill_renderer_api::editor_view_surfaces)
        .unwrap_or_default();

    // The frame's UI comes from the world's only producer. When the slot
    // exists, the project installed its own egui UI, and whatever it published
    // is what the frame carries, even when it drew nothing: a game that owns
    // the UI must not have an empty frame read as "no UI". A world that never
    // installed a slot carries no UI. The log names the producer once, when it
    // changes.
    match ui_slot.get_mut() {
        Some(mut slot) => {
            // A rebuilt painter starts with an empty texture map, while the
            // producer, a shared library that outlives this image, has already
            // sent every image once. Ask for all of them again the first time
            // this image finds a slot. The flag above starts fresh with the
            // module, so this happens once per rebuild.
            if !UI_TEXTURES_REQUESTED.swap(true, Ordering::Relaxed) {
                slot.request_texture_resend();
            }
            frame.ui = slot.take();
            note_ui_producer(PRODUCER_GAME, "game");
        }
        None => {
            frame.ui.clear();
            note_ui_producer(PRODUCER_NONE, "none");
        }
    }

    // The mesh and material pair resolved last, and the draw order it gave
    // (`None` when either handle was dead). Many drawables share one mesh and
    // one material, and those usually sit next to each other in an archetype,
    // so remembering one pair skips nearly every asset lookup. Those lookups
    // dominate this system's time once thousands of draws share one pair.
    let mut last_resolved_pair: Option<(Handle<Mesh>, Handle<Material>, Option<u8>)> = None;
    for (entity, transform, renderer) in drawables.iter_mut() {
        let rendering_order = match last_resolved_pair {
            Some((mesh, material, rendering_order))
                if mesh == renderer.mesh && material == renderer.material =>
            {
                rendering_order
            }
            _ => {
                // A handle with no live asset behind it is dropped here, so a
                // dangling handle draws nothing instead of failing the frame.
                // The material is read, not just checked, because its draw
                // order travels with the instance. The renderer applies that
                // order when it builds the queue, so the instances left here
                // stay in traversal order.
                let rendering_order = assets
                    .get(renderer.material)
                    .filter(|_| assets.contains(renderer.mesh))
                    .map(|material| material.rendering_order);
                last_resolved_pair = Some((renderer.mesh, renderer.material, rendering_order));
                rendering_order
            }
        };
        let Some(rendering_order) = rendering_order else {
            continue;
        };
        frame.instances.push(RenderInstance {
            entity,
            transform: *transform,
            mesh: frame_asset_key(renderer.mesh),
            material: frame_asset_key(renderer.material),
            rendering_order,
        });
    }
    Ok(())
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // `PassKind`, `Shader`, `Texture`, `RenderPass`, `RenderingPipeline` and
    // `TextureType` only build the fixture assets and the expectations these
    // tests need; the code under test names none of them.
    use crate::{PassKind, RenderPass, RenderingPipeline, Shader, Texture, TextureType};
    use pill_engine::{AssetManager, Engine};

    /// The renderer reports exactly the three components its system queries,
    /// by their pinned shared names.
    #[test]
    fn the_consumed_components_are_the_queried_ones() {
        assert_eq!(
            super::consumed_component_names(),
            [
                "pill_master_renderer::component::TransformComponent",
                "pill_master_renderer::component::CameraComponent",
                "pill_master_renderer::component::MeshRendererComponent",
            ]
        );
    }

    #[test]
    fn a_game_with_no_pipeline_runs_the_built_in_pass() {
        let assets = AssetManager::new();

        let passes = resolve_passes(&assets, Some(&RenderingManager::new()));

        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0].name, "builtin.default");
        assert_eq!(passes[0].shader, None);
        assert_eq!(passes[0].settings.kind, PassKind::Geometry);
        assert_eq!(passes[0].settings.target, PassTarget::Surface);
    }

    #[test]
    fn a_chain_entry_switched_off_is_left_out() {
        let mut assets = AssetManager::new();
        let kept = assets
            .add_named("kept", RenderPass::new("kept"))
            .expect("a free name");
        let left_out = assets
            .add_named("left_out", RenderPass::new("left_out"))
            .expect("a free name");
        let mut pipeline = RenderingPipeline::new().with_pass(kept);
        pipeline.passes.push(crate::PipelinePass {
            pass: left_out,
            enabled: false,
        });
        let pipeline = assets.add_named("chain", pipeline).expect("a free name");
        let mut manager = RenderingManager::new();
        manager.set_pipeline(pipeline);

        let passes = resolve_passes(&assets, Some(&manager));

        assert_eq!(
            passes
                .iter()
                .map(|pass| pass.name.as_str())
                .collect::<Vec<_>>(),
            ["kept"],
            "the entry this chain switched off is left out, and the pass asset itself stays on"
        );
    }

    #[test]
    fn the_chain_runs_in_order_and_leaves_out_disabled_passes() {
        let mut assets = AssetManager::new();
        let second = assets
            .add_named("second", RenderPass::new("second").with_order(1))
            .expect("a free name");
        let first = assets
            .add_named("first", RenderPass::new("first"))
            .expect("a free name");
        let off = assets
            .add_named("off", RenderPass::new("off").with_enabled(false))
            .expect("a free name");
        let pipeline = assets
            .add_named(
                "chain",
                RenderingPipeline::new()
                    .with_pass(second)
                    .with_pass(first)
                    .with_pass(off),
            )
            .expect("a free name");
        let mut manager = RenderingManager::new();
        manager.set_pipeline(pipeline);

        let passes = resolve_passes(&assets, Some(&manager));

        assert_eq!(
            passes
                .iter()
                .map(|pass| pass.name.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"],
            "the disabled pass is left out and order beats pipeline order"
        );
    }

    #[test]
    fn a_pass_that_names_a_shader_carries_its_key() {
        let mut assets = AssetManager::new();
        let shader = assets
            .add_named(
                "pbr",
                Shader::new("pbr")
                    .with_wgsl("vertex", "fragment")
                    .with_engine_parameters(true)
                    .with_camera_parameters(true)
                    .build()
                    .expect("the stages are in memory"),
            )
            .expect("a free name");
        let pass = assets
            .add_named("opaque", RenderPass::new("opaque").with_shader(shader))
            .expect("a free name");
        let pipeline = assets
            .add_named("chain", RenderingPipeline::new().with_pass(pass))
            .expect("a free name");
        let mut manager = RenderingManager::new();
        manager.set_pipeline(pipeline);

        let passes = resolve_passes(&assets, Some(&manager));

        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0].shader, Some(frame_asset_key(shader)));
    }

    #[test]
    fn a_pass_that_binds_a_texture_carries_its_key() {
        let mut assets = AssetManager::new();
        let texture = assets
            .add_named(
                "grain",
                Texture::from_rgba("grain", TextureType::Color, vec![0, 0, 0, 255], 1, 1)
                    .expect("a 1x1 RGBA tile"),
            )
            .expect("a free name");
        let pass = assets
            .add_named(
                "lens",
                RenderPass::new("lens").with_texture("grain", texture),
            )
            .expect("a free name");
        let pipeline = assets
            .add_named("chain", RenderingPipeline::new().with_pass(pass))
            .expect("a free name");
        let mut manager = RenderingManager::new();
        manager.set_pipeline(pipeline);

        let passes = resolve_passes(&assets, Some(&manager));

        assert_eq!(passes.len(), 1);
        assert_eq!(
            passes[0].textures.get("grain"),
            Some(&frame_asset_key(texture))
        );
    }

    /// A pass that names a material takes that material's shader, parameters
    /// and textures. Values the pass sets itself win over the material's.
    #[test]
    fn a_pass_that_names_a_material_draws_with_it() {
        use crate::{Material, MaterialParameter};

        let mut assets = AssetManager::new();
        let shader = assets
            .add_named(
                "sky_shader",
                Shader::new("sky")
                    .with_wgsl("vertex", "fragment")
                    .build()
                    .expect("the stages are in memory"),
            )
            .expect("a free name");
        let texture = assets
            .add_named(
                "sky_texture",
                Texture::from_rgba("sky", TextureType::Color, vec![0, 0, 0, 255], 1, 1)
                    .expect("a 1x1 RGBA tile"),
            )
            .expect("a free name");
        let material = assets
            .add_named(
                "sky_material",
                Material::builder("sky")
                    .shader(&shader)
                    .texture("sky", &texture)
                    .scalar_parameter("skybox_exposure", 2.0)
                    .scalar_parameter("skybox_rotation", 10.0)
                    .build(),
            )
            .expect("a free name");
        let pass = assets
            .add_named(
                "sky",
                RenderPass::new("sky")
                    .with_kind(PassKind::Skybox)
                    .with_material(material)
                    .with_parameter("skybox_rotation", MaterialParameter::Scalar(90.0)),
            )
            .expect("a free name");
        let pipeline = assets
            .add_named("chain", RenderingPipeline::new().with_pass(pass))
            .expect("a free name");
        let mut manager = RenderingManager::new();
        manager.set_pipeline(pipeline);

        let passes = resolve_passes(&assets, Some(&manager));

        assert_eq!(passes[0].shader, Some(frame_asset_key(shader)));
        assert_eq!(
            passes[0].textures.get("sky"),
            Some(&frame_asset_key(texture))
        );
        assert_eq!(
            passes[0].parameters.get("skybox_exposure"),
            Some(&MaterialParameter::Scalar(2.0))
        );
        assert_eq!(
            passes[0].parameters.get("skybox_rotation"),
            Some(&MaterialParameter::Scalar(90.0)),
            "the pass's own value wins"
        );
    }

    fn camera(priority: i32, enabled: bool, vertical_fov: f32) -> CameraComponent {
        CameraComponent {
            priority,
            enabled,
            vertical_fov,
            ..CameraComponent::default()
        }
    }

    #[test]
    fn the_highest_priority_camera_is_selected() {
        let selected = pick_camera([
            (1, camera(0, true, 60.0), TransformComponent::default()),
            (2, camera(5, true, 42.0), TransformComponent::default()),
        ]);

        assert_eq!(selected.map(|(camera, _)| camera.vertical_fov), Some(42.0));
    }

    #[test]
    fn equal_priorities_break_ties_by_the_lowest_entity_id() {
        let selected = pick_camera([
            (7, camera(0, true, 60.0), TransformComponent::default()),
            (3, camera(0, true, 42.0), TransformComponent::default()),
            (5, camera(0, true, 20.0), TransformComponent::default()),
        ]);

        assert_eq!(selected.map(|(camera, _)| camera.vertical_fov), Some(42.0));
    }

    #[test]
    fn a_disabled_camera_is_never_selected() {
        let selected = pick_camera([
            (1, camera(9, false, 42.0), TransformComponent::default()),
            (2, camera(0, true, 60.0), TransformComponent::default()),
        ]);

        assert_eq!(selected.map(|(camera, _)| camera.vertical_fov), Some(60.0));
        assert!(
            pick_camera([(1, camera(9, false, 42.0), TransformComponent::default())]).is_none()
        );
    }

    #[test]
    fn a_pipeline_handle_that_no_longer_resolves_falls_back_to_the_built_in_pass() {
        let mut assets = AssetManager::new();
        let pass = assets
            .add_named("only", RenderPass::new("only"))
            .expect("a free name");
        let pipeline = assets
            .add_named("chain", RenderingPipeline::new().with_pass(pass))
            .expect("a free name");
        let mut manager = RenderingManager::new();
        manager.set_pipeline(pipeline);
        assets.remove(pipeline);

        let passes = resolve_passes(&assets, Some(&manager));

        assert_eq!(passes.len(), 1);
        assert_eq!(passes[0].name, "builtin.default");
    }

    #[test]
    fn a_pass_handle_that_no_longer_resolves_is_left_out_of_the_chain() {
        let mut assets = AssetManager::new();
        let pass = assets
            .add_named("gone", RenderPass::new("gone"))
            .expect("a free name");
        let pipeline = assets
            .add_named("chain", RenderingPipeline::new().with_pass(pass))
            .expect("a free name");
        let mut manager = RenderingManager::new();
        manager.set_pipeline(pipeline);
        assets.remove(pass);

        let passes = resolve_passes(&assets, Some(&manager));

        assert!(
            passes.is_empty(),
            "a chain whose only pass is gone resolves to nothing"
        );
    }

    /// Builds an engine the rendering system can run in: the frame and the
    /// asset store are inserted as resources, and the artifact entry point
    /// registers the declared `rendering` system.
    fn renderable_engine() -> Engine {
        let mut engine = Engine::new();
        engine.world_mut().insert_resource(RenderFrame::default());
        engine.world_mut().insert_resource(AssetManager::new());
        // The system is declared with `#[system]`, so it reaches the engine
        // through the artifact entry point. `register` is the half a reload
        // repeats, and it declares no system of its own.
        let status = pill_engine::component_registry::run_artifact_registration(
            &mut engine,
            crate::register,
        );
        assert_eq!(status, 0, "the renderer's registration reports success");
        engine
    }

    /// Builds a frame with one filled quad, the way a game's UI producer
    /// publishes it. It is made from `pill_ui_frame`'s draw vocabulary, which is
    /// the plain data the slot carries.
    fn drawn_frame() -> pill_renderer_api::frame::RenderFrame {
        let mut frame = RenderFrame::default();
        let mut list = pill_ui_frame::DrawList::new();
        list.push_quad(
            pill_ui_frame::TextureId(1),
            pill_ui_frame::Rect::from_min_size(
                pill_core::math::Vector2f::ZERO,
                pill_core::math::Vector2f::new(2.0, 2.0),
            ),
            pill_core::math::Vector2f::ZERO,
            pill_core::math::Vector2f::new(1.0, 1.0),
            pill_ui_frame::Color::WHITE,
        );
        frame.ui.draws.push(pill_ui_frame::UiDraw {
            surface: 0,
            scale_factor: 1.0,
            list,
        });
        frame
    }

    #[test]
    fn a_published_frame_reaches_the_renderer() {
        let mut engine = renderable_engine();
        engine
            .world_mut()
            .insert_resource(pill_ui_frame::UiFrameSlot::new());
        {
            let slot = engine
                .world_mut()
                .get_resource_mut::<pill_ui_frame::UiFrameSlot>()
                .expect("inserted above");
            slot.publish(drawn_frame().ui);
        }
        engine.process_frame().expect("the frame runs");

        let frame = engine
            .world()
            .get_resource::<RenderFrame>()
            .expect("inserted above");
        assert_eq!(
            frame.ui.draws.len(),
            1,
            "the published frame did not reach the renderer"
        );
        let slot = engine
            .world()
            .get_resource::<pill_ui_frame::UiFrameSlot>()
            .expect("inserted above");
        assert!(slot.is_empty(), "the frame was copied rather than drained");
    }

    #[test]
    fn a_slot_that_drew_nothing_still_owns_the_frame() {
        let mut engine = renderable_engine();
        engine
            .world_mut()
            .insert_resource(pill_ui_frame::UiFrameSlot::new());
        engine.process_frame().expect("the frame runs");

        let frame = engine
            .world()
            .get_resource::<RenderFrame>()
            .expect("inserted above");
        assert!(
            frame.ui.is_empty(),
            "a game that drew nothing publishes an empty frame, not a fallback one"
        );
    }

    #[test]
    fn a_world_without_a_slot_carries_no_ui() {
        let mut engine = renderable_engine();
        engine.process_frame().expect("the frame runs");

        let frame = engine
            .world()
            .get_resource::<RenderFrame>()
            .expect("inserted above");
        assert!(
            frame.ui.draws.is_empty(),
            "a world that never installed a slot has no UI to draw"
        );
        assert!(
            frame.ui.textures.textures.is_empty(),
            "and no textures either"
        );
    }
}
