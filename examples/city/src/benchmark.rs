//! Benchmark mode for the city example.
//!
//! Compiled via `--features benchmark_windowed` or `--features benchmark_headless`.
//! Spawns 10 000 citizens, runs the simulation for N frames, collects per‑frame
//! timing statistics, prints a JSON report to stdout, and exits automatically.

use pill_engine::{define_global_component, project::*};
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::time::Instant;

use crate::shared;

// -- Constants ---------------------------------------------------------------

const BENCHMARK_CITIZEN_COUNT: usize = 10_000;
const BENCHMARK_DEFAULT_MAX_FRAMES: u64 = 5_000;
const BENCHMARK_WARMUP_FRAMES: u64 = 1_000;
const BENCHMARK_RNG_SEED: Option<u64> = Some(42);

// -- Components --------------------------------------------------------------

define_global_component!(BenchmarkState {
    current_frame: u64,
    max_frames: u64,
    frame_times_ms: Vec<f32>,
    benchmark_start: Option<Instant>,
});

// -- Project --------------------------------------------------------------------

pub struct Project {}

impl PillProject for Project {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        let max_frames = BENCHMARK_DEFAULT_MAX_FRAMES;

        let scene = engine.create_scene("benchmark")?;
        engine.set_active_scene(scene)?;

        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<shared::CitizenComponent>(scene)?;

        // -- Windowed-only: rendering setup ------------------------------
        #[cfg(feature = "benchmark_windowed")]
        {
            engine.register_component::<MeshRenderingComponent>(scene)?;
            engine.register_component::<CameraComponent>(scene)?;

            let gray_material = engine.add_resource::<Material>(
                Material::builder(shared::GRAY_MATERIAL_NAME)
                    .color_parameter("tint", shared::GRAY_MATERIAL_TINT)?
                    .build(),
            )?;
            let _orange = engine.add_resource::<Material>(
                Material::builder(shared::ORANGE_MATERIAL_NAME)
                    .color_parameter("tint", shared::ORANGE_MATERIAL_TINT)?
                    .build(),
            )?;
            let plane_mesh_handle = engine.add_resource(Mesh::new(
                shared::PLANE_MESH_NAME,
                shared::PLANE_MESH_PATH.into(),
            ))?;
            let _pill = engine.add_resource(Mesh::new(
                shared::PILL_MESH_NAME,
                shared::PILL_MESH_PATH.into(),
            ))?;

            // Ground plane
            engine
                .build_entity(scene)
                .with_component(
                    TransformComponent::builder()
                        .position(Vector3f::new(0.0, -2.0, 0.0))
                        .scale(Vector3f::new(20.0, 1.0, 20.0))
                        .build(),
                )
                .with_component(
                    MeshRenderingComponent::builder()
                        .material(&gray_material)
                        .mesh(&plane_mesh_handle)
                        .build(),
                )
                .build();

            // Camera
            engine
                .build_entity(scene)
                .with_component(
                    TransformComponent::builder()
                        .position(shared::CAMERA_POSITION)
                        .rotation(shared::CAMERA_ROTATION)
                        .build(),
                )
                .with_component(
                    CameraComponent::builder()
                        .enabled(true)
                        .fov(55.0)
                        .clear_color(Color::new(0.08, 0.12, 0.18))
                        .build(),
                )
                .build();
        }

        engine.add_system("benchmark_citizen_move", benchmark_citizen_move_system)?;
        engine.add_system("benchmark_frame", benchmark_frame_system)?;

        spawn_citizens(engine, scene, BENCHMARK_CITIZEN_COUNT)?;

        engine.add_global_component(BenchmarkState {
            current_frame: 0,
            max_frames,
            frame_times_ms: Vec::with_capacity(max_frames as usize),
            benchmark_start: None,
        })?;

        Ok(())
    }
}

// -- Systems -----------------------------------------------------------------

fn benchmark_citizen_move_system(engine: &mut Engine) -> Result<()> {
    let mut rng: StdRng = match BENCHMARK_RNG_SEED {
        Some(seed) => StdRng::seed_from_u64(seed.wrapping_add(engine.frame_count())),
        None => StdRng::seed_from_u64(0),
    };
    shared::move_citizens_toward_targets(engine, &mut rng)
}

