use pill_engine::{define_component, define_global_component, game::*};
use std::{cell::Cell, collections::VecDeque};
use wasm_bindgen::prelude::*;

// --- Scene constants ---------------------------------------------------------

// Camera
const CAMERA_POSITION_Z: f32 = 0.0;
const CAMERA_FOV: f32 = 60.0;
const CLEAR_COLOR: (f32, f32, f32) = (1.0, 0.27, 0.27);
// exp(-density²·distance²). Half-blend at d=20 → visible haze pulls in close
// and the back half of the tunnel vanishes into bg:
//   d=10 (hero) → 16%, d=20 → 50%, d=40 → 94%, d=80 (far) → 100%.
const FOG_DENSITY: f32 = 0.0416;

// Subtle sine-wave drift on camera position — breathes life into an otherwise
// fixed viewpoint without disorienting the viewer.
const CAMERA_DRIFT_SPEED: (f32, f32) = (0.30, 0.22);
const CAMERA_DRIFT_AMPLITUDE: (f32, f32) = (0.35, 0.25);

// Particle tunnel — N pills streaming forward through a donut-shaped emitter.
// Camera looks down +Z; pills start spread uniformly along Z so steady-state
// density is reached on frame 1. Each pill wraps back to far when it crosses near.
const PILL_COUNT: usize = 1440;
const EMITTER_RADIUS_MIN: f32 = 6.0;
const EMITTER_RADIUS_MAX: f32 = 13.0;
const TUNNEL_NEAR_Z: f32 = -3.0;
const TUNNEL_FAR_Z: f32 = 80.0;
const PILL_FORWARD_SPEED: f32 = 8.0;
const PILL_SCALE: f32 = 0.5;
const PILL_SCALE_JITTER: f32 = 0.25; // ± fraction per particle
const PILL_SPIN: (f32, f32, f32) = (0.4, 0.8, 1.2);
const PILL_SPIN_VARIANCE: f32 = 0.4; // ± fraction per particle
                                     // Emit PILL_COUNT particles from the donut (uniform in θ, radius, z).
const RADIUS_SPAN: f32 = EMITTER_RADIUS_MAX - EMITTER_RADIUS_MIN;
const TUNNEL_LENGTH: f32 = TUNNEL_FAR_Z - TUNNEL_NEAR_Z;

// Parallax: per-particle forward speed scales with 1/radius → inner pills
// streak past, outer pills drift. Big depth cue for nearly free.
const PILL_PARALLAX_REF_RADIUS: f32 = EMITTER_RADIUS_MIN;

// Y wobble: each particle gets a per-particle phase; y oscillates as the pill
// travels through z so the whole tunnel reads as "swimming", not rigid.
const PILL_WOBBLE_AMPLITUDE: f32 = 0.45;
const PILL_WOBBLE_Z_K: f32 = 0.22; // ≈ 2.9 waves across the tunnel length

// Hero pill — large, slowly rotating, gently breathing centerpiece.
const HERO_POSITION_Z: f32 = 12.0;
const HERO_SCALE: f32 = 1.5;
const HERO_SPIN: (f32, f32, f32) = (0.15, 0.25, 0.10);
const HERO_BREATH_AMPLITUDE: f32 = 0.08; // ± fraction of scale
const HERO_BREATH_SPEED: f32 = 0.7;

// Brand-cohesive palette: 4 warm tints + 1 cool lavender accent.
const PALETTE: &[(f32, f32, f32)] = &[
    (1.00, 0.45, 0.60), // coral pink — brand primary
    (1.00, 0.72, 0.38), // amber
    (1.00, 0.32, 0.52), // rose
    (0.95, 0.40, 0.82), // magenta
    (0.70, 0.55, 1.00), // lavender
];
const HERO_TINT: (f32, f32, f32) = (1.00, 0.55, 0.65);

// --- Deterministic hash RNG --------------------------------------------------
// Wang-style integer hash + seed table → independent streams per particle.
// No `rand` dep, reproducible across runs.

