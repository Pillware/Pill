use pill_engine::{define_component, define_global_component, game::*};
use rand::{thread_rng, Rng};
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;

pub const FLOATING_OBJECT_SPAWN_BATCH_COUNT: usize = 100;
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
    current_mesh: usize,
    mesh_handles: Vec::<MeshHandle>,
    current_material_set: usize,
    textured_material_handles: Vec::<MaterialHandle>,
    plain_color_material_handles: Vec::<MaterialHandle>,
});

define_component!(CameraMovementComponent {
    orbit_speed: f32,
    zoom_speed: f32,
    angle: f32,
    radius: f32,
    delta_y: f32,
    delta_z: f32,
});

#[cfg(feature = "benchmark")]
define_global_component!(BenchComponent {
    frames: u64,
    acc_math_ns: u128,
    acc_frame_ms: f64,
    worst_frame_ms: f32,
    moved_acc: u64,
    last_report: Instant,
    report_every_frames: u32,
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

        // Add systems
        engine.add_system("spawn_floating_objects", floating_objects_spawn_system)?;
        engine.add_system("delete_floating_objects", floating_objects_remove_system)?;
        engine.add_system("objects_movement", floating_objects_movement_system)?;
        engine.add_system("camera_movement", camera_movement_system)?;
        engine.add_system("camera_fov", camera_fov_changing_system)?;
        //engine.add_system("mesh_changing", object_appearance_changing_system)?;
        engine.add_system("demo_control", demo_control_system)?;
        #[cfg(feature = "benchmark")]
        engine.add_system("bench_report", bench_report_system)?;

        // --- Create resources ---

        // Add meshes
        let pill_mesh = Mesh::new("pill", "models/pill.obj".into());
        let pill_mesh_handle = engine.add_resource(pill_mesh)?;

        let cube_mesh = Mesh::new("rounded_cube", "models/rounded_cube.obj".into());
        let cube_mesh_handle = engine.add_resource(cube_mesh)?;

        let torus_mesh = Mesh::new("torus", "models/torus.obj".into());
        let torus_mesh_handle = engine.add_resource(torus_mesh)?;

        // Add sounds
        let ambient_music = Sound::new("ambient", "audio/test_music.mp3".into());
        let ambient_music_handle = engine.add_resource(ambient_music)?;

        // Add textures
        let fabric_color_texture = Texture::new(
            "fabric_color",
            TextureType::Color,
            ResourceLoader::Path("textures/fabric_color.jpg".into()),
        );
        let fabric_color_texture_handle = engine.add_resource::<Texture>(fabric_color_texture)?;

        let fabric_normal_texture = Texture::new(
            "fabric_normal",
            TextureType::Normal,
            ResourceLoader::Path("textures/fabric_normal.jpg".into()),
        );
        let fabric_normal_texture_handle = engine.add_resource::<Texture>(fabric_normal_texture)?;

        let stones_color_texture = Texture::new(
            "stones_color",
            TextureType::Color,
            ResourceLoader::Path("textures/stones_color.jpg".into()),
        );
        let stones_color_texture_handle = engine.add_resource::<Texture>(stones_color_texture)?;

        let stones_normal_texture = Texture::new(
            "stones_normal",
            TextureType::Normal,
            ResourceLoader::Path("textures/stones_normal.jpg".into()),
        );
        let stones_normal_texture_handle = engine.add_resource::<Texture>(stones_normal_texture)?;

        let organic_color_texture = Texture::new(
            "organic_color",
            TextureType::Color,
            ResourceLoader::Path("textures/organic_color.jpg".into()),
        );
        let organic_color_texture_handle = engine.add_resource::<Texture>(organic_color_texture)?;

        let organic_normal_texture = Texture::new(
            "organic_normal",
            TextureType::Normal,
            ResourceLoader::Path("textures/organic_normal.jpg".into()),
        );
        let organic_normal_texture_handle =
            engine.add_resource::<Texture>(organic_normal_texture)?;

        // Add materials
        let fabric_material_handle = engine.add_resource::<Material>(
            Material::builder("fabric")
                .texture("color", fabric_color_texture_handle)?
                .texture("normal", fabric_normal_texture_handle)?
                .color_parameter("tint", Color::new(1.0, 0.1, 0.1))?
                .build(),
        )?;

        let stones_material_handle = engine.add_resource::<Material>(
            Material::builder("stones")
                .texture("color", stones_color_texture_handle)?
                .texture("normal", stones_normal_texture_handle)?
                .build(),
        )?;

        let organic_material_handle = engine.add_resource::<Material>(
            Material::builder("organic")
                .texture("color", organic_color_texture_handle)?
                .texture("normal", organic_normal_texture_handle)?
                .color_parameter("tint", Color::new(0.26, 0.87, 0.9))?
                .scalar_parameter("specularity", 3.0)?
                .build(),
        )?;

        let yellow_material_handle = engine.add_resource::<Material>(
            Material::builder("yellow")
                .color_parameter("tint", Color::new(1.0, 0.88, 0.0))?
                .build(),
        )?;

        let blue_material_handle = engine.add_resource::<Material>(
            Material::builder("blue")
                .color_parameter("tint", Color::new(0.26, 0.87, 0.9))?
                .build(),
        )?;

        let white_material_handle =
            engine.add_resource::<Material>(Material::builder("white").build())?;

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

        // Create camera entity
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.0, -30.0))
                    .rotation(Vector3f::new(0.0, 0.0, 0.0))
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(60.0)
                    .clear_color(Color::new(0.35, 0.40, 0.50))
                    .build(),
            )
            .with_component(CameraMovementComponent {
                orbit_speed: 60.0,
                zoom_speed: 5.0,
                angle: 0.0,
                radius: 30.0,
                delta_y: 0.0,
                delta_z: 0.0,
            })
            .with_component(AudioListenerComponent::builder().enabled(true).build())
            .build();

        #[cfg(feature = "benchmark")]
        engine.add_global_component(BenchComponent {
            frames: 0,
            acc_math_ns: 0,
            acc_frame_ms: 0.0,
            worst_frame_ms: 0.0,
            moved_acc: 0,
            last_report: Instant::now(),
            report_every_frames: 120,
        })?;

        // Setup demo state component
        let demo_state = DemoStateComponent {
            floating_objects_movement_enabled: true,
            current_mesh: 0,
            mesh_handles: vec![pill_mesh_handle, cube_mesh_handle, torus_mesh_handle],
            current_material_set: 0,
            textured_material_handles: vec![
                fabric_material_handle,
                stones_material_handle,
                organic_material_handle,
            ],
            plain_color_material_handles: vec![
                yellow_material_handle,
                blue_material_handle,
                white_material_handle,
            ],
        };
        engine.add_global_component(demo_state)?;

        // Spawn certain number of floating objects
        spawn_floating_objects(engine, FLOATING_OBJECT_SPAWN_BATCH_COUNT)?;

        #[cfg(feature = "benchmark")]
        if let Ok(mut fh) = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open("vec3_bench.csv")
        {
            let _ = writeln!(fh, "LOG BEGIN",);
            writeln!(fh, "fps | avg_math_ms | avg_frame_ms | worst_frame_ms");
        }

        Ok(())
    }
}