fn benchmark_frame_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.frame_delta_time();
    let state = engine.get_global_component_mut::<BenchmarkState>()?;
    state.current_frame += 1;

    if state.current_frame > BENCHMARK_WARMUP_FRAMES {
        if state.benchmark_start.is_none() {
            state.benchmark_start = Some(Instant::now());
        }
        state.frame_times_ms.push(delta_time);
    }

    if state.current_frame >= state.max_frames {
        print_report(state);
        engine.request_exit();
    }

    Ok(())
}

// -- Helpers -----------------------------------------------------------------

fn spawn_citizens(engine: &mut Engine, scene: SceneHandle, count: usize) -> Result<()> {
    let mut rng: StdRng = match BENCHMARK_RNG_SEED {
        Some(seed) => StdRng::seed_from_u64(seed),
        None => StdRng::seed_from_u64(0),
    };

    // Windowed mode: citizens get PBR renderable components.
    // Headless mode: citizens are pure simulation entities.
    #[cfg(feature = "benchmark_windowed")]
    let (orange_material, pill_mesh) = {
        let material = engine.get_resource_handle::<Material>("orange")?;
        let mesh = engine.get_resource_handle::<Mesh>("pill")?;
        (material, mesh)
    };

    for _ in 0..count {
        let mut builder = engine.build_entity(scene);

        builder = builder
            .with_component(TransformComponent::new())
            .with_component(shared::CitizenComponent {
                path_points: std::collections::VecDeque::new(),
                current_movement_speed: shared::CITIZEN_INITIAL_SPEED,
                max_movement_speed: rng
                    .gen_range(shared::CITIZEN_MAX_SPEED_MIN..shared::CITIZEN_MAX_SPEED_MAX),
                acceleration: rng
                    .gen_range(shared::CITIZEN_ACCELERATION_MIN..shared::CITIZEN_ACCELERATION_MAX),
                facing_angle: rng.gen_range(0.0..360.0),
                rotation_speed: rng.gen_range(
                    shared::CITIZEN_ROTATION_SPEED_MIN..shared::CITIZEN_ROTATION_SPEED_MAX,
                ),
            });

        #[cfg(feature = "benchmark_windowed")]
        {
            builder = builder.with_component(
                MeshRenderingComponent::builder()
                    .material(&orange_material)
                    .mesh(&pill_mesh)
                    .build(),
            );
        }

        builder.build();
    }

    Ok(())
}

// -- Statistics --------------------------------------------------------------

fn print_report(state: &BenchmarkState) {
    let times = &state.frame_times_ms;
    if times.is_empty() {
        return;
    }

    let sample_count = times.len() as f64;
    let sum: f32 = times.iter().sum();
    let average = sum / sample_count as f32;

    let mut sorted = times.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = if sorted.len() % 2 == 0 {
        (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) / 2.0
    } else {
        sorted[sorted.len() / 2]
    };

    let min = sorted.first().copied().unwrap_or(0.0);
    let max = sorted.last().copied().unwrap_or(0.0);
    let range = max - min;

    let variance: f32 =
        times.iter().map(|t| (t - average).powi(2)).sum::<f32>() / sample_count as f32;
    let standard_deviation = variance.sqrt();

    // Pick the mode string based on which feature is active.
    #[cfg(feature = "benchmark_windowed")]
    let mode = "windowed";
    #[cfg(feature = "benchmark_headless")]
    let mode = "headless";

    println!(
        concat!(
            "{{",
            "\"mode\":\"{md}\",",
            "\"total_frames\":{total},",
            "\"measured_frames\":{measured},",
            "\"warmup_frames\":{warmup},",
            "\"entity_count\":{entities},",
            "\"stats\":{{",
            "\"average_ms\":{avg:.3},",
            "\"median_ms\":{med:.3},",
            "\"min_ms\":{min:.3},",
            "\"max_ms\":{max:.3},",
            "\"range_ms\":{range:.3},",
            "\"variance\":{var:.6},",
            "\"stddev_ms\":{std:.3}",
            "}}",
            "}}"
        ),
        md = mode,
        total = state.current_frame,
        measured = times.len(),
        warmup = BENCHMARK_WARMUP_FRAMES,
        entities = BENCHMARK_CITIZEN_COUNT,
        avg = average,
        med = median,
        min = min,
        max = max,
        range = range,
        var = variance,
        std = standard_deviation,
    );
}