const SEED_ANGLE: u32 = 0x9e37_79b9;
const SEED_RADIUS: u32 = 0x85eb_ca6b;
const SEED_Z: u32 = 0xc2b2_ae35;
const SEED_SCALE: u32 = 0x27d4_eb2d;
const SEED_SPIN: u32 = 0x1656_67b1;
const SEED_MATERIAL: u32 = 0x7ed5_5d16;
const SEED_WOBBLE: u32 = 0xd3a2_646c;
const SEED_ROT_X: u32 = 0xa5a5_f00d;
const SEED_ROT_Y: u32 = 0x5a5a_feed;
const SEED_ROT_Z: u32 = 0x3c3c_b16b;

// --- Smoothing Filter constants ----
const FILTER_SIZE: usize = 100;

fn hash_u32(mut n: u32) -> u32 {
    n = (n ^ 61) ^ (n >> 16);
    n = n.wrapping_mul(9);
    n ^= n >> 4;
    n = n.wrapping_mul(0x27d4_eb2d);
    n ^= n >> 15;
    n
}
fn hash_f32(i: usize, seed: u32) -> f32 {
    hash_u32((i as u32).wrapping_add(seed)) as f32 / u32::MAX as f32
}
fn hash_usize(i: usize, seed: u32, bound: usize) -> usize {
    hash_u32((i as u32).wrapping_add(seed)) as usize % bound
}
// Centered in [-1, 1).
fn hash_signed(i: usize, seed: u32) -> f32 {
    hash_f32(i, seed) * 2.0 - 1.0
}

// --- Helpers -----------------------------------------------------------------

fn apply_spin(transform: &mut TransformComponent, rate: (f32, f32, f32), dt: f32) {
    let r = transform.rotation;
    transform.set_rotation(Vector3f::new(
        r.x + rate.0 * dt,
        r.y + rate.1 * dt,
        r.z + rate.2 * dt,
    ));
}

fn tinted_pill_material(
    engine: &mut Engine,
    name: &str,
    tint: (f32, f32, f32),
    color_tex: TextureHandle,
    normal_tex: TextureHandle,
) -> Result<MaterialHandle> {
    engine.add_resource(
        Material::builder(name)
            .texture("color", color_tex)?
            .texture("normal", normal_tex)?
            .color_parameter("tint", Color::new(tint.0, tint.1, tint.2))?
            .build(),
    )
}

define_global_component!(GameState {
    tunnel_materials: Vec<MaterialHandle>,
    pill_mesh: MeshHandle,
    smoothed_fps: SmoothingFilter,
});

// TODO: improvement - this could be an EMA filter instead, and calculate time samples instead of
// FPS
struct SmoothingFilter {
    data: VecDeque<f32>,
    average: f64,
}

impl SmoothingFilter {
    pub fn new(size: usize) -> Self {
        SmoothingFilter {
            data: VecDeque::with_capacity(size),
            average: 0.0,
        }
    }

    pub fn add_sample(&mut self, sample: f32) {
        if self.data.len() < self.data.capacity() {
            self.data.push_back(sample);
            self.average += (sample / self.data.capacity() as f32) as f64;
            return;
        }

        let diff: f32 = sample - self.data.pop_front().unwrap();
        self.average += (diff / self.data.capacity() as f32) as f64;
        self.data.push_back(sample);
    }

    pub fn get_average(&self) -> f64 {
        self.average
    }

    pub fn is_ready(&self) -> bool {
        self.data.len() == self.data.capacity()
    }
}

struct DemoValues {
    fps: Cell<f32>,
    frame_time_ms: Cell<f64>,
    desired_pill_count: Cell<u32>,
    current_pill_count: Cell<u32>,
    spawn_increment: Cell<u32>,
    next_pill_id: Cell<u32>,
}

