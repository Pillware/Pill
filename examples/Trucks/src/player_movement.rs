//! player_movement.rs
//! Smooth “pseudo-physics” driving +
//! • snappier steering
//! • keyboard + game-pad handbrake for effortless drifts

use pill_engine::game::*;
use pill_engine::define_component;

use crate::game::{PlayerTagComponent, TargetTransformComponent};

define_component!(CarControllerComponent {
    speed:     f32, // m/s along forward axis
    direction: f32, // -1 = backward, 0 = coasting, 1 = forward
});

// ───── Tunables ──────────────────────────────────────────────────────
const MAX_SPEED:            f32 = 40.0;
const ACCELERATION:         f32 = 65.0;
const BRAKE_DECELERATION:   f32 = 80.0;
const FRICTION:             f32 = 30.0;

// ► steering tweaks
const MAX_STEER_DEG:        f32 = 100.0;   // was 85 – more lock at low speed
const MIN_STEER_DEG:        f32 = 35.0;    // was 30 – keep control at high speed
const STEER_SENSITIVITY:    f32 = 1.4;     // overall multiplier

// ► drifting / hand-brake
const HANDBRAKE_STEER_MULT: f32 = 2.2;     // extra angle when engaged
const HANDBRAKE_SLOW:       f32 = 0.55;    // per-second speed retention (<1 = slows down)
const DRIFT_SLOWDOWN:       f32 = 0.85;    // normal turn bleed

// ► camera juice
const CAMERA_FOV_BASE:      f32 = 60.0;
const CAMERA_FOV_BOOST:     f32 = 25.0;
const CAM_CHROM_ABB_FACTOR: f32 = 0.6;

// ► smoothing
const SMOOTHING_POS:        f32 = 6.0;
const SMOOTHING_ROT:        f32 = 5.0;
// ─────────────────────────────────────────────────────────────────────

