//! The free flying camera: movement, look direction and field of view, in one
//! system.
//!
//! # Responsibilities
//!
//! - Fly with WASD, rise and sink with E and Q, sprint with the left Shift
//!   key, look around by dragging with the right mouse button, and widen or
//!   narrow the view with T and G ([`camera_system`]).
//!
//! # Design
//!
//! It is one system because it is one camera: a separate field-of-view system
//! would need a second query over the same rows and a second place to keep the
//! camera's limits. Both the view direction and the velocity ease towards
//! their targets over time, and the step is built from `delta_time`, so the
//! camera behaves the same at any frame rate.
//!
//! The camera turns only while the UI does not want the pointer, so dragging
//! over the in-game panel never moves the view.

// External crates
use glam::{EulerRot, Quat, Vec3};
use pill_engine::{pill_hot, system, Input, KeyCode, MouseButton, Query, Res, SystemError, Time};
use pill_master_renderer_data::{CameraComponent, TransformComponent};
use pill_project_ui::GameUi;

// Current crate
use pills_rust::CameraMovementComponent;

/// How far the view may tilt up or down, in degrees. Stopping one degree short
/// of straight up and down keeps the view from flipping over.
const PITCH_LIMIT_DEGREES: f32 = 89.0;

/// The field of view stays in this range, in degrees. The lower end is allowed
/// and the upper end is not, so 10 degrees works and 120 does not.
const FIELD_OF_VIEW_RANGE_DEGREES: std::ops::Range<f32> = 10.0..120.0;

/// Degrees the field of view changes per second while T or G is held.
const FIELD_OF_VIEW_DEGREES_PER_SECOND: f32 = 100.0;

/// Flies the camera from the keys, turns it from right button drags and widens
/// or narrows its view.
///
/// Returns `Ok` and moves nothing while the frame's `Input` or `Time` resource
/// is missing; the system never fails.
#[system(name = "camera", order = 510)]
pub(crate) fn camera_system(
    input: Res<Input>,
    game_ui: Res<GameUi>,
    time: Res<Time>,
    mut cameras: Query<(
        &mut TransformComponent,
        &mut CameraComponent,
        &mut CameraMovementComponent,
    )>,
) -> Result<(), SystemError> {
    let (Some(input), Some(time)) = (input.get(), time.get()) else {
        return Ok(());
    };

    // Step 1: Read this frame's input. The time step sets how much one frame
    // moves, and the mouse and the view keys decide what changes.
    let delta_time = time.delta_seconds();
    let ui_wants_pointer = game_ui.get().is_some_and(GameUi::wants_pointer);

    // The window does not capture the cursor, so the view turns only while the
    // right mouse button is held and the UI does not want the pointer.
    let mouse_delta = if input.mouse_button_held(MouseButton::Right) && !ui_wants_pointer {
        input.mouse_delta()
    } else {
        glam::Vec2::ZERO
    };

    // T widens the view and G narrows it. Holding both leaves it alone.
    let field_of_view_change = match (input.key_held(KeyCode::KeyT), input.key_held(KeyCode::KeyG))
    {
        (true, false) => FIELD_OF_VIEW_DEGREES_PER_SECOND,
        (false, true) => -FIELD_OF_VIEW_DEGREES_PER_SECOND,
        _ => 0.0,
    };

    // Step 2: Update every enabled camera from that input.
    for (mut transform, mut camera, mut movement) in cameras.iter_mut() {
        if !camera.enabled {
            continue;
        }

        // Moving the mouse right turns the view right, and moving it down
        // looks down. The clamp stops the pitch short of straight up and down,
        // where the view would flip over.
        movement.target_rotation[1] -= mouse_delta.x * movement.mouse_sensitivity;
        movement.target_rotation[0] -= mouse_delta.y * movement.mouse_sensitivity;
        movement.target_rotation[0] =
            movement.target_rotation[0].clamp(-PITCH_LIMIT_DEGREES, PITCH_LIMIT_DEGREES);

        // Close part of the gap to the target angle this frame. The fraction
        // comes from `delta_time`, so the view eases at the same speed at any
        // frame rate.
        let rotation_blend = 1.0 - (-movement.rotation_lerp_speed * delta_time).exp();
        for rotation_index in 0..2 {
            movement.current_rotation[rotation_index] += (movement.target_rotation[rotation_index]
                - movement.current_rotation[rotation_index])
                * rotation_blend;
        }
        // Yaw turns the view around the world's up axis, and pitch tilts it up
        // and down. With both angles at zero the camera looks down its local
        // -Z axis.
        let rotation = Quat::from_euler(
            EulerRot::YXZ,
            movement.current_rotation[1].to_radians(),
            movement.current_rotation[0].to_radians(),
            0.0,
        );
        transform.rotation = rotation.to_array();

        // Forward and right are flattened onto the horizontal plane, so WASD
        // never changes the camera's height. E and Q move straight up and
        // down.
        let forward = (rotation * Vec3::NEG_Z).with_y(0.0).normalize_or_zero();
        let right = (rotation * Vec3::X).with_y(0.0).normalize_or_zero();
        let mut direction = Vec3::ZERO;
        let is_key_held = |key| input.key_held(key);
        if is_key_held(KeyCode::KeyW) {
            direction += forward;
        }
        if is_key_held(KeyCode::KeyS) {
            direction -= forward;
        }
        if is_key_held(KeyCode::KeyD) {
            direction += right;
        }
        if is_key_held(KeyCode::KeyA) {
            direction -= right;
        }
        if is_key_held(KeyCode::KeyE) {
            direction.y += 1.0;
        }
        if is_key_held(KeyCode::KeyQ) {
            direction.y -= 1.0;
        }

        let speed = if is_key_held(KeyCode::ShiftLeft) {
            movement.move_speed * movement.sprint_multiplier
        } else {
            movement.move_speed
        };
        let target_velocity = direction.normalize_or_zero() * speed;
        movement.target_velocity = target_velocity.to_array();

        // Ease the velocity the same way, so starting and stopping are smooth
        // instead of sudden.
        let velocity_blend = 1.0 - (-movement.lerp_speed * delta_time).exp();
        let current_velocity = Vec3::from_array(movement.current_velocity);
        let current_velocity =
            current_velocity + (target_velocity - current_velocity) * velocity_blend;
        movement.current_velocity = current_velocity.to_array();

        let position = Vec3::from_array(transform.translation) + current_velocity * delta_time;
        transform.translation = position.to_array();

        let field_of_view = camera.vertical_fov + field_of_view_change * delta_time;
        if FIELD_OF_VIEW_RANGE_DEGREES.contains(&field_of_view) {
            camera.vertical_fov = field_of_view;
        }
    }
    Ok(())
}
