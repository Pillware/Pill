use pill_engine::{define_component, game::*};

pub struct Game {}

define_component!(CubeComponent {});

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // --- Cube that can be manipulated via C# scripts ---

        // Create scene
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<AudioListenerComponent>(active_scene)?;
        engine.register_component::<AudioSourceComponent>(active_scene)?;
        engine.register_component::<CubeComponent>(active_scene)?;

        // Add systems
        engine.add_system("dummy_system", dummy_system)?;

        // --- Create resources ---

        // Add meshes
        let pill_mesh = Mesh::new("pill", "models/pill.obj".into());
        let pill_mesh_handle = engine.add_resource(pill_mesh)?;

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

        let organic_material_handle = engine.add_resource::<Material>(
            Material::builder("organic")
                .texture("color", organic_color_texture_handle)?
                .texture("normal", organic_normal_texture_handle)?
                .color_parameter("tint", Color::new(0.26, 0.87, 0.9))?
                .scalar_parameter("specularity", 3.0)?
                .build(),
        )?;

        // --- Create entities ---
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
            .with_component(CubeComponent {})
            .build();

        engine
            .build_entity(active_scene)
            .with_component(TransformComponent::new())
            .with_component(
                MeshRenderingComponent::builder()
                    .material(&organic_material_handle)
                    .mesh(&pill_mesh_handle)
                    .build(),
            )
            .build();

        Ok(())
    }
}

fn dummy_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    println!("Delta time: {}", delta_time);

    Ok(())
}