// --- Systems ---

fn demo_control_system(engine: &mut Engine) -> Result<()> {
    let input_component = engine.get_global_component::<InputComponent>()?;
    let system_toggle_key = input_component.get_key_pressed(TOGGLE_FLOATING_OBJECTS_SYSTEM);

    let demo_state = engine.get_global_component_mut::<DemoStateComponent>()?;
    if system_toggle_key {
        demo_state.floating_objects_movement_enabled =
            !demo_state.floating_objects_movement_enabled;
        let enabled = demo_state.floating_objects_movement_enabled;
        engine.toggle_system("objects_movement", UpdatePhase::Game, enabled)?;
    }

    Ok(())
}

fn floating_objects_movement_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

    #[cfg(feature = "benchmark")]
    let frame_t0 = Instant::now();
    #[cfg(feature = "benchmark")]
    let math_t0 = Instant::now();
    #[cfg(feature = "benchmark")]
    let mut moved_this_frame: u64 = 0;

    for (_, floating_object_transform, floating_object_component) in
        engine.iterate_two_components_mut::<TransformComponent, FloatingObjectComponent>()?
    {
        #[cfg(feature = "benchmark")]
        {
            moved_this_frame += 1;
        }

        // Local rotation
        let rotation_speed = floating_object_component.rotation_speed;
        floating_object_transform
            .rotate_around_axis(rotation_speed * delta_time, Vector3f::new(1.0, 1.0, 1.0));

        // Local scale
        let scale_speed = floating_object_component.scale_speed;
        floating_object_component.scale_factor += scale_speed * delta_time;
        let scale_factor = floating_object_component.scale_factor;
        floating_object_transform
            .set_scale(Vector3f::new(0.4, 0.4, 0.4) * (scale_factor.sin() / 1.5 + 1.5));

        // Radius
        let radius_speed = floating_object_component.radius_speed;
        floating_object_component.radius_factor += radius_speed * delta_time;

        // Movement
        let orbital_movement_speed = floating_object_component.orbital_movement_speed;
        floating_object_component.angle += orbital_movement_speed * delta_time;

        let angle = floating_object_component.angle;
        let radius = floating_object_component.radius_factor.sin() * 6.0 + 10.0;

        floating_object_transform.set_position(Vector3f::new(
            angle.to_radians().cos() * radius,
            floating_object_transform.position.y,
            angle.to_radians().sin() * radius,
        ));

        let y_axis_movement_speed = floating_object_component.y_axis_movement_speed;
        floating_object_component.y_axis_factor += y_axis_movement_speed * delta_time;
        let y_axis_factor = floating_object_component.y_axis_factor;

        floating_object_transform.set_position(Vector3f::new(
            angle.to_radians().cos() * radius,
            y_axis_factor.sin() * 0.8 * radius,
            angle.to_radians().sin() * radius,
        ));
    }

    #[cfg(feature = "benchmark")]
    {
        let math_ns = math_t0.elapsed().as_nanos();
        let frame_ms = frame_t0.elapsed().as_secs_f64() * 1_000.0;

        let s = engine.get_global_component_mut::<BenchComponent>()?;
        s.frames += 1;
        s.acc_math_ns += math_ns;
        s.acc_frame_ms += frame_ms;
        if frame_ms as f32 > s.worst_frame_ms {
            s.worst_frame_ms = frame_ms as f32;
        }

        s.moved_acc += moved_this_frame;
    }

    Ok(())
}

