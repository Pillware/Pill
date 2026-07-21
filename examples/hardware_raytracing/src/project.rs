use pill_engine::project::*;

// ── Color palette ──────────────────────────────────────────────────────
const CUBE_COLORS: [Color; 6] = [
    Color::new(0.90, 0.30, 0.25), // red
    Color::new(0.25, 0.55, 0.90), // blue
    Color::new(0.30, 0.80, 0.35), // green
    Color::new(0.90, 0.75, 0.20), // gold
    Color::new(0.70, 0.35, 0.85), // purple
    Color::new(0.95, 0.55, 0.15), // orange
];

// ── Project ────────────────────────────────────────────────────────────

pub struct Project {}
create_project!(Project {}, PillProject);

impl PillProject for Project {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // --- Scene setup ---
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;

        // --- Create meshes ---
        let cube_mesh = engine.add_resource(Mesh::cube("cube", 1.0))?;
        // Reuse the cube mesh for a large thin floor plane.
        let floor_mesh = engine.add_resource(Mesh::cube("floor", 1.0))?;

        // --- Create materials ---
        let materials: Vec<MaterialHandle> = CUBE_COLORS
            .iter()
            .enumerate()
            .map(|(i, color)| {
                engine.add_resource(
                    Material::builder(&format!("cube_material_{i}"))
                        .color_parameter("tint", *color)?
                        .scalar_parameter("specularity", 0.6)?
                        .build(),
                )
            })
            .collect::<Result<Vec<_>>>()?;

        let floor_material = engine.add_resource(
            Material::builder("floor_material")
                .color_parameter("tint", Color::new(0.35, 0.33, 0.30))?
                .scalar_parameter("specularity", 0.1)?
                .build(),
        )?;

        // --- Camera ---
        // +Z = backward (into screen). look_to_rh looks toward +Z.
        // Place camera at z=-10 so objects at z=0..8 are in front.
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 4.0, -10.0))
                    .rotation(Vector3f::new(0.0, 0.0, 0.0)) // look straight ahead (+Z)
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(70.0)
                    .clear_color(Color::new(0.08, 0.09, 0.12))
                    .build(),
            )
            .build();

        // --- Floor: a wide thin plane at y = -0.1 ────────────────────
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, -0.1, 4.0))
                    .scale(Vector3f::new(30.0, 0.1, 30.0))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&floor_mesh)
                    .material(&floor_material)
                    .build(),
            )
            .build();

        // --- Cubes: a grid from z=0 to z=8, x=-4 to x=4, y=1 ────────
        // Plenty of colored cubes directly in front of the camera.
        let mut color_index: usize = 0;
        for z_step in 0..5 {
            let z = (z_step as f32) * 2.0; // 0, 2, 4, 6, 8
            for x_step in -2..=2 {
                let x = (x_step as f32) * 2.0; // -4, -2, 0, 2, 4
                let y = 1.0 + ((x_step + z_step) as f32).abs() * 0.25; // varied heights

                engine
                    .build_entity(active_scene)
                    .with_component(
                        TransformComponent::builder()
                            .position(Vector3f::new(x, y, z))
                            .scale(Vector3f::new(1.0, 1.0, 1.0))
                            .build(),
                    )
                    .with_component(
                        MeshRenderingComponent::builder()
                            .mesh(&cube_mesh)
                            .material(&materials[color_index % materials.len()])
                            .build(),
                    )
                    .build();

                color_index += 1;
            }
        }

        // --- Extra ring of cubes close to camera (z=-4) ──────────────
        for i in 0..8 {
            let angle = (i as f32) * std::f32::consts::TAU / 8.0;
            let x = angle.cos() * 5.0;
            let y = 3.0 + angle.sin() * 2.0;
            let z = -4.0;

            engine
                .build_entity(active_scene)
                .with_component(
                    TransformComponent::builder()
                        .position(Vector3f::new(x, y, z))
                        .scale(Vector3f::new(0.6, 0.6, 0.6))
                        .build(),
                )
                .with_component(
                    MeshRenderingComponent::builder()
                        .mesh(&cube_mesh)
                        .material(&materials[i % materials.len()])
                        .build(),
                )
                .build();
        }

        // --- Walls: four walls around the scene (z=-2 to z=10, x=-5 to x=5) -------
        let wall_material_index = 5; // orange

        // Helper: place a cube at a grid position for the walls
        let mut place_wall_cube = |x: f32, y: f32, z: f32| -> Result<()> {
            engine
                .build_entity(active_scene)
                .with_component(
                    TransformComponent::builder()
                        .position(Vector3f::new(x, y, z))
                        .scale(Vector3f::new(1.0, 1.0, 1.0))
                        .build(),
                )
                .with_component(
                    MeshRenderingComponent::builder()
                        .mesh(&cube_mesh)
                        .material(&materials[wall_material_index])
                        .build(),
                )
                .build();
            Ok(())
        };

        // Front wall (z = -2)
        for xi in -5..=5 {
            for yi in 0..5 {
                place_wall_cube(xi as f32, yi as f32 + 0.5, -2.0)?;
            }
        }
        // Back wall (z = 10)
        for xi in -5..=5 {
            for yi in 0..5 {
                place_wall_cube(xi as f32, yi as f32 + 0.5, 10.0)?;
            }
        }
        // Left wall (x = -5)
        for zi in -1..=9 {
            for yi in 0..5 {
                place_wall_cube(-5.0, yi as f32 + 0.5, zi as f32)?;
            }
        }
        // Right wall (x = 5)
        for zi in -1..=9 {
            for yi in 0..5 {
                place_wall_cube(5.0, yi as f32 + 0.5, zi as f32)?;
            }
        }

        // --- Rotating cubes system ───────────────────────────────────
        engine.add_system("rotate_cubes", rotate_cubes_system)?;

        // --- Camera controller ──────────────────────────────────────
        engine.add_system("camera_controller", camera_controller_system)?;

        Ok(())
    }
}