fn calculate_pill_spawn_increment(current_count: u32) -> u32 {
    const STEPS: [(u32, u32); 5] = [
        (PILL_COUNT as u32 * 2, 500),
        (PILL_COUNT as u32 * 5, 1_000),
        (PILL_COUNT as u32 * 10, 2_000),
        (PILL_COUNT as u32 * 20, 5_000),
        (PILL_COUNT as u32 * 50, 10_000),
    ];
    for (pill_count, increment) in STEPS {
        if current_count < pill_count {
            return increment;
        }
    }
    STEPS[4].0
}

fn calculate_desired_spawn(current_count: u32) -> u32 {
    const STEPS: [(u32, u32); 5] = [
        (1_500, 10),
        (3_000, 20),
        (8_000, 30),
        (16_000, 40),
        (32_000, 50),
    ];
    for (pill_count, spawn) in STEPS {
        if current_count < pill_count {
            return spawn;
        }
    }
    STEPS[0].1
}

thread_local!(static DEMO_VALUES: DemoValues = const { DemoValues{
    fps: Cell::new(60.0),
    frame_time_ms: Cell::new(0.0),
    desired_pill_count: Cell::new(PILL_COUNT as u32),
    current_pill_count: Cell::new(PILL_COUNT as u32),
    spawn_increment: Cell::new(0),
    next_pill_id: Cell::new(PILL_COUNT as u32),
}});

// --- API ---
#[wasm_bindgen]
pub fn spawn_more_pills() {
    // TODO: add some kind of cooldown to prevent spamming
    let current_count = DEMO_VALUES.with(|v| v.current_pill_count.get());
    let spawn_increment = calculate_pill_spawn_increment(current_count);
    DEMO_VALUES.with(|v| v.spawn_increment.set(spawn_increment));
    DEMO_VALUES.with(|v| v.desired_pill_count.set(spawn_increment * 10));
}

#[wasm_bindgen]
pub fn reset_pills() {
    DEMO_VALUES.with(|v| v.desired_pill_count.set(0))
}

#[wasm_bindgen]
pub fn get_fps() -> f32 {
    DEMO_VALUES.with(|v| v.fps.get())
}

#[wasm_bindgen]
pub fn get_frame_time_ms() -> f64 {
    DEMO_VALUES.with(|v| v.frame_time_ms.get())
}

#[wasm_bindgen]
pub fn get_num_pills() -> u32 {
    DEMO_VALUES.with(|v| v.current_pill_count.get())
}

// --- Components --------------------------------------------------------------

define_component!(PillParticleComponent {
    spin_multiplier: f32,
    forward_speed: f32, // per-particle (parallax)
    base_y: f32,        // sample point around which y wobbles
    wobble_phase: f32,  // per-particle phase for the y wobble
});
define_component!(HeroPillComponent {});

pub struct WebGame {}

fn spawn_pill(
    engine: &mut Engine,
    active_scene: SceneHandle,
    tunnel_materials: &[MaterialHandle],
    pill_mesh: MeshHandle,
    i: usize,
) -> Result<()> {
    let theta = hash_f32(i, SEED_ANGLE) * std::f32::consts::TAU;
    let radius = EMITTER_RADIUS_MIN + hash_f32(i, SEED_RADIUS) * RADIUS_SPAN;
    let z = TUNNEL_NEAR_Z + hash_f32(i, SEED_Z) * TUNNEL_LENGTH;
    let base_x = theta.cos() * radius;
    let base_y = theta.sin() * radius;
    let scale = PILL_SCALE * (1.0 + PILL_SCALE_JITTER * hash_signed(i, SEED_SCALE));
    let spin_multiplier = 1.0 + PILL_SPIN_VARIANCE * hash_signed(i, SEED_SPIN);
    let forward_speed = PILL_FORWARD_SPEED * PILL_PARALLAX_REF_RADIUS / radius;
    let wobble_phase = hash_f32(i, SEED_WOBBLE) * std::f32::consts::TAU;
    let rotation = Vector3f::new(
        hash_f32(i, SEED_ROT_X) * std::f32::consts::TAU,
        hash_f32(i, SEED_ROT_Y) * std::f32::consts::TAU,
        hash_f32(i, SEED_ROT_Z) * std::f32::consts::TAU,
    );
    let material = tunnel_materials[hash_usize(i, SEED_MATERIAL, tunnel_materials.len())];

    engine
        .build_entity(active_scene)
        .with_component(
            TransformComponent::builder()
                .position(Vector3f::new(base_x, base_y, z))
                .rotation(rotation)
                .scale(Vector3f::new(scale, scale, scale))
                .build(),
        )
        .with_component(
            MeshRenderingComponent::builder()
                .mesh(&pill_mesh)
                .material(&material)
                .build(),
        )
        .with_component(PillParticleComponent {
            spin_multiplier,
            forward_speed,
            base_y,
            wobble_phase,
        })
        .build();
    Ok(())
}

