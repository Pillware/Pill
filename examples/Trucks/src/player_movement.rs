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

    let any_key = w_key || s_key || a_key || d_key;

    let move_speed = 25.0;
    let rotate_speed = 90.0; // degrees per second
    let smoothing = if any_key { 2.0 } else { 2.3 };
    let move_speed_boost = if shift_key { 2.0 } else { 1.0 };
    //Slow down on turns
    let move_speed_slow_down = if a_key || d_key { 0.8 } else { 1.0 };

    
    

    let mut player_transform: Option<TransformComponent> = None;

    let final_speed = move_speed_slow_down * move_speed * move_speed_boost * delta_time;

    for (_, transform_component, target_transform_component, player_tag_component) in 
        engine.iterate_three_components_mut::<TransformComponent, TargetTransformComponent, PlayerTagComponent>()? {

        // --- Input updates target transform ---
        if w_key {
            target_transform_component.0.translate(final_speed, Direction::Forward);
        }
        if s_key {
            target_transform_component.0.translate(final_speed, Direction::Backward);
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
    
        player_transform = Some(transform_component.clone());
    }

    for (_, camera_transform_component, camera_component) in 
        engine.iterate_two_components_mut::<TransformComponent, CameraComponent>()? {

            
            
        let mut in_move: f32 = 0.0;
        in_move += if w_key || s_key { 1.0 } else { 0.0 };
        in_move += if w_key || s_key { 0.2 } else { 0.0 };
        in_move += if (a_key || d_key  ) && (w_key || s_key) { -0.5 } else { 0.0 };
        in_move = in_move.clamp(0.0, 1.0);

        let aa = in_move * 0.4 * final_speed;

        // Update camera post-process parameters based on speed
        camera_component.postprocess_params.abberration_strength = lerp_f32(
            camera_component.postprocess_params.abberration_strength,
            aa , // Adjust the factor as needed
            6.0 * delta_time
        );

        camera_component.fov = lerp_f32(
            camera_component.fov,
            60.0 + aa * 20.0, // Adjust the factor as needed
            3.0 * delta_time
        );



        if let Some(player_transform) = player_transform {
            let target_position = (player_transform.position + player_transform.get_backward_direction() * 10.0) + Vector3f::new(0.0, 6.0, 0.0);
            camera_transform_component.set_position(lerp_vec3(camera_transform_component.position, target_position, smoothing * delta_time));
            camera_transform_component.set_rotation(lerp_vec3(camera_transform_component.rotation, player_transform.rotation, smoothing * delta_time));
        }
    }

    Ok(())
}

fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0, 1.0)
}
 
 // lerp one parameter
fn lerp_f32(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t.clamp(0.0, 1.0)
}
