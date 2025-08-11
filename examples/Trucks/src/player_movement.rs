//! player_movement.rs
//! ---------------------------------------------------------------------------
//! Arcade driving with strong nitro
//!   – Expo steering, yaw-momentum drift, reverse softening
//!   – Heavy drift camera lag & body-roll
//!   – **Nitro** on RT ≥ 50 % or Left-Shift: 3.5× accel, 2× top speed, +20° FOV
//! ---------------------------------------------------------------------------

#![allow(non_snake_case)]

use pill_engine::game::*;
use pill_engine::define_component;
use crate::game::{PlayerTagComponent, TargetTransformComponent};

/*────────────────────  Per-car state  ──────────────────────────*/
define_component!(CarControllerComponent {
    speed:          f32,
    direction:      f32,
    last_steer:     f32,
    drift_yaw_vel:  f32,
});

/*────────────────────  Tunables  ───────────────────────────────*/
const MAX_SPEED:            f32 = 35.0;
const NITRO_SPEED_MULT:     f32 = 2.0;      //  2× top speed
const ACCELERATION:         f32 = 65.0;
const NITRO_ACCEL_MULT:     f32 = 3.5;      //  3.5× punch
const BRAKE_DECELERATION:   f32 = 80.0;
const FRICTION:             f32 = 30.0;

const MAX_STEER_DEG:        f32 = 105.0;
const MIN_STEER_DEG:        f32 = 40.0;
const STEER_SENSITIVITY:    f32 = 1.6;
const STEER_EXPO:           f32 = 3.0;

const HANDBRAKE_STEER_MULT: f32 = 3.0;
const HANDBRAKE_SLOW:       f32 = 0.70;
const DRIFT_SLOWDOWN:       f32 = 0.85;
const DRIFT_STEER_MIN:      f32 = 0.15;

const DRIFT_YAW_ACCEL:      f32 = 450.0;
const DRIFT_YAW_DAMP:       f32 = 3.0;
const DRIFT_ENTRY_SPEED:    f32 = 8.0;
const DRIFT_YAW_MAX:        f32 = 360.0;

const DRIFT_DRAG_COEFF:     f32 = 1.5;

/* Camera */
const CAMERA_FOV_BASE:      f32 = 60.0;
const CAMERA_FOV_BOOST:     f32 = 25.0;
const NITRO_FOV_BONUS:      f32 = 20.0;     //  huge tunnel vision
const FOV_SNAP_MAX:         f32 = 5.0;
const CAM_CHROM_ABB:        f32 = 0.6;
const SMOOTH_POS_BASE:      f32 = 6.0;
const SMOOTH_ROT_BASE:      f32 = 5.0;
const ROLL_MAX_DEG:         f32 = 8.0;
const ROT_LAG_MAX:          f32 = 4.0;
const DRIFT_LOCK_THRESH:    f32 = 0.30;
const NITRO_THRESH:         f32 = 0.50;     //  half squeeze to ignite

/*────────────────────  Helpers  ───────────────────────────────*/
fn lerp_vec3(a: Vector3f, b: Vector3f, t: f32) -> Vector3f { a + (b - a) * t.clamp(0.0, 1.0) }
fn lerp_f32 (a: f32,      b: f32,      t: f32) -> f32      { a + (b - a) * t.clamp(0.0, 1.0) }
fn normalize_vec3(v: Vector3f) -> Vector3f {
    let len_sq = v.x * v.x + v.y * v.y + v.z * v.z;
    if len_sq > 1e-6 { v / len_sq.sqrt() } else { v }
}

// NEW: flatten a direction to XZ plane (kills vertical pull-in)
#[inline]
fn flatten_xz(v: Vector3f) -> Vector3f {
    normalize_vec3(Vector3f::new(v.x, 0.0, v.z))
}

