//! player_movement.rs
//! Arcade-plus driving
//!   • expo-curve steering & hand-brake drift whip
//!   • dynamic FOV kick, speed-based camera lag
//!   • NEW: camera “body roll” with steering / drift
//!   • keyboard fully supported

#![allow(non_snake_case)]

use pill_engine::game::*;
use pill_engine::define_component;

use crate::game::{PlayerTagComponent, TargetTransformComponent};

/* ───── Controller  ─────────────────────────────────────────────── */

define_component!(CarControllerComponent {
    speed:        f32,
    direction:    f32,
    last_steer:   f32,
});

/* ───── Tunables – tweak & play! ────────────────────────────────── */

const MAX_SPEED:          f32 = 40.0;
const ACCELERATION:       f32 = 65.0;
const BRAKE_DECELERATION: f32 = 80.0;
const FRICTION:           f32 = 30.0;

// steering
const MAX_STEER_DEG:      f32 = 105.0;
const MIN_STEER_DEG:      f32 = 40.0;
const STEER_SENSITIVITY:  f32 = 1.6;
const STEER_EXPO:         f32 = 3.0;   // 1=linear, 3=cubic

// drifting / hand-brake
const HANDBRAKE_STEER_MULT: f32 = 3.0;
const HANDBRAKE_SLOW:       f32 = 0.70;
const DRIFT_SLOWDOWN:       f32 = 0.85;

// camera
const CAMERA_FOV_BASE:    f32 = 60.0;
const CAMERA_FOV_BOOST:   f32 = 25.0;
const FOV_SNAP_MAX:       f32 = 5.0;
const CAM_CHROM_ABB:      f32 = 0.6;
const SMOOTH_POS_BASE:    f32 = 6.0;
const SMOOTH_ROT_BASE:    f32 = 5.0;
const ROLL_MAX_DEG:       f32 = 8.0;   // NEW body-roll amplitude

/* ───── Main system ─────────────────────────────────────────────── */

pub fn player_movement_system(engine: &mut Engine) -> Result<()> {
    let input = engine.get_global_component::<InputComponent>()?;
    let dt    = engine.get_global_component::<TimeComponent>()?.delta_time;

    /* ── Input ─────────────────────────────────────────────────── */

    let k_w  = input.get_key(KeyboardKey::KeyW);
    let k_s  = input.get_key(KeyboardKey::KeyS);
    let k_a  = input.get_key(KeyboardKey::KeyA);
    let k_d  = input.get_key(KeyboardKey::KeyD);
    let k_hb = input.get_key(KeyboardKey::Space);

    const DZ: f32 = 0.1;

    let stick_y =  input.get_gamepad_axis(GamepadAxis::LeftStickY);
    let stick_x =  input.get_gamepad_axis(GamepadAxis::LeftStickX);
    let trig_rt =  input.get_gamepad_axis(GamepadAxis::RightTrigger);
    let trig_lt =  input.get_gamepad_axis(GamepadAxis::LeftTrigger);
    let pad_hb  =  input.get_gamepad_button(GamepadButton::B);

    // throttle / brake
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

    // expo-curve steering
    let raw_steer = if stick_x.abs() > DZ {
        stick_x
    } else {
        (k_d as i32 - k_a as i32) as f32
    };
    let steer_input = raw_steer.abs().powf(STEER_EXPO) * raw_steer.signum() * STEER_SENSITIVITY;

    let handbrake = k_hb || pad_hb;

    /* ── Player loop ───────────────────────────────────────────── */

    for (_, tr, tgt, _tag, car) in engine.iterate_four_components_mut::<
        TransformComponent,
        TargetTransformComponent,
        PlayerTagComponent,
        CarControllerComponent,
    >()? {
        // speed
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
        if handbrake { speed *= f32::powf(HANDBRAKE_SLOW, dt); }
        speed = speed.clamp(-MAX_SPEED * 0.5, MAX_SPEED);

        // steering
        let v_ratio     = (speed.abs()/MAX_SPEED).clamp(0.0, 1.0);
        let steer_limit = MAX_STEER_DEG - (MAX_STEER_DEG - MIN_STEER_DEG)*v_ratio;
        let steer_deg   = steer_input
                        * steer_limit
                        * if handbrake { HANDBRAKE_STEER_MULT } else { 1.0 }
                        * dt;

        if steer_input.abs() > 0.01 && speed.abs() > 0.1 {
            tgt.0.rotate_around_axis(-steer_deg, Vector3f::unit_y());
            speed *= f32::powf(DRIFT_SLOWDOWN, dt);
        }

        // translation
        let advance = speed * dt * direction.signum();
        tgt.0.translate(
            advance.abs(),
            if direction>=0.0 {Direction::Forward} else {Direction::Backward},
        );

        // smooth to target (lag grows with speed)
        let lag = 1.0 + v_ratio*0.8;
        tr.set_position(lerp_vec3(tr.position, tgt.0.position, SMOOTH_POS_BASE*lag*dt));
        tr.set_rotation(lerp_vec3(tr.rotation, tgt.0.rotation, SMOOTH_ROT_BASE*lag*dt));

        // store state
        car.speed      = speed;
        car.direction  = direction;
    }

    /* ── Camera pass ───────────────────────────────────────────── */

    let snap = engine
        .iterate_four_components::<
            TransformComponent,
            TargetTransformComponent,
            PlayerTagComponent,
            CarControllerComponent,
        >()?
        .next()
        .map(|(_, t, _, _, c)| (
            t.position,
            t.rotation,
            t.get_backward_direction(),
            (c.speed.abs()/MAX_SPEED).clamp(0.0,1.0),
            c.last_steer,
        ));

    for (_, cam_tr, cam) in engine
        .iterate_two_components_mut::<TransformComponent, CameraComponent>()? {

        if let Some((p_pos,p_rot,back_dir,v_norm,last_steer)) = snap {
            // FOV kick on snap steer
            let steer_snap = (raw_steer - last_steer).abs() / dt;
            let fov_kick   = (steer_snap*4.0).min(FOV_SNAP_MAX);

            cam.fov = lerp_f32(
                cam.fov,
                CAMERA_FOV_BASE + v_norm*CAMERA_FOV_BOOST + fov_kick,
                4.0*dt,
            );

            cam.postprocess_params.abberration_strength = lerp_f32(
                cam.postprocess_params.abberration_strength,
                v_norm*CAM_CHROM_ABB,
                6.0*dt,
            );

            // body-roll tilt
            let roll_target = -raw_steer * ROLL_MAX_DEG * (1.0 + v_norm*0.5);
            let mut rolled_rot = p_rot;
            rolled_rot.z += roll_target.to_radians();

            // chase-cam placement
            let back_offset = 10.0 + v_norm*3.0;
            let height      = 6.0  + v_norm*1.5;
            let target_pos  = p_pos + back_dir*back_offset + Vector3f::new(0.0,height,0.0);

            let lag = 1.0 + v_norm*0.8;
            cam_tr.set_position(lerp_vec3(cam_tr.position, target_pos, SMOOTH_POS_BASE*lag*dt));
            cam_tr.set_rotation(lerp_vec3(cam_tr.rotation, rolled_rot,  SMOOTH_ROT_BASE*lag*dt));
        }
    }

    // save steer for next frame (for snap detection)
    for (_, car, _) in engine
        .iterate_two_components_mut::<CarControllerComponent, PlayerTagComponent>()? {
        car.last_steer = raw_steer;
    }

    Ok(())
}

/* ───── Helpers ─────────────────────────────────────────────────── */

fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0,1.0)
}
fn lerp_f32(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t.clamp(0.0,1.0)
}

