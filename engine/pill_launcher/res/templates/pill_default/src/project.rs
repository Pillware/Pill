use pill_engine::project::*;

pub struct Project {}

impl PillProject for Project {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;

        let mesh_handle = engine.add_resource(Mesh::cube("cube", 1.0))?;
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
            .with_component(TransformComponent::new())
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&mesh_handle)
                    .material(&material_handle)
                    .build(),
            )
            .build();

        Ok(())
    }
}