// ── Systems ─────────────────────────────────────────────────────────────

/// WASD movement + arrow key rotation for the active camera.
fn camera_controller_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

    // Collect input state first so the immutable borrow is released
    // before we mutably borrow engine for the component iterator.
    let key_w = engine
        .get_global_component::<InputComponent>()?
        .get_key(KeyboardKey::KeyW);
    let key_s = engine
        .get_global_component::<InputComponent>()?
        .get_key(KeyboardKey::KeyS);
    let key_a = engine
        .get_global_component::<InputComponent>()?
        .get_key(KeyboardKey::KeyA);
    let key_d = engine
        .get_global_component::<InputComponent>()?
        .get_key(KeyboardKey::KeyD);
    let key_q = engine
        .get_global_component::<InputComponent>()?
        .get_key(KeyboardKey::KeyQ);
    let key_e = engine
        .get_global_component::<InputComponent>()?
        .get_key(KeyboardKey::KeyE);
    let key_up = engine
        .get_global_component::<InputComponent>()?
        .get_key(KeyboardKey::ArrowUp);
    let key_down = engine
        .get_global_component::<InputComponent>()?
        .get_key(KeyboardKey::ArrowDown);
    let key_left = engine
        .get_global_component::<InputComponent>()?
        .get_key(KeyboardKey::ArrowLeft);
    let key_right = engine
        .get_global_component::<InputComponent>()?
        .get_key(KeyboardKey::ArrowRight);

    let move_speed: f32 = 10.0;
    let rotate_speed: f32 = 80.0;

    for (_entity, transform, camera) in
        engine.iterate_two_components_mut::<TransformComponent, CameraComponent>()?
    {
        if !camera.enabled {
            continue;
        }

        let mut position = transform.position;
        let mut rotation = transform.rotation;

        // Forward / right from yaw (rotation.y), ignoring pitch for
        // horizontal movement so WASD stays flat on the XZ plane.
        let yaw_radians = rotation.y.to_radians();
        let forward = Vector3f::new(yaw_radians.sin(), 0.0, yaw_radians.cos());
        let right = Vector3f::new(yaw_radians.cos(), 0.0, -yaw_radians.sin());

        // --- WASD movement -------------------------------------------
        if key_w {
            position += forward * move_speed * delta_time;
        }
        if key_s {
            position -= forward * move_speed * delta_time;
        }
        if key_a {
            position += right * move_speed * delta_time;
        }
        if key_d {
            position -= right * move_speed * delta_time;
        }
        // Q / E for vertical movement
        if key_q {
            position.y -= move_speed * delta_time;
        }
        if key_e {
            position.y += move_speed * delta_time;
        }

        // --- Arrow key rotation -------------------------------------
        if key_up {
            rotation.x -= rotate_speed * delta_time;
        }
        if key_down {
            rotation.x += rotate_speed * delta_time;
        }
        if key_left {
            rotation.y += rotate_speed * delta_time;
        }
        if key_right {
            rotation.y -= rotate_speed * delta_time;
        }

        transform.set_position(position);
        transform.set_rotation(rotation);
        break; // Only control the first enabled camera
    }

    Ok(())
}

/// Slowly rotate all cubes around their own axes.
fn rotate_cubes_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;

    let mut i = 0u32;
    for (_entity, transform, _mesh) in
        engine.iterate_two_components_mut::<TransformComponent, MeshRenderingComponent>()?
    {
        let rot = transform.rotation;
        transform.set_rotation(Vector3f::new(
            rot.x + 1.5 * dt * (1.0 + i as f32 * 0.1),
            rot.y + 2.5 * dt * (1.0 + i as f32 * 0.1),
            rot.z + 0.8 * dt * (1.0 + i as f32 * 0.1),
        ));
        i += 1;
    }

    Ok(())
}
