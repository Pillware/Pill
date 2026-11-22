//! Ball integration, collisions, and spawn states.
//!
//! # Responsibilities
//!
//! - Step every ball by the frame delta under [`GRAVITY`].
//! - Resolve floor, ceiling and wall contacts with [`RESTITUTION`], keeping
//!   weak floor bounces alive via [`MINIMUM_BOUNCE_VELOCITY_Y`].
//! - Seed each ball's spawn state and copy the result into the mesh
//!   transform.

use crate::settings::*;
use crate::{PhysicsState, SimulationTime};
use pill_engine::*;
use pill_rendering::TransformComponent;

impl Default for PhysicsState {
    /// The first ball's spawn state, for callers that need any valid state
    /// rather than a particular one.
    fn default() -> Self {
        ball_spawn_state(0)
    }
}

/// Advances one ball by its own delta and bounces it off the box.
///
/// Public so a hot patch of `physics_system` can call it without the patch
/// having to duplicate the physics constants.
pub fn simulate_ball(state: &mut PhysicsState) {
    if !state.active {
        return;
    }

    let delta = state.delta_time.clamp(0.0, 0.1);
    state.velocity_y += GRAVITY * delta;
    state.position_x += state.velocity_x * delta;
    state.position_y += state.velocity_y * delta;

    if state.position_y + state.radius >= FLOOR_Y {
        state.position_y = FLOOR_Y - state.radius;
        state.velocity_y = -state.velocity_y.abs() * RESTITUTION;
        // Restitution alone decays each bounce towards rest; restore a
        // minimum upward speed so balls bounce forever.
        if state.velocity_y.abs() < MINIMUM_BOUNCE_VELOCITY_Y {
            state.velocity_y = -MINIMUM_BOUNCE_VELOCITY_Y;
        }
    }
    if state.position_y - state.radius <= CEILING_Y {
        state.position_y = CEILING_Y + state.radius;
        state.velocity_y = state.velocity_y.abs() * RESTITUTION;
    }
    if state.position_x - state.radius <= LEFT_WALL {
        state.position_x = LEFT_WALL + state.radius;
        state.velocity_x = state.velocity_x.abs() * RESTITUTION;
    }
    if state.position_x + state.radius >= RIGHT_WALL {
        state.position_x = RIGHT_WALL - state.radius;
        state.velocity_x = -state.velocity_x.abs() * RESTITUTION;
    }
}

/// Steps every ball by the frame delta and copies its state into its mesh.
///
/// # Errors
///
/// Returns [`SystemError::MissingResource`] when `SimulationTime` is absent.
#[system(name = "ball_physics", order = 520)]
pub(crate) fn physics_system(
    mut time: ResMut<SimulationTime>,
    mut query: Query<(&mut PhysicsState, &mut TransformComponent)>,
) -> Result<(), SystemError> {
    let Some(time) = time.get_mut() else {
        return Err(SystemError::MissingResource {
            name: String::from("SimulationTime"),
        });
    };

    let delta_seconds = time.delta_seconds;
    for (mut physics, mut transform) in query.iter_mut() {
        physics.delta_time = delta_seconds;
        simulate_ball(&mut physics);

        // Physics coordinates are the ball's centre in the project's
        // 800x600 pixel space; the transform places the mesh from them.
        transform.translation = [
            (physics.position_x - 400.0) / 80.0,
            (300.0 - physics.position_y) / 80.0,
            0.0,
        ];
        transform.scale = [physics.radius / 80.0; 3];
    }
    Ok(())
}

/// Physics state for the `index`-th ball in the spawn sequence.
///
/// Balls line up across the play area with alternating travel direction and a
/// slightly different launch speed each, so they spread out over the box
/// instead of crossing it as one block.
pub(crate) fn ball_spawn_state(index: usize) -> PhysicsState {
    PhysicsState {
        delta_time: FIXED_DELTA_TIME,
        position_x: 90.0 + index as f32 * 150.0,
        position_y: 120.0,
        velocity_x: if index.is_multiple_of(2) {
            BOUNCE_VELOCITY_X
        } else {
            -BOUNCE_VELOCITY_X
        },
        velocity_y: BOUNCE_VELOCITY_Y - index as f32 * 25.0,
        radius: 10.0 + (index % 4) as f32 * 2.0,
        active: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ball_bounces_off_floor() {
        let mut state = ball_spawn_state(0);
        state.position_y = FLOOR_Y - state.radius;
        state.velocity_y = 100.0;

        simulate_ball(&mut state);

        assert_eq!(state.position_y, FLOOR_Y - state.radius);
        assert!(state.velocity_y < 0.0);
    }

    #[test]
    fn inactive_ball_does_not_move() {
        let mut state = ball_spawn_state(0);
        state.active = false;
        let before = state;

        simulate_ball(&mut state);

        assert_eq!(state.position_x, before.position_x);
        assert_eq!(state.position_y, before.position_y);
        assert_eq!(state.velocity_x, before.velocity_x);
        assert_eq!(state.velocity_y, before.velocity_y);
    }

    /// A weak floor bounce is restored so balls never settle on the floor.
    #[test]
    fn ball_never_rests_on_the_floor() {
        let mut state = ball_spawn_state(0);
        state.position_y = FLOOR_Y - state.radius;
        state.velocity_y = 5.0;

        simulate_ball(&mut state);

        assert!(state.velocity_y <= -MINIMUM_BOUNCE_VELOCITY_Y);
    }
}