// --- Systems -----------------------------------------------------------------

fn pill_particle_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
    let tunnel_length = TUNNEL_FAR_Z - TUNNEL_NEAR_Z;
    let mut count = 0;

    for (_entity, transform, pill) in
        engine.iterate_two_components_mut::<TransformComponent, PillParticleComponent>()?
    {
        let mut z = transform.position.z - pill.forward_speed * dt;
        while z < TUNNEL_NEAR_Z {
            z += tunnel_length;
        }
        let y_wobble = (z * PILL_WOBBLE_Z_K + pill.wobble_phase).sin() * PILL_WOBBLE_AMPLITUDE;
        transform.set_position(Vector3f::new(
            transform.position.x,
            pill.base_y + y_wobble,
            z,
        ));

        let m = pill.spin_multiplier;
        apply_spin(
            transform,
            (PILL_SPIN.0 * m, PILL_SPIN.1 * m, PILL_SPIN.2 * m),
            dt,
        );
        count += 1;
    }

    DEMO_VALUES.with(|v| v.current_pill_count.set(count));

    Ok(())
}

fn hero_pill_system(engine: &mut Engine) -> Result<()> {
    let tc = engine.get_global_component::<TimeComponent>()?;
    let (time, dt) = (tc.time, tc.delta_time);
    let s = HERO_SCALE * (1.0 + HERO_BREATH_AMPLITUDE * (time * HERO_BREATH_SPEED).sin());
    for (_entity, transform, _hero) in
        engine.iterate_two_components_mut::<TransformComponent, HeroPillComponent>()?
    {
        apply_spin(transform, HERO_SPIN, dt);
        transform.set_scale(Vector3f::new(s, s, s));
    }
    Ok(())
}

fn camera_drift_system(engine: &mut Engine) -> Result<()> {
    let time = engine.get_global_component::<TimeComponent>()?.time;
    for (_entity, transform, _cam) in
        engine.iterate_two_components_mut::<TransformComponent, CameraComponent>()?
    {
        let x = (time * CAMERA_DRIFT_SPEED.0).sin() * CAMERA_DRIFT_AMPLITUDE.0;
        let y = (time * CAMERA_DRIFT_SPEED.1).cos() * CAMERA_DRIFT_AMPLITUDE.1;
        transform.set_position(Vector3f::new(x, y, CAMERA_POSITION_Z));
    }
    Ok(())
}

