//! Hot-reloadable bouncing-ball project implemented with ECS systems.
//!
//! # Responsibilities
//!
//! - Runs the ball physics: [`BALL_COUNT`] balls bouncing inside a fixed box,
//!   stepped by the frame delta the host clock reports.
//! - Drives the project's single [`Spline`]: its control points are the ball
//!   centres, and one [`SplineSample`] dot sits on the curve every
//!   [`SPLINE_SAMPLE_STEP`] of `t`.
//! - Carries the migration witnesses (`migration`): four persistable
//!   components whose values print once per generation, so a schema change can
//!   be proven to preserve them.
//! - Carries the layout witness (`layout_witness`): one resource that declares
//!   the file its layout is made of, and prints when it is inserted and dropped,
//!   so the keep-or-rebuild decision across a reload is observable.
//! - Carries the settings manager (`settings_probe`): a manager resource with a
//!   settings surface, so the host's per-manager settings pipeline has a subject
//!   whose applied values are visible in the log.
//! - Exposes the two entry points the host resolves in this artifact through
//!   the project-module ABI: `pill_module_register`, which re-registers the
//!   systems and resources on every generation, and `pill_module_init`, the
//!   one-time seeding of the scene.
//!
//! # Design
//!
//! The project is a plain `cdylib` that registers ECS components, resources,
//! and systems during `pill_module_register`, against the engine the host hands
//! it through [`EngineApi`]. The scene itself is seeded by `pill_module_init`,
//! which the host calls once, on the first load. A hot reload repeats the
//! registration and never the seeding, so the entities the first generation
//! created are the ones every later generation steps.
//!
//! Balls and spline are tied together by data rather than by an entity
//! relationship: `spline_path_system` copies the ball centres into the control
//! point array on every frame, which keeps every component in the scene plain,
//! serializable data. xx

// Standard library
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

// External crates
use pill_core::error;
use pill_core::math::Vector3f;
use pill_core::platform::Instant;
use pill_engine::*;
// `TransformComponent` is the shared placement row, registered by the renderer
// data crate this project depends on; `MeshRendererComponent` is that
// renderer's own idea of a thing to draw.
use pill_rendering::{
    CameraComponent, DirectionalLightComponent, Material, Mesh, MeshRendererComponent, RenderPass,
    RenderingManager, RenderingPipeline, Shader, TransformComponent,
};
use pill_spline::Spline;

mod follow_target;
mod layout_witness;
mod physics;
/// The scene's tuning constants. Public because hot patches compile separately
/// and import the same settings.
pub mod settings;
mod settings_probe;
mod simulation_time;
mod spline_path;
mod spline_probe;

// Keep the public hot-patch helper available at its original crate-root path.
pub use physics::simulate_ball;

// The follow-target system's patch is compiled as its own crate, so the
// component its body names has to be reachable from this crate's root: a patch
// naming `native::follow_target::FollowTarget` would be refused because the
// module is private.
pub use follow_target::FollowTarget;

use physics::ball_spawn_state;
use settings::*;
use spline_probe::SplineProbeState;

/// The pass the project's own frame draws with, as a file in `res`.
const PASS_PATH: &str = "dwdw.render_pass";

/// The frame this project runs, authored as a file: it names [`PASS_PATH`] by
/// guid in the order the pass runs.
const CHAIN_PATH: &str = "pipelines/main.pipeline";

/// A shader asset of the project's own, as a file in `res`.
///
/// The reload suites edit this source and require the host to re-cook it and
/// reimport the asset. Its `.hlsl.meta` names the renderer's shared vertex
/// stage, and its cooked `.wgsl` is committed the way the stage files beside a
/// build script are.
const PROBE_SHADER_PATH: &str = "shaders/reload_probe_fragment.hlsl";

/// Wall-clock delta between frames, shared by every system that steps on time.
///
/// Public so a hot patch can name it: a patched system's signature must be
/// expressible from outside the crate.
#[resource]
pub struct SimulationTime {
    pub last_frame: Instant,
    pub delta_seconds: f32,

