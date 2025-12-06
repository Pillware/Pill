use pill_engine::{define_component, define_global_component, game::*};
use rand::{thread_rng, Rng};

use crate::resources::{create_resources};
use crate::curl_noise_system::{curl_noise_system};

pub const FLOATING_OBJECT_SPAWN_BATCH_COUNT: usize = 100000;
pub const FLOATING_OBJECT_REMOVE_BATCH_COUNT: usize = 10;
pub const SPAWN_FLOATING_OBJECTS_BUTTON: KeyboardKey = KeyboardKey::KeyO;
pub const REMOVE_FLOATING_OBJECTS_BUTTON: KeyboardKey = KeyboardKey::KeyL;
pub const TOGGLE_FLOATING_OBJECTS_SYSTEM: KeyboardKey = KeyboardKey::KeyI;
pub const FLOATING_OBJECTS_CHANGE_MESH_BUTTON: KeyboardKey = KeyboardKey::KeyN;
pub const FLOATING_OBJECTS_CHANGE_MATERIAL_BUTTON: KeyboardKey = KeyboardKey::KeyM;
pub const INCREASE_CAMERA_FOV_BUTTON: KeyboardKey = KeyboardKey::KeyT;
pub const DECREASE_CAMERA_FOV_BUTTON: KeyboardKey = KeyboardKey::KeyG;

define_component!(FloatingObjectComponent {
    angle: f32,
    radius_factor: f32,
    scale_factor: f32,
    y_axis_factor: f32,

    orbital_movement_speed: f32,
    y_axis_movement_speed: f32,
    rotation_speed: f32,
    scale_speed: f32,
    radius_speed: f32,
});

define_global_component!(DemoStateComponent {
    floating_objects_movement_enabled: bool,
    curl_scale: f32,
    curl_epsilon: f32,
    curl_attraction: f32,
    curl_damping: f32,
});

// Curl noise component tag
define_component!(CurlNoiseComponent {
    velocity: Vector3f,
    curl_strength: f32,
    noise_scale: f32,
});

define_component!(CameraMovementComponent {
    // Movement settings
    move_speed: f32,
    sprint_multiplier: f32,
    lerp_speed: f32,
    
    // Rotation settings
    mouse_sensitivity: f32,
    rotation_lerp_speed: f32,
    
    // Current velocity (for lerping)
    current_velocity: Vector3f,
    target_velocity: Vector3f,
    
    // Current rotation (for lerping)
    current_rotation: Vector3f,
    target_rotation: Vector3f,
});

pub struct Game {}

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // --- Basic setup ---

        // Disable build-in audio system
        engine.toggle_system("audio_system", UpdatePhase::PostGame, false)?;

        // Create scene
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<AudioListenerComponent>(active_scene)?;
        engine.register_component::<AudioSourceComponent>(active_scene)?;
        engine.register_component::<CameraMovementComponent>(active_scene)?;
        engine.register_component::<FloatingObjectComponent>(active_scene)?;
        engine.register_component::<CurlNoiseComponent>(active_scene)?;


        // Add systems
        //engine.add_system("spawn_floating_objects", floating_objects_spawn_system)?;
       // engine.add_system("delete_floating_objects", floating_objects_remove_system)?;
        engine.add_system("fps_camera", fps_camera_system)?;
        engine.add_system("curl_noise", curl_noise_system)?;
        //engine.add_system("objects_movement", floating_objects_movement_system)?;
        //engine.add_system("camera_movement", camera_movement_system)?;
        engine.add_system("camera_fov", camera_fov_changing_system)?;
        //engine.add_system("mesh_changing", object_appearance_changing_system)?;
        engine.add_system("demo_control", demo_control_system)?;

        // --- Create resources ---

        // Add meshes
        create_resources(engine)?;

        // Add sounds
        let ambient_music = Sound::new("ambient", "audio/test_music.mp3".into());
        let ambient_music_handle = engine.add_resource(ambient_music)?;

        // --- Create entities ---

        // Create ambient music player entity
        engine
            .build_entity(active_scene)
            .with_component(
                AudioSourceComponent::builder()
                    .sound_type(SoundType::Sound2D)
                    .sound(ambient_music_handle)
                    .volume(0.05)
                    .play_on_awake(false)
                    .build(),
            )
            .build();

        // Create camera entity (FPS style)
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 1.6, -10.0)) // Eye height ~1.6m
                    .rotation(Vector3f::new(0.0, 0.0, 0.0))
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(75.0) // Wider FOV for FPS feel
                    .clear_color(Color::new(0.35, 0.40, 0.50))
                    .build(),
            )
            .with_component(CameraMovementComponent {
                move_speed: 25.0,
                sprint_multiplier: 2.0,
                lerp_speed: 8.0,
                mouse_sensitivity: 0.1,
                rotation_lerp_speed: 25.0,
                current_velocity: Vector3f::new(0.0, 0.0, 0.0),
                target_velocity: Vector3f::new(0.0, 0.0, 0.0),
                current_rotation: Vector3f::new(0.0, 0.0, 0.0),
                target_rotation: Vector3f::new(0.0, 0.0, 0.0),
            })
            .with_component(AudioListenerComponent::builder().enabled(true).build())
            .build();

        // Setup demo state component
        let demo_state = DemoStateComponent {
            floating_objects_movement_enabled: true,
            curl_epsilon: 0.0038,
            curl_scale: 0.1005,
            curl_attraction: 135.0,
            curl_damping:2.91,
        };
        engine.add_global_component(demo_state)?;

        // for velocity not divided by 3.0
