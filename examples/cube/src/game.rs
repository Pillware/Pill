use pill_engine::game::*;

pub struct WebGame {}

fn rotate_cubes_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;

    for (_entity, transform, _mesh) in
        engine.iterate_two_components_mut::<TransformComponent, MeshRenderingComponent>()?
    {
        // Rotation is in degrees: 2.0 / 3.5 / 1.0 rad/s
        let rot = transform.rotation;
        transform.set_rotation(Vector3f::new(
            rot.x + (2.0 * dt).to_degrees(),
            rot.y + (3.5 * dt).to_degrees(),
            rot.z + (1.0 * dt).to_degrees(),
        ));
    }

    Ok(())
}

impl PillGame for WebGame {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;
        engine
            .get_global_component_mut::<RenderStateComponent>()?
            .background_color = [0.10, 0.10, 0.11];

        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;

        let cube_mesh_handle = engine.add_resource(Mesh::cube("cube", 2.0))?;
        let material_handle = engine
            .add_resource(Material::new("cube_material").albedo(Color::new(0.80, 0.80, 0.82)))?;

        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.0, -5.0))
                    .build(),
            )
            .with_component(CameraComponent::builder().enabled(true).fov(60.0).build())
            .build();

        engine
            .build_entity(active_scene)
            .with_component(TransformComponent::new())
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&cube_mesh_handle)
                    .material(&material_handle)
                    .build(),
            )
            .build();

        engine.add_system("rotate_cubes", rotate_cubes_system)?;

        Ok(())
    }
}
