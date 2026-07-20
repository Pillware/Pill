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

        // --- Rotating cubes system ───────────────────────────────────
        engine.add_system("rotate_cubes", rotate_cubes_system)?;

        Ok(())
    }
}

// ── Systems ─────────────────────────────────────────────────────────────

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
