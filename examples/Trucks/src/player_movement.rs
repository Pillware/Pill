use pill_engine::game::*;
use pill_engine::define_component;

use crate::game::{
    PlayerTagComponent,
    TargetTransformComponent,
};

// ------------ NEW  ----------------------------------
define_component!(CarControllerComponent {
    speed:      f32, // m/s along forward axis
    direction:  f32, // -1 = backward, 0 = coasting, 1 = forward
});


// ------------ TUNABLES -------------------------------
const MAX_SPEED:            f32 = 40.0;   // top speed
const ACCELERATION:         f32 = 65.0;   // units / s²
const BRAKE_DECELERATION:   f32 = 80.0;
const FRICTION:             f32 = 30.0;   // natural drag when no input
const MAX_STEER_DEG:        f32 = 85.0;   // at zero speed
const MIN_STEER_DEG:        f32 = 30.0;   // at top speed
const DRIFT_SLOWDOWN:       f32 = 0.85;   // % of speed kept each sec when turning
const CAMERA_FOV_BASE:      f32 = 60.0;
const CAMERA_FOV_BOOST:     f32 = 25.0;
const CAM_CHROM_ABB_FACTOR: f32 = 0.6;
const SMOOTHING_POS:        f32 = 6.0;
const SMOOTHING_ROT:        f32 = 5.0;
// -----------------------------------------------------

pub fn player_movement_system(engine: &mut Engine) -> Result<()> {
    let input     = engine.get_global_component::<InputComponent>()?;
    let dt        = engine.get_global_component::<TimeComponent>()?.delta_time;

    // ---------- Raw input ----------------------------
    let k_w   = input.get_key(KeyboardKey::KeyW);
    let k_s   = input.get_key(KeyboardKey::KeyS);
    let k_a   = input.get_key(KeyboardKey::KeyA);
    let k_d   = input.get_key(KeyboardKey::KeyD);
    let k_shift = input.get_key(KeyboardKey::ShiftLeft);

    const DZ: f32 = 0.1;
    let pad_fwd    =  -input.get_gamepad_axis(GamepadAxis::LeftStickY); // up = +1
    let pad_turn   =   input.get_gamepad_axis(GamepadAxis::LeftStickX); // left = -1
    let pad_throt  =   input.get_gamepad_axis(GamepadAxis::RightTrigger);
    let pad_brake  =   input.get_gamepad_axis(GamepadAxis::LeftTrigger);

    // digital fallbacks for gamepad
    let throt = if pad_throt > DZ { pad_throt } else if k_w { 1.0 } else { 0.0 };
    let brake = if pad_brake > DZ { pad_brake } else if k_s { 1.0 } else { 0.0 };
    let steer_input = if pad_turn.abs() > DZ { pad_turn } else { (k_d as i32 - k_a as i32) as f32 };

    // ---------- Loop over players --------------------
    for (_, transform, target, player_tag, car) in engine.iterate_four_components_mut::<
        TransformComponent,
        TargetTransformComponent,
        PlayerTagComponent,
        CarControllerComponent,
    >()? {
        // --- UPDATE SPEED --------------------------------------------------
        let mut speed      = car.speed;
        let mut direction  = car.direction;

        if throt > 0.0 {
            direction = 1.0;
            speed += ACCELERATION * throt * dt;
        } else if brake > 0.0 {
            direction = -1.0;
            speed -= BRAKE_DECELERATION * brake * dt;
        } else {
            // natural drag
            let drag = FRICTION * dt;
            speed = speed.signum() * f32::max(speed.abs() - drag, 0.0);
            if speed.abs() < 0.1 { direction = 0.0; }
        }

        // clamp to max
        speed = speed.clamp(-MAX_SPEED * 0.5, MAX_SPEED); // reverse slower

        // --- TURNING -------------------------------------------------------
        let speed_ratio = (speed.abs() / MAX_SPEED).clamp(0.0, 1.0);
        let steer_deg_max = MAX_STEER_DEG - (MAX_STEER_DEG - MIN_STEER_DEG) * speed_ratio;
        let steer_deg = steer_input * steer_deg_max * dt;

        if steer_input.abs() > 0.01 && speed.abs() > 0.1 {
            // yaw around global up
            target.0.rotate_around_axis(-steer_deg, Vector3f::unit_y());
            // drift slowdown
            speed *= f32::powf(DRIFT_SLOWDOWN, dt);
        }

        // --- TRANSLATION ---------------------------------------------------
        let advance = speed * dt * direction.signum();
        target.0.translate(advance.abs(), if direction >= 0.0 { Direction::Forward } else { Direction::Backward });

        // --- SMOOTH actual transform toward target ---
        transform.set_position(lerp_vec3(transform.position, target.0.position, SMOOTHING_POS * dt));
        transform.set_rotation(lerp_vec3(transform.rotation, target.0.rotation, SMOOTHING_ROT * dt));

        // write back state
        car.speed     = speed;
        car.direction = direction;
    }

    // ---------- Camera pass --------------------------
	let player_snapshot = {
		engine
			.iterate_four_components::<
				TransformComponent,
				TargetTransformComponent,
				PlayerTagComponent,
				CarControllerComponent,
			>()?
			.next()                              // first (and only) player
			.map(|(_, transform, _, _, car)| {
				let v_norm = (car.speed.abs() / MAX_SPEED).clamp(0.0, 1.0);
                let back_dir = transform.get_backward_direction();
				(transform.position, transform.rotation, back_dir, v_norm)
			})
	};
	// `player_snapshot` now owns plain values → immutable borrow over.

	// ── 2 ── mutable pass over cameras
	for (_, cam_xform, camera) in engine
			.iterate_two_components_mut::<TransformComponent, CameraComponent>()? {

		if let Some((player_pos, player_rot, back_dir, v_norm)) = player_snapshot {
			// camera juice
			let target_fov = CAMERA_FOV_BASE + v_norm * CAMERA_FOV_BOOST;
			camera.fov = lerp_f32(camera.fov, target_fov, 4.0 * dt);

			camera.postprocess_params.abberration_strength = lerp_f32(
				camera.postprocess_params.abberration_strength,
				v_norm * CAM_CHROM_ABB_FACTOR,
				6.0 * dt,
			);

			// chase-cam placement
			let back_offset = 10.0 + v_norm * 3.0;
			let height      = 6.0  + v_norm * 1.5;

             let target_pos = player_pos
                 + back_dir * back_offset
                 + Vector3f::new(0.0, height, 0.0);
			cam_xform.set_position(lerp_vec3(cam_xform.position, target_pos, SMOOTHING_POS * dt));
			cam_xform.set_rotation(lerp_vec3(cam_xform.rotation, player_rot, SMOOTHING_ROT * dt));
		}
	}

    Ok(())
}

// ----------- HELPERS (unchanged) --------------------
fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0, 1.0)
}
fn lerp_f32(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t.clamp(0.0, 1.0)
}