    pub test: i32,
}

/// A rigid ball that bounces inside a fixed box, simulated each frame.
///
/// The host serializes this component across hot-reload generations, so the
/// struct layout is pinned with `#[repr(C)]` and every field stays
/// `Serialize` / `Deserialize` compatible.
#[component(persistable)]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PhysicsState {
    /// The simulation owns it; the Inspector leaves it out.
    #[pill(hide_in_inspector)]
    pub delta_time: f32,
    #[pill(header("Position"))]
    pub position_x: f32,
    pub position_y: f32,
    #[pill(header("Velocity"), space(10.0))]
    pub velocity_x: f32,
    pub velocity_y: f32,
    #[pill(tooltip("Radius of the ball in world units"), min(0.0))]
    pub radius: f32,
    pub active: bool,
}

/// One dot on the project's spline, drawn at the curve parameter `t`.
///
/// The dot carries a [`TransformComponent`] and a [`MeshRendererComponent`], so
/// it renders like any other mesh, and `spline_path_system` moves it along the
/// curve as the balls move. The host serializes this component across
/// hot-reload generations, so the layout is pinned with `#[repr(C)]`.
#[component(persistable)]
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SplineSample {
    /// Curve parameter this dot samples, running from 0.0 to 1.0.
    #[pill(
        header("Spline"),
        tooltip("Curve parameter this dot samples"),
        range(0.0, 1.0)
    )]
    pub t: f32,
}

/// Registers the renderer, the project's resources and its systems.
///
/// The host calls this half on every generation: a reload repeats what the
/// arriving image's code owns - the systems it steps, the resources their
/// access sets name, the components other artifacts query - and nothing else.
/// It must be idempotent, because a first load reaches it too, immediately
/// before [`init`].
///
/// Component and system registration happens from the artifact's own
/// declarations - the `PillComponent` derives and the `#[system]` attributes -
/// before this function runs.
///
/// Public so the generated `pill_module_register` export can name it.
pub fn register(engine: &mut Engine) -> u32 {
    // Register the renderer from the project artifact so its extraction system
    // uses the same concrete asset types as this DLL. RenderFrame is a shared
    // resource, so the host-side GPU backend reads the extracted data through
    // the engine's stable cross-artifact resource identity.
    pill_rendering::register(engine);
    // The entity-reference test subject: its layout is what gives the
    // Inspector a reference row to draw (W-0048 Stage 6).
    follow_target::register(engine.world_mut());
    engine.world_mut().insert_resource(SimulationTime {
        last_frame: Instant::now(),
        delta_seconds: FIXED_DELTA_TIME,
        test: 0,
    });
    // The keep-or-rebuild witness: a declared resource that keeps its value
    // across a reload unless the file its layout lives in changed.
    layout_witness::register(engine);
    // The settings manager: the host writes its `settings/` document from the
    // manager's own defaults and applies it, live when it is edited.
    settings_probe::register(engine);
    // Replaced, not reused. The state holds nothing that has to outlive a
    // generation - a report timestamp and an id string [`init`] derives - and
    // a resource the previous generation inserted is retired by the reload, so
    // keeping the reload's copy would leave the system pointing at a resource
    // the world no longer has.
    engine.world_mut().insert_resource(SplineProbeState::new());
    // A generation that runs against a world the first one filled - every
    // reload - must point the fresh state at the spline that is already there.
    refresh_probe_entity_id(engine);
    0
}

