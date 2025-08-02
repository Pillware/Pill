use pill_engine::game::*;

pub fn free_camera_system(engine: &mut Engine) -> Result<()> {
    let input_component = engine.get_global_component::<InputComponent>()?;
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

    // Movement keys
    let w_key = input_component.get_key(KeyboardKey::KeyW);
    let s_key = input_component.get_key(KeyboardKey::KeyS);
    let a_key = input_component.get_key(KeyboardKey::KeyA);
    let d_key = input_component.get_key(KeyboardKey::KeyD);
    let q_key = input_component.get_key(KeyboardKey::KeyQ);
    let e_key = input_component.get_key(KeyboardKey::KeyE);

    // Mouse movement
    let mouse_delta = input_component.get_mouse_delta();

    // Constants
    let move_speed = 7.0;
    let mouse_sensitivity = 0.5;

    for (_, camera_component, transform_component) in engine.iterate_two_components_mut::<CameraComponent, TransformComponent>()? {
        // --- Mouse look ---
        let mut rotation = transform_component.rotation;
        rotation.y += -mouse_delta.x * mouse_sensitivity;
        rotation.x -= mouse_delta.y * mouse_sensitivity;
        rotation.x = rotation.x.clamp(-89.9, 89.9);
        transform_component.set_rotation(rotation);

        // --- Movement ---
        if w_key {
            transform_component.translate(move_speed * delta_time, Direction::Forward);
        }

        if s_key {
            transform_component.translate(move_speed * delta_time, Direction::Backward);
        }

        if d_key {
            transform_component.translate(move_speed * delta_time, Direction::Right);
        }

        if a_key {
            transform_component.translate(move_speed * delta_time, Direction::Left);
        }

        if e_key {
            transform_component.translate(move_speed * delta_time, Direction::WorldUp);
        }

        if q_key {
            transform_component.translate(move_speed * delta_time, Direction::WorldDown);
        }
    }

    Ok(())
}