fn object_appearance_changing_system(engine: &mut Engine) -> Result<()> {
    let mut rng = thread_rng();

    let input_component = engine.get_global_component::<InputComponent>()?;
    let mesh_key = input_component.get_key_pressed(FLOATING_OBJECTS_CHANGE_MESH_BUTTON);
    let material_key = input_component.get_key_pressed(FLOATING_OBJECTS_CHANGE_MATERIAL_BUTTON);

    // Set same mesh
    if mesh_key {
        let demo_state = engine.get_global_component_mut::<DemoStateComponent>()?;
        demo_state.current_mesh = (demo_state.current_mesh + 1) % 3;
        let mesh_handle = *demo_state
            .mesh_handles
            .get(demo_state.current_mesh)
            .unwrap();
        for (_, mesh_rendering_component) in
            engine.iterate_one_component_mut::<MeshRenderingComponent>()?
        {
            mesh_rendering_component.set_mesh(&mesh_handle);
        }
    }

    // Set random material from set
    if material_key {
        let demo_state = engine.get_global_component_mut::<DemoStateComponent>()?;
        demo_state.current_material_set = (demo_state.current_material_set + 1) % 2;

        let current_material_set = match demo_state.current_material_set == 0 {
            true => demo_state.textured_material_handles.clone(),
            false => demo_state.plain_color_material_handles.clone(),
        };

        for (_, mesh_rendering_component) in
            engine.iterate_one_component_mut::<MeshRenderingComponent>()?
        {
            let material_handle = current_material_set[rng.gen_range(0..=2)];
            mesh_rendering_component.set_material(&material_handle);
        }
    }

    Ok(())
}

fn camera_movement_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component_mut::<InputComponent>()?;

    // Get input
    let a_key = input_component.get_key(KeyboardKey::KeyA);
    let d_key = input_component.get_key(KeyboardKey::KeyD);
    let right_mouse_button = input_component.get_mouse_button(MouseButton::Right);
    let mouse_scroll_delta = input_component.get_mouse_scroll_delta();
    let mouse_delta = input_component.get_mouse_delta();

    // Get gamepad input
    let gamepad_left_stick =
        input_component.get_gamepad_axis(PlayerId::Player1, GamepadAxis::LeftStickX);

    // Pressing left bumper causes rumble (Example of haptics usage)
    let left_bumper =
        input_component.get_gamepad_button(PlayerId::Player1, GamepadButton::LeftBumper);
    if left_bumper {
        input_component.enqueue_rumble(PlayerId::Player1, 1.0, 1.0, 500);
    }

    for (_, transform_transform, camera_movement_component) in
        engine.iterate_two_components_mut::<TransformComponent, CameraMovementComponent>()?
    {
        // Zoom
        let zoom_speed = camera_movement_component.zoom_speed;
        camera_movement_component.radius -= mouse_scroll_delta.y * zoom_speed;

        // Orbit
        let mut change_value: f32 = 0.0;
        // TODO: make it progressive for gamepad
        if d_key {
            change_value -= 1.0;
        } else if gamepad_left_stick < -0.1 {
            change_value += 1.0;
        }
        if a_key {
            change_value += 1.0;
        } else if gamepad_left_stick > 0.1 {
            change_value -= 1.0;
        }
        let orbit_speed = camera_movement_component.orbit_speed;
        camera_movement_component.angle += change_value * orbit_speed * delta_time;
        let angle = camera_movement_component.angle;
        let radius = camera_movement_component.radius;

        let x_position = angle.to_radians().cos() * radius;
        let z_position = angle.to_radians().sin() * radius;

        // Mouse movement
        let mut z_change_value = 0.0;
        if mouse_delta.x > 0.0 {
            z_change_value -= 0.2;
        }
        if mouse_delta.x < 0.0 {
            z_change_value += 0.2;
        }

        let mut y_change_value = 0.0;
        if mouse_delta.y > 0.0 {
            y_change_value -= 0.2;
        }
        if mouse_delta.y < 0.0 {
            y_change_value += 0.2;
        }

        if right_mouse_button {
            camera_movement_component.delta_z += z_change_value;
            camera_movement_component.delta_y += y_change_value;
        }

        let delta_y = camera_movement_component.delta_y;
        let delta_z = camera_movement_component.delta_z;

        // Set position
        transform_transform.set_position(Vector3f::new(x_position, delta_y, z_position + delta_z));

        // Set rotation
        transform_transform.set_rotation(Vector3f::new(0.0, -angle - 90.0, 0.0));
    }

    Ok(())
}