/// Seeds the render chain, the project's shader and the scene.
///
/// This is the one-time half: the host runs it on the artifact's first load,
/// after [`register`], and a reload never reaches it. Everything below may
/// therefore create entities without asking whether an earlier generation
/// already did.
///
/// Public so a statically linked build can call it directly. With a reloading
/// build, `#[pill_project]` exports it as `pill_module_init` for the host to
/// resolve in the loaded DLL.
///
/// # Errors
///
/// Returns 1 when the render chain or the project's shader will not load, or
/// when an entity cannot be created. The host reads any non-zero status as a
/// failed generation and rolls back to the previous library.
#[pill_project(register = register)]
pub fn init(engine: &mut Engine) -> u32 {
    // The one-time entry point announces itself, so a reload suite can count how
    // often a process reaches it. Anything above one means a reload ran the
    // seeding again, which is the behaviour this split exists to end.
    pill_core::info!(target: "project", "[project] init ran");
    // The frame this project runs is authored as files in `res`, not composed
    // here: `pipelines/main.pipeline` names `dwdw.render_pass` in the order it
    // runs, and the manager hands that chain to the renderer.
    let chain = {
        let assets = engine
            .world_mut()
            .get_resource_mut::<AssetManager>()
            .expect("Engine always owns an AssetManager");
        match load_chain(assets) {
            Ok(chain) => chain,
            Err(message) => {
                pill_engine::tracing::error!(
                    target: "project",
                    error = message.as_str(),
                    "[project] render chain failed to load"
                );
                return 1;
            }
        }
    };
    engine
        .world_mut()
        .get_resource_mut::<RenderingManager>()
        .expect("the renderer data crate registers its manager")
        .set_pipeline(chain);
    // The project's own shader: importing it is what puts the asset in the
    // store, and the renderer compiles every shader asset it finds there.
    if let Err(message) = import_probe_shader(engine.world_mut()) {
        pill_engine::tracing::error!(
            target: "project",
            error = message.as_str(),
            "[project] probe shader not imported"
        );
        return 1;
    }
    let (mesh, scene_material, ball_materials, sample_material) = {
        let assets = engine
            .world_mut()
            .get_resource_mut::<AssetManager>()
            .expect("Engine always owns an AssetManager");
        let mesh = assets
            .handle_by_name::<Mesh>("project.triangle")
            .unwrap_or_else(|| {
                assets
                    .add_named("project.triangle", Mesh::triangle())
                    .expect("the name is free after the lookup missed")
            });
        let mut material = |name: &str, color: [f32; 3], specularity: f32| {
            assets.handle_by_name::<Material>(name).unwrap_or_else(|| {
                assets
                    .add_named(
                        name,
                        Material::builder(name)
                            .color_parameter("tint", color)
                            .scalar_parameter("specularity", specularity)
                            .build(),
                    )
                    .expect("the name is free after the lookup missed")
            })
        };
        let scene_material = material("project.scene", [0.8, 0.8, 0.85], 0.5);
        let ball_materials: [Handle<Material>; BALL_COUNT] = std::array::from_fn(|index| {
            material(
                &format!("project.ball.{index}"),
                BALL_COLOR,
                index as f32 / BALL_COUNT as f32,
            )
        });
        let sample_material = material("project.spline", SAMPLE_DOT_COLOR, 0.2);
        (mesh, scene_material, ball_materials, sample_material)
    };
    let has_camera = Query::<&CameraComponent>::new(engine.world_mut())
        .iter_mut()
        .next()
        .is_some();
    if !has_camera {
        if engine
            .world_mut()
            .create_entity()
            .with(TransformComponent::default())
            .with(DirectionalLightComponent::default())
            .build()
            .is_err()
        {
            return 1;
        }
        let renderable = MeshRendererComponent::builder()
            .mesh(&mesh)
            .material(&scene_material)
            .build();
        if engine
            .world_mut()
            .create_entity()
            .with(renderable)
            .with(TransformComponent {
                translation: [0.0, 0.0, -1.5],
                scale: [1.5; 3],
                ..Default::default()
            })
            .build()
            .is_err()
        {
            return 1;
        }
    }
    let existing_camera = Query::<(&CameraComponent, Entity)>::new(engine.world_mut())
        .iter_mut()
        .next()
        .map(|(_, entity)| entity);
    let camera = match existing_camera {
        Some(camera) => camera,
        None => match engine
            .world_mut()
            .create_entity()
            .with(CameraComponent::default())
            .with(TransformComponent {
                translation: [0.0, 0.0, 9.0],
                ..Default::default()
            })
            .build()
        {
            Ok(camera) => camera,
            Err(_) => return 1,
        },
    };

    // The migration witnesses' entities. They are seeded once, here, and every
    // later generation finds them through the columns it migrates instead of
    // seeding a second set.
    seed_migration_entities(engine.world_mut());

    // The spline entity. `Spline` is registered by the linked `pill_spline`
    // crate, and the host may also be loading a separately built copy of that
    // crate as a module DLL. `Spline` declares `shared`, so both
    // copies are one component with one column: this query sees the module's
    // spline as readily as the project's own, and whichever artifact
    // initialises first creates the entity while the other adopts it.
    //
    // `spline_path_system` rewrites the control points from the ball centres
    // every frame, so the only decision left here is whether the world already
    // has a spline: the module's copy of `pill_spline` may have run first and
    // created one, and a second would be a curve nobody asked for. A fresh one
    // is seeded with the spawn positions, so even the first frame shows a
    // curve through the balls rather than every sample dot stacked on the
    // origin.
    let existing_spline = {
        let mut query = Query::<&Spline>::new(engine.world_mut());
        query.iter_mut().next().copied()
    };
    let spline = match existing_spline {
        Some(spline) => spline,
        None => {
            let mut spawn_points = [Vector3f::ZERO; BALL_COUNT];
            for (index, point) in spawn_points.iter_mut().enumerate() {
                let ball = ball_spawn_state(index);
                *point = Vector3f::new(ball.position_x, ball.position_y, 0.0);
            }
            let seeded = Spline::from_points(&spawn_points);
            if engine
                .world_mut()
                .create_entity()
                .with(seeded)
                .build()
                .is_err()
            {
                error!(
                    target: pill_core::telemetry::telemetry_target::ECS,
                    "failed to build the project spline entity; aborting this generation"
                );
                return 1;
            }
            seeded
        }
    };

    // Balls are the scene's only simulated bodies, and this is the generation
    // that creates them. The count check only guards a world a startup scene
    // already filled, which the spawn loop must not double.
    let existing_balls = {
        let mut query = Query::<&PhysicsState>::new(engine.world_mut());
        query.iter_mut().count()
    };
    for (index, ball_material) in ball_materials.iter().enumerate().skip(existing_balls) {
        let physics = ball_spawn_state(index);
        let entity = engine
            .world_mut()
            .create_entity()
            .with(physics)
            .with(TransformComponent {
                translation: [
                    (physics.position_x - 400.0) / 80.0,
                    (300.0 - physics.position_y) / 80.0,
                    0.0,
                ],
                scale: [physics.radius / 80.0; 3],
                ..Default::default()
            })
            .with(
                MeshRendererComponent::builder()
                    .mesh(&mesh)
                    .material(ball_material)
                    .build(),
            );

        if entity.build().is_err() {
            // Report the failure and abort the generation: the host keeps the
            // previously loaded module when the init returns non-zero.
            error!(
                target: pill_core::telemetry::telemetry_target::ECS,
                "failed to build a ball entity; aborting this project generation"
            );
            return 1;
        }
    }

    // The camera follows the first ball, set from code so the Inspector's
    // reference row has a target to name the moment the project starts
    // (W-0048 Stage 6). Attaching only when absent preserves a target a startup
    // scene - or the user - already set.
    let first_ball = Query::<(&PhysicsState, Entity)>::new(engine.world_mut())
        .iter_mut()
        .next()
        .map(|(_, entity)| entity);
    if engine
        .world()
        .get_component::<follow_target::FollowTarget>(camera)
        .is_none()
    {
        if let Some(first_ball) = first_ball {
            if engine
                .world_mut()
                .add_component(
                    camera,
                    follow_target::FollowTarget {
                        target: first_ball,
                        weight: FOLLOW_WEIGHT,
                    },
                )
                .is_err()
            {
                error!(
                    target: pill_core::telemetry::telemetry_target::ECS,
                    "failed to attach the camera's follow target; aborting this project generation"
                );
                return 1;
            }
        }
    }

    // The probe reports the spline entity's stable id, which is what the reload
    // suites compare across a reload: the entity handle names a slot the world
    // recycles, the id names the entity (W-0048 Stage 5). The spline is the
    // witness because it is `shared` and so is still there after a
    // project reload, while the project's own components are only carried over
    // when they are persistable.
    refresh_probe_entity_id(engine);
    if let Some(state) = engine.world_mut().get_resource_mut::<SplineProbeState>() {
        state.last_report = Instant::now();
    }

    // One sample dot per t step, placed on the curve as the spawn positions
    // shape it. Only the position is ever rewritten afterwards, so the
    // inspector can retune the dots' size and colour while the sim runs.
    let existing_samples = {
        let mut query = Query::<&SplineSample>::new(engine.world_mut());
        query.iter_mut().count()
    };
    for index in existing_samples..SPLINE_SAMPLE_COUNT {
        let t = index as f32 * SPLINE_SAMPLE_STEP;
        let location = spline.get_location_at(t);
        let entity = engine
            .world_mut()
            .create_entity()
            .with(SplineSample { t })
            .with(TransformComponent {
                translation: [
                    (location.x - 400.0) / 80.0,
                    (300.0 - location.y) / 80.0,
                    0.0,
                ],
                scale: [SPLINE_SAMPLE_DOT_SIZE / 160.0; 3],
                ..Default::default()
            })
            .with(
                MeshRendererComponent::builder()
                    .mesh(&mesh)
                    .material(&sample_material)
                    .build(),
            );

        if entity.build().is_err() {
            error!(
                target: pill_core::telemetry::telemetry_target::ECS,
                "failed to build a spline sample dot; aborting this project generation"
            );
            return 1;
        }
    }

    // Report successful registration so the host keeps this generation.
    0
}

