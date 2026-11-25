//! Building the frame a renderer draws, from the world it was handed.
//!
//! # Responsibilities
//!
//! - Fill the frame from the values a renderer gathered this update
//!   ([`update_frame`]): the pass chain, the camera each surface draws through,
//!   the instances to draw, the clocks, the debug geometry and the UI.
//! - Turn the game's pipeline into plain passes ([`resolve_passes`]). Entries
//!   that are switched off or point at dead assets are left out, and a chain
//!   whose surface pass breaks the renderer's contract is reported in the log.
//! - Collect the drawables the world offers ([`collect_instances`]) and pick
//!   the camera the primary surface draws through ([`pick_camera`]).
//!
//! # Design
//!
//! The frame itself ([`RenderFrame`], [`RenderInstance`] and [`ResolvedPass`])
//! is plain data from `pill_renderer_api`. Filling it is shared rendering
//! behaviour, so it lives here, where every renderer gets the same frame from
//! the same world.
//!
//! A renderer's system reads the world's resources and queries and hands the
//! results over as plain values ([`FrameInput`]) plus two iterators: the
//! cameras and the drawables. Nothing here reads a world, so the same code runs
//! inside a module image and inside a statically linked build.
//!
//! Filling the frame also makes choices the world data does not spell out:
//! which of several cameras to use, what each pipeline pass turns into, and the
//! draw order each instance carries.

// Standard library
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};

// External crates
use pill_core::telemetry::telemetry_target;
use pill_core::{info, warn};
use pill_engine::{AssetManager, Component, Entity, Handle};
use pill_renderer_api::editor::{PickRequest, PreviewRequest, SelectionOutlines};
use pill_renderer_api::frame::{PassTarget, RenderFrame, RenderInstance, ResolvedPass};
use pill_renderer_api::{AppendPass, AssetKey, ChainSource, SurfaceCameras};
use pill_rendering::assets::{asset_key, Material, Mesh, RenderingPipeline};
use pill_rendering::components::{CameraComponent, MeshRendererComponent, TransformComponent};
use pill_rendering::AppendPipeline;

// =============================================================================
// Constants
// =============================================================================

/// Which producer filled `RenderFrame.ui` on the last frame, so the log states
/// a change once instead of on every frame.
///
/// This is module state rather than a frame field: the frame is plain data
/// shared through the renderer contract, while the producer is a fact about the
/// world this code last filled a frame from. A reload gives the image fresh
/// statics, so the next frame logs which producer is feeding it again.
static UI_PRODUCER: AtomicU8 = AtomicU8::new(PRODUCER_UNSET);

/// No frame has been filled since this image started.
const PRODUCER_UNSET: u8 = 0;
/// The game's own UI.
const PRODUCER_GAME: u8 = 1;
/// No producer: the frame carries no UI.
const PRODUCER_NONE: u8 = 2;

// =============================================================================
// Types
// =============================================================================

/// Everything one frame's update reads from the world, gathered by the
/// renderer's system.
///
/// Each field is a plain value the system took out of its resource, so this
/// struct borrows nothing that has to outlive the call. A world without a
/// resource hands over `None` or the empty value, and the frame says what that
/// means: no camera binding, no debug geometry, no outlines, no UI.
pub struct FrameInput<'a> {
    /// The store the instances and the chain are resolved from.
    pub assets: &'a AssetManager,
    /// The pipeline the game selected, or `None` when it selected none.
    pub pipeline: Option<Handle<RenderingPipeline>>,
    /// The pipelines owners appended after every surface's chain, in the order
    /// they run.
    pub append_pipelines: &'a [AppendPipeline],
    /// This frame's duration in seconds, when the world carries a clock.
    pub delta_seconds: Option<f32>,
    /// What systems debug-drew for this frame, already aged by this frame's
    /// step.
    pub debug: pill_renderer_api::DebugFrame,
    /// The entities an editor asked to outline.
    pub outlines: SelectionOutlines,
    /// The picks asked for since the last frame.
    pub pick_requests: Vec<PickRequest>,
    /// The previews asked for since the last frame.
    pub preview_requests: Vec<PreviewRequest>,
    /// The cameras bound to surfaces, when a frontend binds any.
    pub surface_cameras: Option<&'a SurfaceCameras>,
    /// The UI a producer published this update. `Some` means a producer owns
    /// the frame's UI, even when it drew nothing; `None` means the world has
    /// none.
    pub ui: Option<pill_ui_frame::UiFrame>,
}