pub fn player_movement_system(engine: &mut Engine) -> Result<()> {
    let input = engine.get_global_component::<InputComponent>()?;
    let dt    = engine.get_global_component::<TimeComponent>()?.delta_time;

    // ───── Input (keyboard + game-pad) ──────────────────────────────
    let k_w   = input.get_key(KeyboardKey::KeyW);
    let k_s   = input.get_key(KeyboardKey::KeyS);
    let k_a   = input.get_key(KeyboardKey::KeyA);
    let k_d   = input.get_key(KeyboardKey::KeyD);
    let k_hb  = input.get_key(KeyboardKey::Space);            // hand-brake key

    const DZ: f32 = 0.1;

    // Game-pad axes & buttons
    let stick_y = input.get_gamepad_axis(GamepadAxis::LeftStickY); // up = +1
    let stick_x =  input.get_gamepad_axis(GamepadAxis::LeftStickX); // left = -1
    let trig_rt =  input.get_gamepad_axis(GamepadAxis::RightTrigger);
    let trig_lt =  input.get_gamepad_axis(GamepadAxis::LeftTrigger);
    let pad_hb  = input.get_gamepad_button(GamepadButton::B);    // B / Circle

    // Throttle & brake (triggers → stick Y → keyboard)
    let mut throt = 0.0;
    let mut brake = 0.0;

    if trig_rt > DZ || trig_lt > DZ {
        throt = trig_rt;
        brake = trig_lt;
    } else if stick_y.abs() > DZ {
        if stick_y > 0.0 { throt = stick_y; } else { brake = -stick_y; }
    }

    if k_w { throt = 1.0; }
    if k_s { brake = 1.0; }

    // Steering (stick X → keys)
    let steer_input = if stick_x.abs() > DZ {
        stick_x
    } else {
        (k_d as i32 - k_a as i32) as f32
    } * STEER_SENSITIVITY;

    // Hand-brake flag
    let handbrake = k_hb || pad_hb;
    // ───────────────────────────────────────────────────────────────

    // ───── Player entities ─────────────────────────────────────────
    for (_, transform, target, _player_tag, car) in engine.iterate_four_components_mut::<
        TransformComponent,
        TargetTransformComponent,
        PlayerTagComponent,
        CarControllerComponent,
    >()? {
        // speed / direction
        let mut speed     = car.speed;
        let mut direction = car.direction;

        if throt > 0.0 {
            direction = 1.0;
            speed += ACCELERATION * throt * dt;
        } else if brake > 0.0 {
            direction = -1.0;
            speed -= BRAKE_DECELERATION * brake * dt;
        } else {
            let drag = FRICTION * dt;
            speed = speed.signum() * (speed.abs() - drag).max(0.0);
            if speed.abs() < 0.1 { direction = 0.0; }
        }

        // extra slowdown while sliding on the hand-brake
        if handbrake {
            speed *= f32::powf(HANDBRAKE_SLOW, dt);
        }

        speed = speed.clamp(-MAX_SPEED * 0.5, MAX_SPEED);

        // steering
        let speed_ratio   = (speed.abs() / MAX_SPEED).clamp(0.0, 1.0);
        let steer_deg_max = MAX_STEER_DEG - (MAX_STEER_DEG - MIN_STEER_DEG) * speed_ratio;
        let steer_deg     = steer_input
            * steer_deg_max
            * if handbrake { HANDBRAKE_STEER_MULT } else { 1.0 }
            * dt;

        if steer_input.abs() > 0.01 && speed.abs() > 0.1 {
            target.0.rotate_around_axis(-steer_deg, Vector3f::unit_y());
            speed *= f32::powf(DRIFT_SLOWDOWN, dt); // bleed some speed on any turn
        }

        // forward / reverse translation
        let advance = speed * dt * direction.signum();
        target.0.translate(
            advance.abs(),
            if direction >= 0.0 { Direction::Forward } else { Direction::Backward },
        );

        // smooth toward target
        transform.set_position(lerp_vec3(
            transform.position,
            target.0.position,
            SMOOTHING_POS * dt,
        ));
        transform.set_rotation(lerp_vec3(
            transform.rotation,
            target.0.rotation,
            SMOOTHING_ROT * dt,
        ));

        car.speed     = speed;
        car.direction = direction;
    }
    // ───────────────────────────────────────────────────────────────

    // ───── Camera pass ─────────────────────────────────────────────
    let player_snapshot = engine
        .iterate_four_components::<
            TransformComponent,
            TargetTransformComponent,
            PlayerTagComponent,
            CarControllerComponent,
        >()?
        .next()
        .map(|(_, transform, _, _, car)| {
            (
                transform.position,
                transform.rotation,
                transform.get_backward_direction(),
                (car.speed.abs() / MAX_SPEED).clamp(0.0, 1.0),
            )
        });

    for (_, cam_xform, camera) in engine
        .iterate_two_components_mut::<TransformComponent, CameraComponent>()? {

        if let Some((player_pos, player_rot, back_dir, v_norm)) = player_snapshot {
            let target_fov = CAMERA_FOV_BASE + v_norm * CAMERA_FOV_BOOST;
            camera.fov = lerp_f32(camera.fov, target_fov, 4.0 * dt);

            camera.postprocess_params.abberration_strength = lerp_f32(
                camera.postprocess_params.abberration_strength,
                v_norm * CAM_CHROM_ABB_FACTOR,
                6.0 * dt,
            );

            let back_offset = 10.0 + v_norm * 3.0;
            let height      = 6.0  + v_norm * 1.5;

            let target_pos = player_pos
                + back_dir * back_offset
                + Vector3f::new(0.0, height, 0.0);

            cam_xform.set_position(lerp_vec3(cam_xform.position, target_pos, SMOOTHING_POS * dt));
            cam_xform.set_rotation(lerp_vec3(cam_xform.rotation, player_rot, SMOOTHING_ROT * dt));
        }
    }
    // ───────────────────────────────────────────────────────────────

    Ok(())
}

// ───── Helpers ─────────────────────────────────────────────────────
fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0, 1.0)
}
fn lerp_f32(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t.clamp(0.0, 1.0)
}