fn camera_fov_changing_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component::<InputComponent>()?;

    // Get input
    let t_key = input_component.get_key(INCREASE_CAMERA_FOV_BUTTON);
    let g_key = input_component.get_key(DECREASE_CAMERA_FOV_BUTTON);

    // Get gamepad input
    let gamepad_right_stick =
        input_component.get_gamepad_axis(PlayerId::Player1, GamepadAxis::RightStickY);

    for (_, camera_component) in engine.iterate_one_component_mut::<CameraComponent>()? {
        let mut change_value: f32 = 0.0;
        if t_key {
            change_value += 1.0;
        } else if gamepad_right_stick > 0.1 {
            change_value -= 1.0;
        }
        if g_key {
            change_value -= 1.0;
        } else if gamepad_right_stick < -0.1 {
            change_value += 1.0;
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
    let input_component = engine.get_global_component::<InputComponent>()?;

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
    let input_component = engine.get_global_component::<InputComponent>()?;

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
    let demo_state = engine.get_global_component::<DemoStateComponent>()?;
    let mesh_handles = demo_state.mesh_handles.clone();
    let textured_material_handles = demo_state.textured_material_handles.clone();

    for _ in 0..object_count {
        let mesh_index = rng.gen_range(0..mesh_handles.len());
        let mat_index = rng.gen_range(0..textured_material_handles.len());

        let mesh_handle = mesh_handles[mesh_index];
        let material_handle = textured_material_handles[mat_index];

        // // Randommize mesh
        //     let mesh_handle = mesh_handles[rng.gen_range(0..=2)];

        //         // Randomize material
        //     let material_handle = textured_material_handles[rng.gen_range(0..=2)];

        engine
            .build_entity(active_scene)
            .with_component(FloatingObjectComponent {
                angle: rng.gen_range(0.0..359.0),
                radius_factor: rng.gen_range(20.0..180.0),
                scale_factor: rng.gen_range(0.5..1.5),
                y_axis_factor: rng.gen_range(0.0..6.0),

                orbital_movement_speed: rng.gen_range(40.0..80.0),
                y_axis_movement_speed: rng.gen_range(-0.6..0.6),
                rotation_speed: rng.gen_range(-45.0..45.0),
                scale_speed: rng.gen_range(0.06..1.2),
                radius_speed: rng.gen_range(0.1..1.2),
            })
            .with_component(TransformComponent::new())
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

#[cfg(feature = "benchmark")]
fn bench_report_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let s = engine.get_global_component_mut::<BenchComponent>()?;

    if s.frames >= s.report_every_frames as u64 {
        let f = s.frames as f64;
        let avg_math_ms = (s.acc_math_ns as f64 / 1_000_000.0) / f;
        let avg_frame_ms = s.acc_frame_ms / f;
        let moved_per_frame = (s.moved_acc as f64 / f).round() as u64;
        let new_frame_time = delta_time * 1000.0;
        let fps = 1000.0 / new_frame_time;

        // One-line, spreadsheet-friendly
        println!(
            "BENCH vec3 | fps={} | moved/frame={} | avg_math_ms={:.3} | avg_frame_ms={:.3} | worst_ms={:.3}",
            fps, moved_per_frame, avg_math_ms, avg_frame_ms, s.worst_frame_ms
        );

        if let Ok(mut fh) = OpenOptions::new()
            .append(true)
            .create(true)
            .open("vec3_bench.csv")
        {
            let _ = writeln!(
                fh,
                "{},{:.3},{:.3},{:.3}",
                fps, avg_math_ms, avg_frame_ms, s.worst_frame_ms
            );
        }

        // reset rolling window
        s.frames = 0;
        s.acc_math_ns = 0;
        s.acc_frame_ms = 0.0;
        s.worst_frame_ms = 0.0;
        s.moved_acc = 0;
        s.last_report = Instant::now();
    }

    Ok(())
}