// curl_epsilon: 0.0038,
//             curl_scale: 0.036,
//             curl_attraction: 260.0,
//             curl_damping: 0.99,
        // Spawn certain number of floating objects
        //spawn_floating_objects(engine, FLOATING_OBJECT_SPAWN_BATCH_COUNT)?;

        spawn_level(engine)?;

        Ok(())
    }
}

// --- Systems ---

// fn demo_control_system(engine: &mut Engine) -> Result<()> {
//     let input_component = engine.get_global_component::<InputComponent>()?;
//     let system_toggle_key = input_component.get_key_pressed(TOGGLE_FLOATING_OBJECTS_SYSTEM);

//     let demo_state = engine.get_global_component_mut::<DemoStateComponent>()?;
//     if system_toggle_key {
//         demo_state.floating_objects_movement_enabled =
//             !demo_state.floating_objects_movement_enabled;
//         let enabled = demo_state.floating_objects_movement_enabled;
//         engine.toggle_system("objects_movement", UpdatePhase::Game, enabled)?;
//     }

//     Ok(())
// }

fn floating_objects_movement_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

    for (_, floating_object_transform, floating_object_component) in
        engine.iterate_two_components_mut::<TransformComponent, FloatingObjectComponent>()?
    {
        // Local rotation
        let rotation_speed = floating_object_component.rotation_speed.clone();
        floating_object_transform
            .rotate_around_axis(rotation_speed * delta_time, Vector3f::new(1.0, 1.0, 1.0));

        // Local scale
        let scale_speed = floating_object_component.scale_speed.clone();
        floating_object_component.scale_factor += scale_speed * delta_time;
        let scale_factor = floating_object_component.scale_factor.clone();
        floating_object_transform
            .set_scale(Vector3f::new(0.4, 0.4, 0.4) * (scale_factor.sin() / 1.5 + 1.5));

        // Radius
        let radius_speed = floating_object_component.radius_speed.clone();
        floating_object_component.radius_factor += radius_speed * delta_time;

        // Movement
        let orbital_movement_speed = floating_object_component.orbital_movement_speed.clone();
        floating_object_component.angle += orbital_movement_speed * delta_time;

        let angle = floating_object_component.angle.clone();
        let radius = floating_object_component.radius_factor.clone().sin() * 6.0 + 10.0;

        floating_object_transform.set_position(Vector3f::new(
            angle.to_radians().cos() * radius,
            floating_object_transform.position.y,
            angle.to_radians().sin() * radius,
        ));

        let y_axis_movement_speed = floating_object_component.y_axis_movement_speed.clone();
        floating_object_component.y_axis_factor += y_axis_movement_speed * delta_time;
        let y_axis_factor = floating_object_component.y_axis_factor.clone();

        floating_object_transform.set_position(Vector3f::new(
            angle.to_radians().cos() * radius,
            y_axis_factor.sin() * 0.8 * radius,
            angle.to_radians().sin() * radius,
        ));
    }

    Ok(())
}


