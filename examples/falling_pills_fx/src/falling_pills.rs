//! A rain of small pills (the demo's "Build" shot emitter). Each pill falls at its own constant
//! speed from a random XZ position, tumbling, and wraps from the floor back to the top. The pose
//! is a closed-form function of time, not simulation state.

use pill_engine::{define_component, define_global_component, game::*};

pub(crate) const FALLING_PILLS_COUNT: usize = 688;
/// XZ spawn half-extent around (0, FALLING_PILLS_Z_CENTER).
const FALLING_PILLS_AREA: f32 = 3.3;
const FALLING_PILLS_Z_CENTER: f32 = -0.5;
/// Loop range: pills fall from HEIGHT down to FLOOR, then wrap back to HEIGHT.
const FALLING_PILLS_HEIGHT: f32 = 6.5;
const FALLING_PILLS_FLOOR: f32 = -1.0;
/// Per-pill fall speed range (units/s).
const FALLING_PILLS_SPEED_MIN: f32 = 0.1;
const FALLING_PILLS_SPEED_MAX: f32 = 0.3;
const FALLING_PILLS_SCALE: f32 = 0.08;
/// Per-pill tumble rate range per axis (degrees/s).
const FALLING_PILLS_SPIN_SPEED_MAX: f32 = 86.0;

define_component!(FallingPill {
    x: f32,
    z: f32,
    speed: f32,
    // Fraction of the fall cycle to offset by, so pills don't fall in lockstep
    phase: f32,
    // Starting orientation and tumble rate per axis (degrees, degrees/s)
    rotation: Vector3f,
    spin: Vector3f,
});

// Rising instead of falling (set per shot)
define_global_component!(FallingPillsDirection { rising: bool });

/// Deterministic hash → [0, 1) (GLSL sin-hash); a fixed per-index scatter needs no RNG.
fn hash01(seed: f32) -> f32 {
    (seed.sin() * 43_758.547).fract().abs()
}

fn make_pill(index: usize) -> FallingPill {
    let seed = index as f32 * 7.0 + 1.0;
    let symmetric = |salt: f32| hash01(seed * salt) * 2.0 - 1.0;
    FallingPill {
        x: symmetric(12.9898) * FALLING_PILLS_AREA,
        z: FALLING_PILLS_Z_CENTER + symmetric(78.233) * FALLING_PILLS_AREA,
        speed: FALLING_PILLS_SPEED_MIN
            + hash01(seed * 39.346) * (FALLING_PILLS_SPEED_MAX - FALLING_PILLS_SPEED_MIN),
        phase: hash01(seed * 11.135),
        rotation: Vector3f::new(
            hash01(seed * 45.164) * 360.0,
            hash01(seed * 71.324) * 360.0,
            hash01(seed * 20.719) * 360.0,
        ),
        spin: Vector3f::new(
            symmetric(55.132) * FALLING_PILLS_SPIN_SPEED_MAX,
            symmetric(83.417) * FALLING_PILLS_SPIN_SPEED_MAX,
            symmetric(17.638) * FALLING_PILLS_SPIN_SPEED_MAX,
        ),
    }
}

/// Spawns `FALLING_PILLS_COUNT` pills sharing `mesh` and `material`.
pub(crate) fn spawn_falling_pills(
    engine: &mut Engine,
    scene: SceneHandle,
    mesh: &MeshHandle,
    material: &MaterialHandle,
) {
    for index in 0..FALLING_PILLS_COUNT {
        let pill = make_pill(index);
        engine
            .build_entity(scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(pill.x, FALLING_PILLS_HEIGHT, pill.z))
                    .rotation(pill.rotation)
                    .scale(Vector3f::splat(FALLING_PILLS_SCALE))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(mesh)
                    .material(material)
                    .build(),
            )
            .with_component(pill)
            .build();
    }
}

pub(crate) fn falling_pills_system(engine: &mut Engine) -> Result<()> {
    let time = engine.get_global_component::<TimeComponent>()?.time;
    let rising = engine
        .get_global_component::<FallingPillsDirection>()?
        .rising;
    let span = FALLING_PILLS_HEIGHT - FALLING_PILLS_FLOOR;
    for (_, transform, pill) in
        engine.iterate_two_components_mut::<TransformComponent, FallingPill>()?
    {
        let cycle = span / pill.speed;
        let fall_time = (time + pill.phase * cycle).rem_euclid(cycle);
        let y = if rising {
            FALLING_PILLS_FLOOR + pill.speed * fall_time
        } else {
            FALLING_PILLS_HEIGHT - pill.speed * fall_time
        };
        transform.set_position(Vector3f::new(pill.x, y, pill.z));
        transform.set_rotation(pill.rotation + pill.spin * time);
    }
    Ok(())
}