// =============================================================================
// Free Functions
// =============================================================================

/// Builds the key the frame uses for `handle`.
///
/// The asset layer packs the handle into a number; the renderer contract
/// carries that value as an [`AssetKey`].
pub fn frame_asset_key<T: pill_engine::Asset>(handle: Handle<T>) -> AssetKey {
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
pub fn resolve_passes(
    assets: &AssetManager,
    pipeline: Option<Handle<RenderingPipeline>>,
) -> Vec<ResolvedPass> {
    let Some(pipeline) = pipeline.and_then(|handle| assets.get(handle)) else {
        // The handle exists but no longer resolves: the pipeline asset was
        // removed after the game selected it. That earns a warning, because
        // the game asked for a chain and lost it. A game that never selected a
        // pipeline is not news. Both fall back to the built-in pass.
        if pipeline.is_some() {
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

/// Returns the passes the frame appends after each surface's chain.
///
/// Every owner's pipeline is resolved the way the chain is, and each of its
/// passes carries the owner's surface choice. The list keeps the order the
/// manager hands over - by `order`, then by owner - so the passes run in the
/// order their owners asked for.
///
/// An owner whose pipeline no longer resolves appends nothing: the frame says
/// so in the log instead of running the built-in pass a second time, which is
/// what the chain fallback would do.
pub fn resolve_append_passes(
    assets: &AssetManager,
    append_pipelines: &[AppendPipeline],
) -> Vec<AppendPass> {
    let mut passes = Vec::new();
    for append in append_pipelines {
        if assets.get(append.pipeline).is_none() {
            warn!(
                target: telemetry_target::RENDERING,
                owner = append.owner.as_str(),
                "an appended pipeline no longer resolves and is left out"
            );
            continue;
        }
        for pass in resolve_passes(assets, Some(append.pipeline)) {
            passes.push(AppendPass {
                pass,
                surfaces: append.surfaces,
            });
        }
    }
    passes
}

/// Picks the camera the primary surface draws through, out of every enabled
/// camera in the world.///
/// The highest `priority` wins. Equal priorities go to the lowest entity id, so
/// the choice stays the same from frame to frame while the same cameras exist,
/// whatever order the query walks them in. `None` means no camera is enabled;
/// the primary surface is then drawn only if a camera is bound to it.
pub fn pick_camera(
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

/// Pushes one instance into `instances` for every drawable the world offers.
///
/// A drawable whose material or mesh handle has no live asset behind it is left
/// out, so a dangling handle draws nothing instead of failing the frame. Each
/// instance carries the material's rendering order, which the renderer applies
/// when it builds its draw queue.
///
/// The mesh and material pair resolved last is remembered: many drawables share
/// one mesh and one material, and those usually sit next to each other in an
/// archetype, so one pair skips nearly every asset lookup. Those lookups
/// dominate this function's time once thousands of draws share one pair.
pub fn collect_instances(
    assets: &AssetManager,
    drawables: impl IntoIterator<Item = (Entity, TransformComponent, MeshRendererComponent)>,
    instances: &mut Vec<RenderInstance>,
) {
    let mut last_resolved_pair: Option<(Handle<Mesh>, Handle<Material>, Option<u8>)> = None;
    for (entity, transform, renderer) in drawables {
        let rendering_order = match last_resolved_pair {
            Some((mesh, material, rendering_order))
                if mesh == renderer.mesh && material == renderer.material =>
            {
                rendering_order
            }
            _ => {
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
        instances.push(RenderInstance {
            entity,
            transform,
            mesh: frame_asset_key(renderer.mesh),
            material: frame_asset_key(renderer.material),
            rendering_order,
        });
    }
}

/// Returns the shared names of the components a renderer's frame system reads.
///
/// `RenderCapabilities::consumed_components` carries them. The host compares
/// that list with the render components a project uses and warns about any the
/// renderer ignores. The names come from the component types themselves rather
/// than from string literals, so they cannot drift from the types.
pub fn consumed_component_names() -> Vec<String> {
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

/// Fills `frame` from one update's worth of world values.
///
/// The pass chain is resolved again only when the asset revision or the
/// pipeline's key moves, so most frames reuse the previous result. Everything
/// else is rebuilt each frame: the clock, the debug geometry, the surfaces'
/// cameras, the UI and the instances.
///
/// `cameras` yields every camera in the world as `(entity id, camera,
/// transform)`, and `drawables` every drawable as `(entity, transform, mesh
/// renderer)`. A world with no enabled camera still gets a frame, but the
/// primary surface has no view unless a camera is bound to it, and a renderer
/// skips a frame that has no views at all.
pub fn update_frame(
    frame: &mut RenderFrame,
    input: FrameInput<'_>,
    cameras: impl IntoIterator<Item = (Entity, CameraComponent, TransformComponent)>,
    drawables: impl IntoIterator<Item = (Entity, TransformComponent, MeshRendererComponent)>,
) {
    let assets = input.assets;

    // Read the pipeline every frame, because a game can set one, or switch a
    // pass inside one, at any point between two frames. Resolve the chain again
    // only when the pipeline key, the appended pipelines or the asset revision
    // moved: resolution clones every pass's strings and maps. The generation it
    // bumps tells the renderer whether the chain it built is still this chain.
    let chain_source = ChainSource {
        revision: assets.revision(),
        pipeline: input.pipeline.map(frame_asset_key),
        append_pipelines: input
            .append_pipelines
            .iter()
            .map(|append| frame_asset_key(append.pipeline))
            .collect(),
    };
    if frame.chain_source.as_ref() != Some(&chain_source) {
        frame.passes = resolve_passes(assets, input.pipeline);
        frame.append_passes = resolve_append_passes(assets, input.append_pipelines);
        frame.chain_source = Some(chain_source);
        frame.chain_generation = frame.chain_generation.wrapping_add(1);
    }

    frame.sequence = frame.sequence.wrapping_add(1);
    if let Some(delta_seconds) = input.delta_seconds {
        // Add the step to the frame's own clock instead of reading the world's
        // clock: a shader is handed this value, so it should grow by the same
        // step the game's update saw.
        frame.delta_seconds = delta_seconds;
        frame.seconds += frame.delta_seconds;
    }
    frame.debug = input.debug;
    frame.outlines = input.outlines;
    frame.pick_requests = input.pick_requests;
    frame.preview_requests = input.preview_requests;

    frame.instances.clear();
    collect_instances(assets, drawables, &mut frame.instances);

    let world_cameras: Vec<(Entity, CameraComponent, TransformComponent)> =
        cameras.into_iter().collect();
    // A binding for the primary surface chooses its camera outright. Without
    // one, the camera is picked from every enabled camera in the world;
    // [`pick_camera`] says which one wins when several are enabled.
    let primary_binding = input.surface_cameras.and_then(|surface_cameras| {
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
    if let Some(surface_cameras) = input.surface_cameras {
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
    frame.grid_cell_size = input
        .surface_cameras
        .and_then(pill_renderer_api::editor::frame_grid);
    // The surfaces the editor draws its own gizmo handles and selection
    // outlines on. This is the view-bound set whether the grid is on or off, so
    // switching the grid off never moves the gizmo onto the game's surface.
    frame.editor_views = input
        .surface_cameras
        .map(pill_renderer_api::editor::editor_view_surfaces)
        .unwrap_or_default();

    // The frame's UI comes from the world's only producer. When one published
    // this update, whatever it published is what the frame carries, even when
    // it drew nothing: a game that owns the UI must not have an empty frame
    // read as "no UI". A world with no producer carries no UI. The log names
    // the producer once, when it changes.
    match input.ui {
        Some(ui) => {
            frame.ui = ui;
            note_ui_producer(PRODUCER_GAME, "game");
        }
        None => {
            frame.ui.clear();
            note_ui_producer(PRODUCER_NONE, "none");
        }
    }
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use pill_renderer_api::frame::PassKind;
    use pill_rendering::assets::{
        MaterialParameter, Mesh, PipelinePass, RenderPass, Shader, Texture, TextureType,
    };

    // =========================================================================
    // The chain
    // =========================================================================

    #[test]
    fn a_game_with_no_pipeline_runs_the_built_in_pass() {
        let assets = AssetManager::new();

        let passes = resolve_passes(&assets, None);

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
        pipeline.passes.push(PipelinePass {
            pass: left_out,
            enabled: false,
        });
        let pipeline = assets.add_named("chain", pipeline).expect("a free name");

        let passes = resolve_passes(&assets, Some(pipeline));

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

        let passes = resolve_passes(&assets, Some(pipeline));

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

        let passes = resolve_passes(&assets, Some(pipeline));

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

        let passes = resolve_passes(&assets, Some(pipeline));

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

        let passes = resolve_passes(&assets, Some(pipeline));

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

    #[test]
    fn a_pipeline_handle_that_no_longer_resolves_falls_back_to_the_built_in_pass() {
        let mut assets = AssetManager::new();
        let pass = assets
            .add_named("only", RenderPass::new("only"))
            .expect("a free name");
        let pipeline = assets
            .add_named("chain", RenderingPipeline::new().with_pass(pass))
            .expect("a free name");
        assets.remove(pipeline);

        let passes = resolve_passes(&assets, Some(pipeline));

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
        assets.remove(pass);

        let passes = resolve_passes(&assets, Some(pipeline));

        assert!(
            passes.is_empty(),
            "a chain whose only pass is gone resolves to nothing"
        );
    }

    // =========================================================================
    // The camera
    // =========================================================================

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

    // =========================================================================
    // The instances
    // =========================================================================

    /// A world with one mesh, one material and one drawable, ready to collect.
    fn drawable_world(rendering_order: u8) -> (AssetManager, Entity, MeshRendererComponent) {
        let mut assets = AssetManager::new();
        let mesh = assets
            .add_named("triangle", Mesh::triangle())
            .expect("a free name");
        let material = assets
            .add_named(
                "drawn",
                Material::builder("drawn")
                    .rendering_order(rendering_order)
                    .build(),
            )
            .expect("a free name");
        let entity = pill_engine::World::new().reserve_entity();
        (assets, entity, MeshRendererComponent { mesh, material })
    }

    #[test]
    fn a_drawable_becomes_one_instance_at_its_materials_order() {
        let (assets, entity, renderer) = drawable_world(3);
        let transform = TransformComponent {
            translation: [1.0, 2.0, 3.0],
            ..TransformComponent::default()
        };
        let mut instances = Vec::new();

        collect_instances(&assets, [(entity, transform, renderer)], &mut instances);

        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].entity, entity);
        assert_eq!(instances[0].transform.translation, [1.0, 2.0, 3.0]);
        assert_eq!(instances[0].rendering_order, 3);
        assert_eq!(instances[0].mesh, frame_asset_key(renderer.mesh));
        assert_eq!(instances[0].material, frame_asset_key(renderer.material));
    }

    /// A handle with no live asset behind it draws nothing: a removed mesh or
    /// material must not fail the frame.
    #[test]
    fn a_drawable_with_a_dead_handle_is_left_out() {
        let (mut assets, entity, renderer) = drawable_world(0);
        assets.remove(renderer.mesh);
        let mut instances = Vec::new();

        collect_instances(
            &assets,
            [(entity, TransformComponent::default(), renderer)],
            &mut instances,
        );

        assert!(instances.is_empty());
    }

    /// Drawables that share a mesh and a material read that pair once, and each
    /// one still becomes its own instance.
    #[test]
    fn drawables_sharing_a_pair_each_become_an_instance() {
        let (assets, entity, renderer) = drawable_world(0);
        let mut instances = Vec::new();

        collect_instances(
            &assets,
            [
                (entity, TransformComponent::default(), renderer),
                (entity, TransformComponent::default(), renderer),
                (entity, TransformComponent::default(), renderer),
            ],
            &mut instances,
        );

        assert_eq!(instances.len(), 3);
    }

    // =========================================================================
    // Filling the frame
    // =========================================================================

    #[test]
    fn the_clock_advances_by_the_step_the_world_reported() {
        let assets = AssetManager::new();
        let mut frame = RenderFrame::default();

        update_frame(
            &mut frame,
            FrameInput {
                assets: &assets,
                pipeline: None,
                append_pipelines: &[],
                delta_seconds: Some(0.25),
                debug: Default::default(),
                outlines: Default::default(),
                pick_requests: Vec::new(),
                preview_requests: Vec::new(),
                surface_cameras: None,
                ui: None,
            },
            [],
            [],
        );

        assert_eq!(frame.delta_seconds, 0.25);
        assert_eq!(frame.seconds, 0.25);
        assert_eq!(frame.sequence, 1);

        update_frame(
            &mut frame,
            FrameInput {
                assets: &assets,
                pipeline: None,
                append_pipelines: &[],
                delta_seconds: Some(0.5),
                debug: Default::default(),
                outlines: Default::default(),
                pick_requests: Vec::new(),
                preview_requests: Vec::new(),
                surface_cameras: None,
                ui: None,
            },
            [],
            [],
        );

        assert_eq!(frame.delta_seconds, 0.5);
        assert_eq!(frame.seconds, 0.75);
        assert_eq!(frame.sequence, 2);
    }

    /// The chain is resolved once and reused until the pipeline or the store
    /// moves, which is what keeps a per-frame cost off the chain.
    #[test]
    fn the_chain_is_resolved_once_until_the_store_or_pipeline_moves() {
        let assets = AssetManager::new();
        let mut frame = RenderFrame::default();

        fn input(assets: &AssetManager) -> FrameInput<'_> {
            FrameInput {
                assets,
                pipeline: None,
                append_pipelines: &[],
                delta_seconds: None,
                debug: Default::default(),
                outlines: Default::default(),
                pick_requests: Vec::new(),
                preview_requests: Vec::new(),
                surface_cameras: None,
                ui: None,
            }
        }

        update_frame(&mut frame, input(&assets), [], []);
        let generation = frame.chain_generation;

        update_frame(&mut frame, input(&assets), [], []);

        assert_eq!(
            frame.chain_generation, generation,
            "the same store and pipeline reuse the chain"
        );
    }

    #[test]
    fn the_primary_surface_draws_through_the_chosen_camera() {
        let assets = AssetManager::new();
        let mut frame = RenderFrame::default();
        let transform = TransformComponent {
            translation: [4.0, 0.0, 0.0],
            ..TransformComponent::default()
        };
        let entity = pill_engine::World::new().reserve_entity();

        update_frame(
            &mut frame,
            FrameInput {
                assets: &assets,
                pipeline: None,
                append_pipelines: &[],
                delta_seconds: None,
                debug: Default::default(),
                outlines: Default::default(),
                pick_requests: Vec::new(),
                preview_requests: Vec::new(),
                surface_cameras: None,
                ui: None,
            },
            [(entity, camera(0, true, 42.0), transform)],
            [],
        );

        let view = frame
            .surface_views
            .get(&pill_renderer_api::SurfaceId::PRIMARY)
            .expect("the primary surface has a camera");
        assert_eq!(view.camera.vertical_fov, 42.0);
        assert_eq!(view.transform.translation, [4.0, 0.0, 0.0]);
    }

    #[test]
    fn a_world_with_no_camera_draws_no_surface_view() {
        let assets = AssetManager::new();
        let mut frame = RenderFrame::default();
        frame.surface_views.insert(
            pill_renderer_api::SurfaceId::PRIMARY,
            pill_renderer_api::SurfaceView {
                camera: camera(0, true, 42.0),
                transform: TransformComponent::default(),
            },
        );

        update_frame(
            &mut frame,
            FrameInput {
                assets: &assets,
                pipeline: None,
                append_pipelines: &[],
                delta_seconds: None,
                debug: Default::default(),
                outlines: Default::default(),
                pick_requests: Vec::new(),
                preview_requests: Vec::new(),
                surface_cameras: None,
                ui: None,
            },
            [],
            [],
        );

        assert!(
            frame.surface_views.is_empty(),
            "the previous frame's views do not survive"
        );
    }

    /// A producer that drew nothing still owns the frame's UI; a world without
    /// one carries none.
    #[test]
    fn the_ui_follows_the_producer_that_published_it() {
        let assets = AssetManager::new();
        let mut frame = RenderFrame::default();
        let mut ui = pill_ui_frame::UiFrame::default();
        ui.draws.push(pill_ui_frame::UiDraw {
            surface: 0,
            scale_factor: 1.0,
            list: pill_ui_frame::DrawList::new(),
        });

        fn input(assets: &AssetManager, ui: Option<pill_ui_frame::UiFrame>) -> FrameInput<'_> {
            FrameInput {
                assets,
                pipeline: None,
                append_pipelines: &[],
                delta_seconds: None,
                debug: Default::default(),
                outlines: Default::default(),
                pick_requests: Vec::new(),
                preview_requests: Vec::new(),
                surface_cameras: None,
                ui,
            }
        }

        update_frame(&mut frame, input(&assets, Some(ui)), [], []);
        assert_eq!(frame.ui.draws.len(), 1, "the published frame arrives");

        update_frame(&mut frame, input(&assets, None), [], []);
        assert!(
            frame.ui.draws.is_empty(),
            "a world without a producer carries no UI"
        );
    }
}
