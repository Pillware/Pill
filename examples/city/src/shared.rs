//! Shared constants, components, and systems for the city example.
//!
//! Used by both `project.rs` (normal play mode) and `benchmark.rs` (benchmark mode).

use pill_engine::{define_component, project::*};
use rand::Rng;

// -- World ---------------------------------------------------------------

pub const WORLD_ORIGIN: Vector3f = Vector3f::new(0.0, 0.0, 0.0);

// -- Citizen movement constants ------------------------------------------

pub const PATH_POINT_COUNT: usize = 4;
pub const PATH_WANDER_RADIUS_XZ: f32 = 20.0;
pub const POINT_REACH_THRESHOLD: f32 = 5.0;

pub const CITIZEN_INITIAL_SPEED: f32 = 0.0;
pub const CITIZEN_MAX_SPEED_MIN: f32 = 4.0;
pub const CITIZEN_MAX_SPEED_MAX: f32 = 8.0;
pub const CITIZEN_ACCELERATION_MIN: f32 = 3.0;
pub const CITIZEN_ACCELERATION_MAX: f32 = 6.0;
pub const CITIZEN_TURN_SLOWDOWN: f32 = 0.4;
pub const CITIZEN_ROTATION_SPEED_MIN: f32 = 150.0;
pub const CITIZEN_ROTATION_SPEED_MAX: f32 = 240.0;

// --- Colors ---
pub const GRAY_MATERIAL_TINT: Color = Color::new(0.3, 0.3, 0.3);
pub const ORANGE_MATERIAL_TINT: Color = Color::new(1.0, 0.5, 0.0);
pub const CLEAR_COLOR: Color = Color::new(0.08, 0.12, 0.18);

// --- World ---
pub const PLANE_POSITION: Vector3f = Vector3f::new(0.0, -2.0, 0.0);
pub const PLANE_SCALE: Vector3f = Vector3f::new(3.0, 1.0, 3.0);

// --- Asset paths ---
pub const PILL_MESH_PATH: &str = "models/pill.obj";
pub const PLANE_MESH_PATH: &str = "models/plane.obj";
pub const GRAY_MATERIAL_NAME: &str = "gray";
pub const ORANGE_MATERIAL_NAME: &str = "orange";
pub const PILL_MESH_NAME: &str = "pill";
pub const PLANE_MESH_NAME: &str = "plane";

// --- Camera -------------------------------------------------------------

pub const CAMERA_POSITION: Vector3f = Vector3f::new(-24.0, 24.0, -24.0);
pub const CAMERA_ROTATION: Vector3f = Vector3f::new(45.0, 45.0, 45.0);
pub const CAMERA_FOV: f32 = 55.0;

// -- Components -----------------------------------------------------------

define_component!(CitizenComponent {
    path_points: std::collections::VecDeque<Vector3f>,
    current_movement_speed: f32,
    max_movement_speed: f32,
    acceleration: f32,
    facing_angle: f32,
    rotation_speed: f32,
});

// -- Shared movement system -----------------------------------------------

/// Moves each citizen toward its current path point using forward-direction
/// movement with angle rotation.
///
/// The caller provides an RNG (e.g. `thread_rng()` for project mode or a seeded
/// `StdRng` for deterministic benchmarks).
///
/// - Refills the path queue with random points around [`ORIGIN`] when empty.
/// - Rotates `facing_angle` toward the target at `rotation_speed` °/s.
/// - Accelerates `current_movement_speed` up to `max_movement_speed`.
/// - Applies a speed penalty on sharp turns ([`CITIZEN_TURN_SLOWDOWN`]).
/// - Moves forward in the facing direction.
/// - Removes reached points (distance < [`POINT_REACH_THRESHOLD`]).
pub fn move_citizens_toward_targets(engine: &mut Engine, mut rng: impl Rng) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

    for (_, transform, citizen) in
        engine.iterate_two_components_mut::<TransformComponent, CitizenComponent>()?
    {
        // Refill path queue when empty
        if citizen.path_points.is_empty() {
            for _ in 0..PATH_POINT_COUNT {
                let point = Vector3f::new(
                    WORLD_ORIGIN.x + rng.gen_range(-PATH_WANDER_RADIUS_XZ..PATH_WANDER_RADIUS_XZ),
                    WORLD_ORIGIN.y,
                    WORLD_ORIGIN.z + rng.gen_range(-PATH_WANDER_RADIUS_XZ..PATH_WANDER_RADIUS_XZ),
                );
                citizen.path_points.push_back(point);
            }
        }

        // Rotate toward the current target
        let mut turn_ratio: f32 = 0.0; // 0 = no turn, 1 = max-rate turn
        if let Some(&target) = citizen.path_points.front() {
            let to_target = target - transform.position;
            let distance = to_target.length();

            // Arrived at this point — remove it
            if distance < POINT_REACH_THRESHOLD {
                citizen.path_points.pop_front();
            } else {
                let desired_angle = to_target.x.atan2(to_target.z).to_degrees();

                let mut angle_difference = desired_angle - citizen.facing_angle;
                while angle_difference > 180.0 {
                    angle_difference -= 360.0;
                }
                while angle_difference < -180.0 {
                    angle_difference += 360.0;
                }

                let maximum_rotation = citizen.rotation_speed * delta_time;
                let rotation_delta = angle_difference.clamp(-maximum_rotation, maximum_rotation);
                citizen.facing_angle = (citizen.facing_angle + rotation_delta) % 360.0;
                if citizen.facing_angle < 0.0 {
                    citizen.facing_angle += 360.0;
                }

                // Turn ratio: how hard the citizen is turning this frame (0-1)
                if maximum_rotation > 0.001 {
                    turn_ratio = (rotation_delta / maximum_rotation).abs().min(1.0);
                }

                transform.set_rotation(Vector3f::new(0.0, -citizen.facing_angle, 0.0));
            }
        }

        // Desired speed is lower while turning sharply.
        let speed_multiplier = 1.0 - CITIZEN_TURN_SLOWDOWN * turn_ratio;
        let target_speed = citizen.max_movement_speed * speed_multiplier;

        if citizen.current_movement_speed < target_speed {
            citizen.current_movement_speed = (citizen.current_movement_speed
                + citizen.acceleration * delta_time)
                .min(target_speed);
        } else {
            citizen.current_movement_speed = (citizen.current_movement_speed
                - citizen.acceleration * delta_time)
                .max(target_speed);
        }

        let angle_radians = citizen.facing_angle.to_radians();
        let forward = Vector3f::new(angle_radians.sin(), 0.0, angle_radians.cos());
        let step = forward * citizen.current_movement_speed * delta_time;
        transform.set_position(transform.position + step);
    }

    Ok(())
}