/// Points the probe at the live spline entity, if the world has one.
///
/// The probe reports a stable entity id rather than an entity handle, and the
/// state holding it is replaced on every generation. Both entry points
/// therefore re-derive the id: [`init`] after it creates the spline, and
/// [`register`] after it replaces the state on a world that already has one.
/// A generation that lost the entity leaves the field empty, which the probe
/// prints as `entityid none` instead of a stale value.
fn refresh_probe_entity_id(engine: &mut Engine) {
    let probe_entity_id = spline_probe::spline_entity(engine.world())
        .and_then(|entity| engine.world().entity_id(entity))
        .map(|id| id.to_string());
    if let Some(state) = engine.world_mut().get_resource_mut::<SplineProbeState>() {
        state.probe_entity_id = probe_entity_id;
    }
}

/// Import the project's own fragment as a shader asset.
///
/// `init` fails when it does not load: a project shader that is missing from
/// the store is a broken project, not a shorter scene.
///
/// # Errors
///
/// A message naming the file when it will not load.
fn import_probe_shader(world: &mut World) -> Result<(), String> {
    let assets = world
        .get_resource_mut::<AssetManager>()
        .expect("Engine always owns an AssetManager");
    assets
        .import::<Shader>(AssetImport::new(
            PROBE_SHADER_PATH,
            MetadataPolicy::ReadIfPresent,
        ))
        .map(|_imported| ())
        .map_err(|error| format!("`{PROBE_SHADER_PATH}` did not load: {error}"))
}

