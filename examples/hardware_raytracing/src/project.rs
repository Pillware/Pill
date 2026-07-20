use pill_engine::project::*;

// ── Color palette ──────────────────────────────────────────────────────
const FLOOR_COLOR: Color = Color::new(0.45, 0.42, 0.38); // warm grey
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
        // Floor: a wide thin cube scaled to act as a ground plane.
        let floor_mesh = engine.add_resource(Mesh::cube("floor", 1.0))?;
        // Unit cube for all objects.
        let cube_mesh = engine.add_resource(Mesh::cube("cube", 1.0))?;

        // --- Create materials ---
        let floor_material = engine.add_resource(
            Material::builder("floor_material")
                .color_parameter("tint", FLOOR_COLOR)?
                .scalar_parameter("specularity", 0.3)?
                .build(),
        )?;

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

        // --- Camera ---
        // Positioned above and behind the scene, looking slightly down.
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 6.0, 12.0))
                    .rotation(Vector3f::new(-15.0, 0.0, 0.0)) // pitched down
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(55.0)
                    .clear_color(Color::new(0.08, 0.09, 0.12))
                    .build(),
            )
            .build();

        // --- Floor ---
        // Wide and thin: scale (16, 0.1, 16), at y = -0.05 so the top
        // surface is approximately at y = 0.
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, -0.05, 0.0))
                    .scale(Vector3f::new(16.0, 0.1, 16.0))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&floor_mesh)
                    .material(&floor_material)
                    .build(),
            )
            .build();

        // --- Static cubes (no rotation system) ---
        // Placed around the scene to cast shadows on the floor.
        let static_positions: [(Vector3f, Vector3f, usize); 7] = [
            (
                Vector3f::new(0.0, 0.5, 1.0),
                Vector3f::new(1.0, 1.0, 1.0),
                0,
            ), // center, red
            (
                Vector3f::new(-3.0, 0.5, 3.0),
                Vector3f::new(1.0, 1.5, 1.0),
                1,
            ), // left, blue, taller
            (
                Vector3f::new(3.0, 0.5, 3.0),
                Vector3f::new(1.5, 1.0, 1.0),
                2,
            ), // right, green, wider
            (
                Vector3f::new(-2.0, 0.5, 6.0),
                Vector3f::new(1.0, 1.0, 1.0),
                3,
            ), // back-left, gold
            (
                Vector3f::new(2.0, 0.5, 6.0),
                Vector3f::new(1.0, 1.0, 1.0),
                4,
            ), // back-right, purple
            (
                Vector3f::new(-1.0, 0.5, -2.0),
                Vector3f::new(1.0, 2.0, 1.0),
                5,
            ), // front-left, orange, tall
            (
                Vector3f::new(1.0, 0.5, -2.0),
                Vector3f::new(1.0, 1.0, 1.5),
                0,
            ), // front-right, red
        ];

        for (pos, scale, color_idx) in &static_positions {
            engine
                .build_entity(active_scene)
                .with_component(
                    TransformComponent::builder()
                        .position(*pos)
                        .scale(*scale)
                        .build(),
                )
                .with_component(
                    MeshRenderingComponent::builder()
                        .mesh(&cube_mesh)
                        .material(&materials[*color_idx])
                        .build(),
                )
                .build();
        }

        // --- Rotating cubes (will be animated) ---
        // These move in a circle to demonstrate dynamic shadow updates
        // with TLAS-only rebuild.
        let rotating_configs: [(Vector3f, f32, usize); 3] = [
            (Vector3f::new(0.0, 2.0, 4.0), 1.5, 1),  // high center, blue
            (Vector3f::new(-4.0, 1.5, 5.0), 1.0, 0), // left, red
            (Vector3f::new(4.0, 1.0, 1.0), 1.3, 3),  // right, gold
        ];

        for (pos, radius, color_idx) in &rotating_configs {
            let angle_offset = if *radius > 1.3 { 0.0 } else { 120.0 };
            engine
                .build_entity(active_scene)
                .with_component(
                    TransformComponent::builder()
                        .position(*pos)
                        .rotation(Vector3f::new(0.0, angle_offset, 0.0))
                        .scale(Vector3f::new(0.8, 0.8, 0.8))
                        .build(),
                )
                .with_component(
                    MeshRenderingComponent::builder()
                        .mesh(&cube_mesh)
                        .material(&materials[*color_idx])
                        .build(),
                )
                .build();
        }

        // --- Systems ---
        engine.add_system("rotate_cubes", rotate_cubes_system)?;
        engine.add_system("orbit_cubes", orbit_cubes_system)?;

        Ok(())
    }
}

// ── Systems ─────────────────────────────────────────────────────────────

/// Slowly rotate some cubes around their own axes.
fn rotate_cubes_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;

    // Rotate every second entity (odd indices) for visual variety.
    let mut i = 0u32;
    for (_entity, transform, _mesh) in
        engine.iterate_two_components_mut::<TransformComponent, MeshRenderingComponent>()?
    {
        // Skip the floor (first entity with mesh) and every other cube.
        if i > 0 && i % 2 == 0 {
            let rot = transform.rotation;
            transform.set_rotation(Vector3f::new(
                rot.x + 1.5 * dt,
                rot.y + 2.5 * dt,
                rot.z + 0.8 * dt,
            ));
        }
        i += 1;
    }

    Ok(())
}

/// Orbit some cubes in a circle to show dynamic shadow movement.
fn orbit_cubes_system(engine: &mut Engine) -> Result<()> {
    let elapsed = engine.get_global_component::<TimeComponent>()?.time;
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

    // Orbit the last 3 mesh entities (the rotating cubes).
    // We count total entities first, then iterate.
    let total: usize = {
        engine
            .iterate_two_components_mut::<TransformComponent, MeshRenderingComponent>()?
            .count()
    };
    let start_idx = total.saturating_sub(3);

    let mut i = 0usize;
    for (_entity, transform, _mesh) in
        engine.iterate_two_components_mut::<TransformComponent, MeshRenderingComponent>()?
    {
        if i >= start_idx {
            let orbit_idx = (i - start_idx) as f32;
            let radius = 1.5 + orbit_idx * 1.0;
            let speed = 0.6 + orbit_idx * 0.3;
            let angle = elapsed * speed;

            let x = angle.cos() * radius;
            let z = 3.0 + angle.sin() * radius;
            let y_original = match orbit_idx as usize {
                0 => 2.0,
                1 => 1.5,
                _ => 1.0,
            };

            transform.set_position(Vector3f::new(x, y_original, z));

            let rot = transform.rotation;
            transform.set_rotation(Vector3f::new(
                rot.x + 1.0 * delta_time,
                rot.y + 2.0 * delta_time,
                rot.z + 0.5 * delta_time,
            ));
        }
        i += 1;
    }

    Ok(())
}