fn camera_fov_changing_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component::<InputComponent>()?;

    // Get input
    let t_key = input_component.get_key(INCREASE_CAMERA_FOV_BUTTON);
    let g_key = input_component.get_key(DECREASE_CAMERA_FOV_BUTTON);

    for (_, camera_component) in engine.iterate_one_component_mut::<CameraComponent>()? {
        let mut change_value: f32 = 0.0;
        if t_key {
            change_value += 1.0;
        }
        if g_key {
            change_value -= 1.0;
        }

        let new_fov = camera_component.fov + change_value * 100.0 * delta_time;
        if new_fov > 10.0 && new_fov < 120.0 {
            camera_component.fov = new_fov;
        }
    }

    Ok(())
}

fn floating_objects_spawn_system(engine: &mut Engine) -> Result<()> {
    // Get input component
    let input_component = (&*engine).get_global_component::<InputComponent>()?;

    // Create new objects
    if input_component.get_key_pressed(SPAWN_FLOATING_OBJECTS_BUTTON) {
        spawn_floating_objects(engine, FLOATING_OBJECT_SPAWN_BATCH_COUNT)?;
    }

    Ok(())
}

fn floating_objects_remove_system(engine: &mut Engine) -> Result<()> {
    let mut count = FLOATING_OBJECT_REMOVE_BATCH_COUNT;

    // Get active scene handle
    let scene_handle = engine.get_active_scene_handle()?;

    // Get input component
    let input_component = (&*engine).get_global_component::<InputComponent>()?;

    // Remove objects
    if input_component.get_key_pressed(REMOVE_FLOATING_OBJECTS_BUTTON) {
        let mut entities_for_deletion = Vec::<EntityHandle>::new();

        for (entity_handle, _) in engine.iterate_one_component::<FloatingObjectComponent>()? {
            if count == 0 {
                break;
            }
            entities_for_deletion.push(entity_handle);
            count -= 1;
        }

        for entity_handle in entities_for_deletion.iter() {
            engine.remove_entity(*entity_handle, scene_handle)?;
        }
    }

    Ok(())
}

// --- Functions ---

fn spawn_floating_objects(engine: &mut Engine, object_count: usize) -> Result<()> {
    // Get active scene handle
    let active_scene = engine.get_active_scene_handle()?;
    let mut rng = thread_rng();

    // Get resources
    let demo_state = (&*engine).get_global_component::<DemoStateComponent>()?;

    for _ in 0..object_count {
        let mesh_handle = engine.get_resource_handle::<Mesh>("pill")?;
        let material_handle: PBRMaterialHandle = engine.get_resource_handle::<PBRMaterial>("dark")?;

        engine
            .build_entity(active_scene)
            .with_component(CurlNoiseComponent {
                velocity: Vector3f::new(0.0, 0.0, 0.0),
                curl_strength: rng.gen_range(70.0..100.0),
                noise_scale: rng.gen_range(0.1..0.2),
            })
            .with_component(TransformComponent::builder()
                .position(Vector3f::new(
                    rng.gen_range(-20.0..20.0),
                    rng.gen_range(0.0..40.0),
                    rng.gen_range(-20.0..20.0),
                ))
                .rotation(Vector3f::new(
                    rng.gen_range(0.0..360.0),
                    rng.gen_range(0.0..360.0),
                    rng.gen_range(0.0..360.0),
                ))
                .scale(Vector3f::new(0.3, 0.3, 0.3))// * rng.gen_range(0.2..1.5))
                .build())
            .with_component(
                MeshRenderingComponent::builder()
                    .material(&material_handle)
                    .mesh(&mesh_handle)
                    .build(),
            )
            .build();
    }

    // Update initial positions once (in case movement system is disabled)
    floating_objects_movement_system(engine)?;

    Ok(())
}