/// Import the frame this project runs: its pass first, then the chain that
/// names it.
///
/// A chain's file names its passes by guid, so the pass has to be in the store
/// before the chain resolves it. Importing a file that is already loaded hands
/// back the same asset, which is what keeps a hot reload's second `init` from
/// failing on a name that is taken.
///
/// # Errors
///
/// A message naming the file when the pass or the chain will not load.
fn load_chain(assets: &mut AssetManager) -> Result<Handle<RenderingPipeline>, String> {
    assets
        .import_standalone::<RenderPass>(Path::new(PASS_PATH))
        .map_err(|error| format!("`{PASS_PATH}` did not load: {error}"))?;
    assets
        .import_standalone::<RenderingPipeline>(Path::new(CHAIN_PATH))
        .map_err(|error| format!("`{CHAIN_PATH}` did not load: {error}"))
        .map(|loaded| loaded.handle)
}

// =============================================================================
// Migration witnesses
// =============================================================================

/// A per-frame counter, the component the counter system advances.
#[component(persistable)]
#[derive(Debug, Clone, Default)]
struct FrameCounter {
    count: u64,
}

/// A two-axis position, the simplest shape a schema change can widen.
#[component(persistable)]
#[derive(Debug, Clone, Default)]
struct SpatialPosition {
    horizontal: f32,
    vertical: f32,
}

