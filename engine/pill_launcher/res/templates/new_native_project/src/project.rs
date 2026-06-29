use pill_engine::project::*;

pub struct Project {}
create_project!(Project {}, PillProject);

impl PillProject for Project {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;

        let mesh_handle = engine.add_resource(Mesh::new("pill", "models/pill.obj"))?;

        let material_handle = engine.add_resource(
            Material::builder("default")
                .color_parameter("tint", Color::new(0.8, 0.8, 0.82))?
                .build(),
        )?;
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.0, -5.0))
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(60.0)
                    .clear_color(Color::new(0.1, 0.1, 0.11))
                    .build(),
            )
            .build();

        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.5, 0.0))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&mesh_handle)
                    .material(&material_handle)
                    .build(),
            )
            .build();

        engine.add_system("float_and_rotate", float_and_rotate_system)?;

        Ok(())
    }
}

// --- Systems ---

fn float_and_rotate_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let elapsed = engine.frame_count() as f32 * delta_time;

    for (_entity, transform, _mesh) in
        engine.iterate_two_components_mut::<TransformComponent, MeshRenderingComponent>()?
    {
        let float_offset = (elapsed * 1.5).sin() * 0.3;
        transform.set_position(Vector3f::new(
            transform.position.x,
            0.5 + float_offset,
            transform.position.z,
        ));
        transform.set_rotation(Vector3f::new(
            (elapsed * 30.0).sin() * 15.0,
            elapsed * 45.0,
            0.0,
        ));
    }

    Ok(())
}