fn spawn_level(engine: &mut Engine) -> Result<()> {
    // Get active scene handle
    let scene_handle = engine.get_active_scene_handle()?;

    // Spawn pillars
    let pillars_mesh_handle = engine.get_resource_handle::<Mesh>("pillars")?;
    let pillars_material_handle = engine.get_resource_handle::<PBRMaterial>("pillars")?;
    let _pillars_entity = engine
        .build_entity(scene_handle)
        .with_component(
            TransformComponent::builder()
                .position(Vector3f::new(0.0, 0.0, 0.0))
                .scale(Vector3f::new(1.0, 1.0, 1.0))
                .build(),
        )
        .with_component(
            MeshRenderingComponent::builder()
                .mesh(&pillars_mesh_handle)
                .material(&pillars_material_handle)
                .build(),
        )
        .build();

    // Spawn ground
    let ground_mesh_handle = engine.get_resource_handle::<Mesh>("ground")?;
    let ground_material_handle = engine.get_resource_handle::<PBRMaterial>("ground")?;

    let _ground_entity = engine
        .build_entity(scene_handle)
        .with_component(
            TransformComponent::builder()
                .position(Vector3f::new(0.0, 0.0, 0.0))
                .scale(Vector3f::new(1.0, 1.0, 1.0))
                .build(),
        )
        .with_component(
            MeshRenderingComponent::builder()
                .mesh(&ground_mesh_handle)
                .material(&ground_material_handle)
                .build(),
        )
        .build();

    // Create plane mesh
    // let plane_mesh_handle = engine.get_resource_handle::<Mesh>("plane")?;
    // let grid_material_handle = engine.get_resource_handle::<PBRMaterial>("grid")?;
   
    // // Spawn plane
    // let _plane_entity = engine
    //     .build_entity(scene_handle)
    //     .with_component(
    //         TransformComponent::builder()
    //             .position(Vector3f::new(0.0, -5.0, 0.0))
    //             .scale(Vector3f::new(50.0, 1.0, 50.0))
    //             .build(),
    //     )
    //     .with_component(
    //         MeshRenderingComponent::builder()
    //             .mesh(&plane_mesh_handle)
    //             .material(&grid_material_handle)
    //             .build(),
    //     )
    //     .build();

    // let pill_mesh_handle = engine.get_resource_handle::<Mesh>("pill")?;
    // let wood_material_handle = engine.get_resource_handle::<PBRMaterial>("wood")?;

    // let _pill_entity = engine
    //     .build_entity(scene_handle)
    //     .with_component(
    //         TransformComponent::builder()
    //             .position(Vector3f::new(5.0, 0.0, 0.0))
    //             .scale(Vector3f::new(1.0, 1.0, 1.0))
    //             .build(),
    //     )
    //     .with_component(
    //         MeshRenderingComponent::builder()
    //             .mesh(&pill_mesh_handle)
    //             .material(&wood_material_handle)
    //             .build(),
    //     )
    //     .build();

    // Spawn curl entities
    // No rendering - 50k - 5ms, 500k - 50ms
    // Rendering, no matrix calculation - 50k - 5ms 26ms total 
    // Rendering, with matrix calculation - 50k - 5ms 32ms total 
    spawn_floating_objects(engine, 50000)?;

    Ok(())
}

fn demo_control_system(engine: &mut Engine) -> Result<()> {
    let input_component = engine.get_global_component::<InputComponent>()?;

    let o_key = input_component.get_key(KeyboardKey::KeyO);
    let p_key = input_component.get_key(KeyboardKey::KeyP);

    let l_key = input_component.get_key(KeyboardKey::KeyL);
    let k_key = input_component.get_key(KeyboardKey::KeyK);

    let m_key = input_component.get_key(KeyboardKey::KeyM);
    let n_key = input_component.get_key(KeyboardKey::KeyN);

    let v_key = input_component.get_key(KeyboardKey::KeyV);
    let b_key = input_component.get_key(KeyboardKey::KeyB);

    let demo_state = engine.get_global_component_mut::<DemoStateComponent>()?;
    
    if o_key {
        demo_state.curl_scale += 0.0005;
    }
    if p_key {
        demo_state.curl_scale -= 0.0005;
    }

    if l_key {
        demo_state.curl_attraction += 5.0;
    }
    if k_key {
        demo_state.curl_attraction -= 5.0;
    }

    if m_key {
        demo_state.curl_damping += 0.01;
    }
    if n_key {
        demo_state.curl_damping -= 0.01;
    }

    if v_key {
        demo_state.curl_epsilon += 0.001;
    }
    if b_key {
        demo_state.curl_epsilon -= 0.001;
    }

    println!(
        "Curl Scale: {:.4}, Attraction: {:.2}, Damping: {:.4}, Epsilon: {:.4}",
        demo_state.curl_scale,
        demo_state.curl_attraction,
        demo_state.curl_damping,
        demo_state.curl_epsilon
    );

    Ok(())
}