/*────────────────────  System  ───────────────────────────────*/
pub fn player_movement_system(engine: &mut Engine) -> Result<()> {
    let input = engine.get_global_component::<InputComponent>()?;
    let dt    = engine.get_global_component::<TimeComponent>()?.delta_time;

    /* ── INPUT ─────────────────────────────────────────────── */
    let (k_w,k_s,k_a,k_d)=(
        input.get_key(KeyboardKey::KeyW),
        input.get_key(KeyboardKey::KeyS),
        input.get_key(KeyboardKey::KeyA),
        input.get_key(KeyboardKey::KeyD));
    let k_shift = input.get_key(KeyboardKey::ShiftLeft);   // nitro key
    let k_hb    = input.get_key(KeyboardKey::Space);       // hand-brake

    const DZ:f32=0.1;
    let stick_y=input.get_gamepad_axis(GamepadAxis::LeftStickY);
    let stick_x=input.get_gamepad_axis(GamepadAxis::LeftStickX);
    let trig_rt=input.get_gamepad_axis(GamepadAxis::RightTrigger);
    let trig_lt=input.get_gamepad_axis(GamepadAxis::LeftTrigger);
    let pad_hb =input.get_gamepad_button(GamepadButton::B);

    let nitro = trig_rt >= NITRO_THRESH || k_shift;

    /* throttle / brake 0-1 */
    let mut throt=0.0; let mut brake=0.0;
    if trig_rt>DZ || trig_lt>DZ { throt=trig_rt; brake=trig_lt; }
    else if stick_y.abs()>DZ { if stick_y>0.0{throt=stick_y;} else {brake=-stick_y;} }
    if k_w { throt=1.0; }
    if k_s { brake=1.0; }

    /* steering input */
    let raw_steer = if stick_x.abs()>DZ { stick_x } else { (k_d as i32-k_a as i32) as f32 };
    let steer_in  = raw_steer.abs().powf(STEER_EXPO) * raw_steer.signum() * STEER_SENSITIVITY;
    let handbrake = k_hb || pad_hb;

    /* ── PHYSICS loop ─────────────────────────────────────── */
    for (_,tr,tgt,_,car) in engine.iterate_four_components_mut::<
        TransformComponent,TargetTransformComponent,
        PlayerTagComponent,CarControllerComponent>()? {

        /* === Speed & direction === */
        let mut speed=car.speed;
        let mut dir  =car.direction;

        let accel   = if nitro { ACCELERATION*NITRO_ACCEL_MULT } else { ACCELERATION };
        let max_spd = if nitro { MAX_SPEED*NITRO_SPEED_MULT   } else { MAX_SPEED };

        if throt>0.0 && dir>=0.0 {
            dir=1.0;
            speed+=accel*throt*dt;
        } else if brake>0.0 && dir<=0.0 {
            dir=-1.0;
            speed-=BRAKE_DECELERATION*brake*dt;
        } else {
            speed = speed.signum()*(speed.abs()-FRICTION*dt).max(0.0);
            if speed.abs()<0.1 { dir=0.0; }
        }
        if handbrake { speed *= f32::powf(HANDBRAKE_SLOW, dt); }
        speed = speed.clamp(-max_spd*0.5, max_spd);

        /* === Steering & grip === */
        let v_ratio = (speed.abs()/max_spd).clamp(0.0,1.0);
        let steer_lim = MAX_STEER_DEG - (MAX_STEER_DEG-MIN_STEER_DEG)*v_ratio;
        let drift_int = (car.drift_yaw_vel.abs()/DRIFT_YAW_MAX).clamp(0.0,1.0);
        let grip = (1.0-drift_int).max(DRIFT_STEER_MIN);

        let steer_deg = steer_in*steer_lim*
            if handbrake { HANDBRAKE_STEER_MULT } else { 1.0 }*
            grip*dt;

        if steer_in.abs()>0.01 && speed.abs()>0.1 && grip>0.01 {
            tgt.0.rotate_around_axis(-steer_deg, Vector3f::unit_y());
            speed *= f32::powf(DRIFT_SLOWDOWN, dt);
        }

        /* === Yaw momentum === */
        if handbrake && speed.abs()>DRIFT_ENTRY_SPEED {
            car.drift_yaw_vel += steer_in*DRIFT_YAW_ACCEL*v_ratio*dt;
        } else {
            car.drift_yaw_vel = lerp_f32(car.drift_yaw_vel,0.0,DRIFT_YAW_DAMP*dt);
        }
        car.drift_yaw_vel = car.drift_yaw_vel.clamp(-DRIFT_YAW_MAX,DRIFT_YAW_MAX);
        if car.drift_yaw_vel.abs()>0.1 {
            tgt.0.rotate_around_axis(-car.drift_yaw_vel*dt,Vector3f::unit_y());
            speed -= speed.signum()*DRIFT_DRAG_COEFF*drift_int*dt;
        }

        /* === Translation === */
        tgt.0.translate((speed*dt).abs(),
                        if dir>=0.0 {Direction::Forward} else {Direction::Backward});

        /* === Smooth to target === */
        let lag = 1.0 + v_ratio*0.8;
        tr.set_position(lerp_vec3(tr.position, tgt.0.position, SMOOTH_POS_BASE*lag*dt));
        tr.set_rotation(lerp_vec3(tr.rotation, tgt.0.rotation, SMOOTH_ROT_BASE*lag*dt));

        car.speed=speed; car.direction=dir;
    }

    /* ── CAMERA ─────────────────────────────────────────────── */
    let snap = engine.iterate_four_components::<
        TransformComponent,TargetTransformComponent,
        PlayerTagComponent,CarControllerComponent>()?
        .next()
        .map(|(_,t,_,_,c)|(
            t.position,t.rotation,t.get_backward_direction(),
            (c.speed.abs()/MAX_SPEED).clamp(0.0,1.0),
            c.last_steer,
            (c.drift_yaw_vel.abs()/DRIFT_YAW_MAX).clamp(0.0,1.0)));

    for (_,cam_tr,cam) in engine.iterate_two_components_mut::<TransformComponent,CameraComponent>()? {
        if let Some((p_pos,p_rot,car_back,v_norm,last_st,drift_int)) = snap {
            /* FOV + Nitro bonus */
            let fov_kick = ((raw_steer-last_st).abs()/dt*4.0).min(FOV_SNAP_MAX);
            let fov_bonus= if nitro { NITRO_FOV_BONUS } else { 0.0 };
            cam.fov = lerp_f32(cam.fov,
                CAMERA_FOV_BASE + v_norm*CAMERA_FOV_BOOST + fov_bonus + fov_kick,
                4.0*dt);
            /* Chromatic aberration (2× while nitro) */
            let abb_target = v_norm*CAM_CHROM_ABB * if nitro { 2.0 } else { 1.0 };
            cam.postprocess_params.abberration_strength =
                lerp_f32(cam.postprocess_params.abberration_strength,
                         abb_target, 6.0*dt);

            /* Body roll (degrees) – remove radians conversion */
            let mut target_rot = p_rot;
            target_rot.z += -raw_steer * ROLL_MAX_DEG * (1.0 + v_norm * 0.5);

            /* Drift-aware lerping */
            let k     = 1.0 - drift_int;
            let rot_t = SMOOTH_ROT_BASE * (0.2 + 0.8*k) * dt;
            let pos_t = SMOOTH_POS_BASE * (0.4 + 0.6*k) * dt;

            // FLATTEN follow direction to avoid vertical pull
            let flat_back = flatten_xz(car_back);

            // Lock zoom purely on handbrake (no v_norm leakage)
            let (dist, height) = if handbrake {
                (10.0, 6.0)
            } else {
                (10.0 + v_norm * 3.0,
                 6.0  + v_norm * 1.5)
            };

            let target_pos = p_pos
                + flat_back * dist
                + Vector3f::new(0.0, height, 0.0);

            cam_tr.set_position(lerp_vec3(cam_tr.position, target_pos, pos_t));
            cam_tr.set_rotation(lerp_vec3(cam_tr.rotation,  target_rot, rot_t));
        }
    }

    /* Save steer */
    for (_,car,_) in engine.iterate_two_components_mut::<CarControllerComponent,PlayerTagComponent>()? {
        car.last_steer = raw_steer;
    }
    Ok(())
}

