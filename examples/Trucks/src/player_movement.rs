use pill_engine::game::*;

use crate::game::{PlayerTagComponent, TargetTransformComponent};

pub fn player_movement_system(engine: &mut Engine) -> Result<()> {
    let input  = engine.get_global_component::<InputComponent>()?;
    let dt     = engine.get_global_component::<TimeComponent>()?.delta_time;

    // ── Keyboard input ─────────────────────────────────────────────────────────
    let w_key    = input.get_key(KeyboardKey::KeyW);
    let s_key    = input.get_key(KeyboardKey::KeyS);
    let a_key    = input.get_key(KeyboardKey::KeyA);
    let d_key    = input.get_key(KeyboardKey::KeyD);
    let shift    = input.get_key(KeyboardKey::ShiftLeft);

    // ── Game-pad input ─────────────────────────────────────────────────────────
    const DEAD_ZONE: f32 = 0.10;

    let pad_forward = input.get_gamepad_axis(GamepadAxis::LeftStickY); // up = -fwd
    let pad_turn    =  input.get_gamepad_axis(GamepadAxis::LeftStickX); // left = −
    let pad_sprint  =  input.get_gamepad_axis(GamepadAxis::RightTrigger); // 0‥1

    // treat as pressed if stick/trigger exceeds DZ
    let pad_fwd   = pad_forward  >  DEAD_ZONE;
    let pad_back  = pad_forward  < -DEAD_ZONE;
    let pad_left  = pad_turn     < -DEAD_ZONE;
    let pad_right = pad_turn     >  DEAD_ZONE;
    let pad_boost = pad_sprint   >  0.5;

    // ── Tunables ───────────────────────────────────────────────────────────────
    let move_speed      = 15.0;
    let rotate_speed    = 90.0;  // degrees / s
    let smoothing       = 2.0;
    let speed_multiplier = if shift || pad_boost { 2.0 } else { 1.0 };

    // ── Iterate over player entities ───────────────────────────────────────────
    for (_, transform, target, _) in engine.iterate_three_components_mut::<
        TransformComponent,
        TargetTransformComponent,
        PlayerTagComponent,
    >()?
    {
        // ╭───────────────── Rotation while moving ───────╮
        // only steer when entity is currently advancing or retreating
        let is_moving = w_key || s_key || pad_fwd || pad_back;
		// Figure out whether we’re going forward or backward *this frame*
		let going_back = s_key || pad_back;

		// 2. ───────────── Rotate FIRST (sign-flip if reversing) ─────────────
		if is_moving {
			let dir = if going_back { -1.0 } else { 1.0 };

			if a_key || pad_left {
				target.0.rotate_around_axis(
					dir *  rotate_speed * dt,
					Vector3f::new(0.0, 1.0, 0.0),
				);
			}
			if d_key || pad_right {
				target.0.rotate_around_axis(
					dir * -rotate_speed * dt,
					Vector3f::new(0.0, 1.0, 0.0),
				);
			}
		}

		// 3. ───────────── Translate AFTER the heading is updated ────────────
		if w_key || pad_fwd {
			target.0.translate(
				move_speed * speed_multiplier * dt,
				Direction::Forward,
			);
		}
		if going_back {
			target.0.translate(
				move_speed * speed_multiplier * dt,
				Direction::Backward,
			);
		}

        // ╭───────────────── Smooth to target ─────────────╮
        transform.set_position(lerp_vec3(
            transform.position,
            target.0.position,
            smoothing * dt,
        ));
        transform.set_rotation(lerp_vec3(
            transform.rotation,
            target.0.rotation,
            smoothing * dt,
        ));
    }

    Ok(())
}

fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0, 1.0)
}

