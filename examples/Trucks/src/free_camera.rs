use pill_engine::game::*;

use crate::game::TargetTransformComponent;

pub fn free_camera_system(engine: &mut Engine) -> Result<()> {
    let input_component = engine.get_global_component::<InputComponent>()?;
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

    let w_key = input_component.get_key(KeyboardKey::KeyW);
    let s_key = input_component.get_key(KeyboardKey::KeyS);
    let a_key = input_component.get_key(KeyboardKey::KeyA);
    let d_key = input_component.get_key(KeyboardKey::KeyD);
    let q_key = input_component.get_key(KeyboardKey::KeyQ);
    let e_key = input_component.get_key(KeyboardKey::KeyE);
    let shift_key = input_component.get_key(KeyboardKey::ShiftLeft);

    let mouse_delta = input_component.get_mouse_delta();

    let move_speed = 15.0;
    let mouse_sensitivity = 1.5;
    let smoothing = 10.0;
    let move_speed_boost = if shift_key { 2.0 } else { 1.0 };

    for (_, camera_component, transform_component, target_transform_component) in 
        engine.iterate_three_components_mut::<CameraComponent, TransformComponent, TargetTransformComponent>()? {
       
        // --- Input updates target transform ---
        let mut rotation = target_transform_component.0.rotation;
        rotation.y += -mouse_delta.x * mouse_sensitivity;
        rotation.x -= mouse_delta.y * mouse_sensitivity;
        rotation.x = rotation.x.clamp(-89.9, 89.9);
        target_transform_component.0.set_rotation(rotation);

        if w_key {
            target_transform_component.0.translate(move_speed * move_speed_boost * delta_time, Direction::Forward);
        }
        if s_key {
            target_transform_component.0.translate(move_speed * move_speed_boost * delta_time, Direction::Backward);
        }
        if d_key {
            target_transform_component.0.translate(move_speed * move_speed_boost * delta_time, Direction::Right);
        }
        if a_key {
            target_transform_component.0.translate(move_speed * move_speed_boost * delta_time, Direction::Left);
        }
        if e_key {
            target_transform_component.0.translate(move_speed * move_speed_boost * delta_time, Direction::WorldUp);
        }
        if q_key {
            target_transform_component.0.translate(move_speed * move_speed_boost * delta_time, Direction::WorldDown);
        }

        // --- Smooth actual transform toward target ---
        transform_component.set_position(lerp_vec3(transform_component.position, target_transform_component.0.position, smoothing * delta_time));
        transform_component.set_rotation(lerp_vec3(transform_component.rotation, target_transform_component.0.rotation, smoothing * delta_time));
    }

    Ok(())
}

fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0, 1.0)
}
