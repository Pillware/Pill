use pill_engine::game::*;

use crate::game::{PlayerTagComponent, TargetTransformComponent};

pub fn player_movement_system(engine: &mut Engine) -> Result<()> {
    let input_component = engine.get_global_component::<InputComponent>()?;
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

    let w_key = input_component.get_key(KeyboardKey::KeyW);
    let s_key = input_component.get_key(KeyboardKey::KeyS);
    let a_key = input_component.get_key(KeyboardKey::KeyA);
    let d_key = input_component.get_key(KeyboardKey::KeyD);
    let shift_key = input_component.get_key(KeyboardKey::ShiftLeft);

    let move_speed = 15.0;
    let rotate_speed = 90.0; // degrees per second
    let smoothing = 2.0;
    let move_speed_boost = if shift_key { 2.0 } else { 1.0 };

    for (_, transform_component, target_transform_component, player_tag_component) in 
        engine.iterate_three_components_mut::<TransformComponent, TargetTransformComponent, PlayerTagComponent>()? {

        // --- Input updates target transform ---

        if w_key {
            target_transform_component.0.translate(move_speed * move_speed_boost * delta_time, Direction::Forward);
        }
        if s_key {
            target_transform_component.0.translate(move_speed * move_speed_boost * delta_time, Direction::Backward);
        }

        if a_key && (w_key || s_key) {
            target_transform_component.0.rotate_around_axis(rotate_speed * delta_time, Vector3f::new(0.0, 1.0, 0.0));
        }
        if d_key && (w_key || s_key){
            target_transform_component.0.rotate_around_axis(-rotate_speed * delta_time, Vector3f::new(0.0, 1.0, 0.0));
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