/// A two-axis velocity, paired with [`SpatialPosition`] on one archetype.
#[component(persistable)]
#[derive(Debug, Clone, Default)]
struct LinearVelocity {
    horizontal_speed: f32,
    vertical_speed: f32,
}

/// A component whose fields own heap data, so migration has to round-trip a
/// `Vec`, a `String` and an engine-owned `DynamicBuffer` through JSON in both
/// directions.
#[component(persistable)]
#[derive(Debug, Clone, Default)]
struct MarkerTrail {
    points: Vec<f32>,
    label: String,
    samples: DynamicBuffer<f32>,
}

/// Guards the once-per-generation [`SpatialPosition`] witness print.
static POSITION_WITNESS_PRINTED: AtomicBool = AtomicBool::new(false);

/// Guards the once-per-generation [`MarkerTrail`] witness print.
static TRAIL_WITNESS_PRINTED: AtomicBool = AtomicBool::new(false);

/// Seeds one entity per archetype, on every generation.
fn seed_migration_entities(world: &mut World) {
    let _ = world
        .create_entity()
        .with(FrameCounter { count: 0 })
        .with(MarkerTrail {
            points: vec![1.0, 2.0, 3.0],
            label: String::from("alpha"),
            samples: DynamicBuffer::from_slice(&[0.5, 1.5]),
        })
        .build();

    let _ = world
        .create_entity()
        .with(FrameCounter { count: 90 })
        .with(SpatialPosition {
            horizontal: 10.0,
            vertical: 20.0,
        })
        .build();

    let _ = world
        .create_entity()
        .with(SpatialPosition {
            horizontal: 1.0,
            vertical: 2.0,
        })
        .with(LinearVelocity {
            horizontal_speed: 1.5,
            vertical_speed: 0.25,
        })
        .with(MarkerTrail {
            points: vec![4.0, 5.0],
            label: String::from("beta"),
            samples: DynamicBuffer::from_slice(&[2.5]),
        })
        .build();

    let _ = world
        .create_entity()
        .with(FrameCounter { count: 180 })
        .with(SpatialPosition {
            horizontal: -5.0,
            vertical: 8.0,
        })
        .with(LinearVelocity {
            horizontal_speed: 0.5,
            vertical_speed: 0.75,
        })
        .build();
}

/// Increments the counter every frame. When it reaches the threshold,
/// resets and logs `counter tick`, the suites' liveness signal.
#[system(name = "counter", order = 560)]
fn counter_system(mut query: Query<&mut FrameCounter>) {
    const THRESHOLD: u64 = 200;

    for mut counter in query.iter_mut() {
        counter.count += 1;
        if counter.count >= THRESHOLD {
            counter.count = 0;

            // The formatter stamps the time; the suites wait for the text.
            pill_engine::tracing::info!(target: "project", "counter tick");
        }
    }
}