// Advanced FPS camera system with WASD movement, mouse look, and smooth lerping
fn fps_camera_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component::<InputComponent>()?;

    // Get input
    let w_key = input_component.get_key(KeyboardKey::KeyW);
    let s_key = input_component.get_key(KeyboardKey::KeyS);
    let a_key = input_component.get_key(KeyboardKey::KeyA);
    let d_key = input_component.get_key(KeyboardKey::KeyD);
    let e_key = input_component.get_key(KeyboardKey::KeyE);
    let shift_key = input_component.get_key(KeyboardKey::ShiftLeft);
    let q_key = input_component.get_key(KeyboardKey::KeyQ);
    
    let mouse_delta = input_component.get_mouse_delta();

    for (_, transform_component, camera_component, movement_component) in
        engine.iterate_three_components_mut::<TransformComponent, CameraComponent, CameraMovementComponent>()?
    {
        if !camera_component.enabled {
            continue;
        }

        // Get settings from component
        let move_speed = movement_component.move_speed;
        let sprint_multiplier = movement_component.sprint_multiplier;
        let lerp_speed = movement_component.lerp_speed;
        let mouse_sensitivity = movement_component.mouse_sensitivity;
        let rotation_lerp_speed = movement_component.rotation_lerp_speed;

        // Mouse look - update target rotation (inverted Y axis)
        movement_component.target_rotation.y -= mouse_delta.x * mouse_sensitivity;
        movement_component.target_rotation.x += mouse_delta.y * mouse_sensitivity; // Inverted Y
        
        // Clamp pitch to avoid gimbal lock
        movement_component.target_rotation.x = movement_component.target_rotation.x.clamp(-89.0, 89.0);
        
        // Lerp rotation for smooth camera movement
        let rotation_t = 1.0 - (-rotation_lerp_speed * delta_time).exp();
        movement_component.current_rotation.x += (movement_component.target_rotation.x - movement_component.current_rotation.x) * rotation_t;
        movement_component.current_rotation.y += (movement_component.target_rotation.y - movement_component.current_rotation.y) * rotation_t;
        
        transform_component.set_rotation(movement_component.current_rotation);

        // Calculate movement direction based on camera rotation
        let yaw = movement_component.current_rotation.y.to_radians();
        let forward = Vector3f::new(yaw.sin(), 0.0, yaw.cos());
        let right = Vector3f::new(yaw.cos(), 0.0, -yaw.sin());

        // Calculate target movement
        let mut movement = Vector3f::new(0.0, 0.0, 0.0);

        if w_key {
            movement = movement + forward;
        }
        if s_key {
            movement = movement - forward;
        }
        if d_key {
            movement = movement - right; // D moves left
        }
        if a_key {
            movement = movement + right; // A moves right
        }
        if e_key {
            movement.y += 1.0;
        }
        if q_key {
            movement.y -= 1.0;
        }

        // Normalize movement vector if moving diagonally
        let movement_length = (movement.x * movement.x + movement.y * movement.y + movement.z * movement.z).sqrt();
        if movement_length > 0.0 {
            movement.x /= movement_length;
            movement.y /= movement_length;
            movement.z /= movement_length;
        }

        // Apply sprint multiplier
        let current_speed = if shift_key {
            move_speed * sprint_multiplier
        } else {
            move_speed
        };

        // Set target velocity
        movement_component.target_velocity = movement * current_speed;

        // Lerp velocity for smooth acceleration/deceleration
        let velocity_t = 1.0 - (-lerp_speed * delta_time).exp();
        movement_component.current_velocity.x += (movement_component.target_velocity.x - movement_component.current_velocity.x) * velocity_t;
        movement_component.current_velocity.y += (movement_component.target_velocity.y - movement_component.current_velocity.y) * velocity_t;
        movement_component.current_velocity.z += (movement_component.target_velocity.z - movement_component.current_velocity.z) * velocity_t;

        // Apply movement
        let displacement = movement_component.current_velocity * delta_time;
        let new_position = transform_component.position + displacement;
        transform_component.set_position(new_position);
    }

    Ok(())
}