fn demo_update_system(engine: &mut Engine) -> Result<()> {
    let active_scene = engine.get_active_scene_handle()?;
    let (tunnel_materials, pill_mesh) = {
        let state = engine.get_global_component::<GameState>()?;
        (state.tunnel_materials.clone(), state.pill_mesh)
    };

    let fps = engine.get_fps();
    if fps.is_finite() && fps > 0.0 {
        let smoothing_filter = &mut engine.get_global_component_mut::<GameState>()?.smoothed_fps;
        smoothing_filter.add_sample(fps);
        if smoothing_filter.is_ready() {
            DEMO_VALUES.with(|v| v.fps.set(smoothing_filter.get_average() as f32));
            DEMO_VALUES.with(|v| v.frame_time_ms.set(1000.0 / smoothing_filter.get_average()));
        }
    }

    let requested_count = DEMO_VALUES.with(|v| v.desired_pill_count.get());
    let current_count = DEMO_VALUES.with(|v| v.current_pill_count.get());

    if requested_count == 0 {
        // despawn all except hero
        let mut entities: Vec<EntityHandle> = vec![];
        for (entity, _) in engine.iterate_one_component_mut::<PillParticleComponent>()? {
            entities.push(entity);
        }
        for entity in entities {
            engine.remove_entity_default_scene(entity)?;
        }
        DEMO_VALUES.with(|v| v.current_pill_count.set(0));
        return Ok(());
    }

    if requested_count > current_count {
        let remaining = requested_count - current_count;
        let to_spawn = calculate_desired_spawn(current_count).min(remaining);

        let next_id = DEMO_VALUES.with(|v| v.next_pill_id.get());

        for offset in 0..to_spawn {
            spawn_pill(
                engine,
                active_scene,
                &tunnel_materials,
                pill_mesh,
                (next_id + offset) as usize,
            )?;
        }

        DEMO_VALUES.with(|v| {
            v.next_pill_id.set(next_id + to_spawn);
            v.current_pill_count.set(current_count + to_spawn);
        });
    } else {
        // TODO: despawn
    }

    Ok(())
}

// --- Game --------------------------------------------------------------------

impl PillGame for WebGame {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<PillParticleComponent>(active_scene)?;
        engine.register_component::<HeroPillComponent>(active_scene)?;

        // Assets embedded in the binary (zero runtime I/O on any target).
        let pill_mesh = engine.add_resource(Mesh::from_cooked_mesh_bytes(
            "pill",
            include_bytes!("../res/models/pill.cooked_mesh"),
        )?)?;
        let color_tex = engine.add_resource::<Texture>(Texture::from_bytes(
            "pill_color",
            TextureType::Color,
            include_bytes!("../res/textures/pill_color.cooked_tex"),
        ))?;
        let normal_tex = engine.add_resource::<Texture>(Texture::from_bytes(
            "pill_normal",
            TextureType::Normal,
            include_bytes!("../res/textures/pill_normal.cooked_tex"),
        ))?;

        let tunnel_materials: Vec<MaterialHandle> = PALETTE
            .iter()
            .enumerate()
            .map(|(i, tint)| {
                tinted_pill_material(
                    engine,
                    &format!("tunnel_material_{i}"),
                    *tint,
                    color_tex,
                    normal_tex,
                )
            })
            .collect::<Result<_>>()?;
        let hero_material =
            tinted_pill_material(engine, "hero_material", HERO_TINT, color_tex, normal_tex)?;
        engine.add_global_component(GameState {
            tunnel_materials,
            pill_mesh,
            smoothed_fps: SmoothingFilter::new(FILTER_SIZE),
        })?;
        let tunnel_materials = engine
            .get_global_component::<GameState>()?
            .tunnel_materials
            .clone();

        // Camera
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.0, CAMERA_POSITION_Z))
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(CAMERA_FOV)
                    .clear_color(Color::new(CLEAR_COLOR.0, CLEAR_COLOR.1, CLEAR_COLOR.2))
                    .fog_density(FOG_DENSITY)
                    // Fade distant pills toward the clear color so the tunnel wrap seam disappears.
                    .fog_color(Color::new(CLEAR_COLOR.0, CLEAR_COLOR.1, CLEAR_COLOR.2))
                    .build(),
            )
            .build();

        // Hero pill
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.0, HERO_POSITION_Z))
                    .scale(Vector3f::new(HERO_SCALE, HERO_SCALE, HERO_SCALE))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&pill_mesh)
                    .material(&hero_material)
                    .build(),
            )
            .with_component(HeroPillComponent {})
            .build();

        for i in 0..PILL_COUNT {
            spawn_pill(engine, active_scene, &tunnel_materials, pill_mesh, i)?;
        }

        engine.add_system("pill_particle", pill_particle_system)?;
        engine.add_system("hero_pill", hero_pill_system)?;
        engine.add_system("camera_drift", camera_drift_system)?;
        engine.add_system("demo_update", demo_update_system)?;
        Ok(())
    }
}