/// Logs every `SpatialPosition` value once per module generation.
#[system(name = "position_witness", order = 570)]
fn position_witness_system(mut query: Query<&SpatialPosition>) {
    if POSITION_WITNESS_PRINTED.swap(true, Ordering::Relaxed) {
        return;
    }
    for position in query.iter_mut() {
        pill_engine::tracing::info!(
            target: "project",
            "[project] witness SpatialPosition({:.2},{:.2})",
            position.horizontal, position.vertical
        );
    }
}

/// Logs every `MarkerTrail` value once per module generation, including the
/// heap contents migration must carry across (element count, sum, text, and
/// the engine-owned buffer's length and last element).
#[system(name = "trail_witness", order = 580)]
fn trail_witness_system(mut query: Query<&MarkerTrail>) {
    if TRAIL_WITNESS_PRINTED.swap(true, Ordering::Relaxed) {
        return;
    }
    for trail in query.iter_mut() {
        let sum: f32 = trail.points.iter().sum();
        let last = trail.samples.last().copied().unwrap_or(0.0);
        pill_engine::tracing::info!(
            target: "project",
            "[project] witness MarkerTrail(points={},sum={:.2},label=\"{}\",samples={},last={:.2})",
            trail.points.len(),
            sum,
            trail.label,
            trail.samples.len(),
            last,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh world runs the chain the project's own files declare: `init`
    /// imports `pipelines/main.pipeline`, hands it to the renderer's manager
    /// instead of the installed default chain, and the chain holds the
    /// project's `dwdw.render_pass`.
    ///
    /// The test drives the two entry points in the order the host does -
    /// `register` first, then `init` - because `init` no longer registers the
    /// renderer itself.
    #[test]
    fn init_hands_the_manager_the_chain_from_the_project_files() {
        // A host mounts this project's `res` and registers the modules the
        // project lists before its entry points; the test mounts the directory
        // and registers what the scene needs itself.
        //
        // The same file compiles as this project's crate and, when a host runs
        // the project, as the generated workspace member under
        // `modules/projects_generated/`. The member's manifest directory is not
        // the project root, so the source file anchors the directory both
        // builds share: the member points its `[lib] path` straight at it.
        let manifest_directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source_file = std::path::Path::new(file!());
        let source_file = if source_file.is_absolute() {
            source_file.to_path_buf()
        } else {
            manifest_directory.join(source_file)
        };
        let project_root = source_file
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the project's lib.rs sits in src/");
        AssetLoader::set_root(project_root.join("res"));
        let mut engine = Engine::new();
        engine
            .world_mut()
            .register_component::<pill_spline::Spline>();
        // The artifact-wide loop the host and the runtime run before the
        // entry points registers the project's own components; a direct call
        // does it here. Neither entry point repeats it: a second live
        // registration of one type name fails the generation, which is what a
        // reload would hit.
        __pill_register_PhysicsState(engine.world_mut());
        __pill_register_SplineSample(engine.world_mut());

        // The chain loads straight from the project's files, so a broken file
        // fails here with its own message rather than as a status code.
        let imported = {
            let assets = engine
                .world_mut()
                .get_resource_mut::<AssetManager>()
                .expect("the engine owns an asset manager");
            load_chain(assets).expect("the project's chain loads")
        };

        assert_eq!(register(&mut engine), 0);
        assert_eq!(init(&mut engine), 0);

        let manager = engine
            .world()
            .get_resource::<RenderingManager>()
            .expect("the renderer data registers its manager");
        let pipeline = manager.pipeline().expect("init set the chain");
        let assets = engine
            .world()
            .get_resource::<AssetManager>()
            .expect("the engine owns an asset manager");
        let chain = assets
            .handle_by_name::<RenderingPipeline>(CHAIN_PATH)
            .expect("the chain is imported under its path");
        let pass = assets
            .handle_by_name::<RenderPass>(PASS_PATH)
            .expect("the pass is imported under its path");

        assert_eq!(pipeline, imported);
        assert_eq!(imported, chain);
        assert_eq!(
            assets.get(chain).expect("the chain is loaded").passes,
            vec![pill_rendering::PipelinePass::new(pass)]
        );
    }
}
