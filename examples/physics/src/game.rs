use pill_engine::game::*;

// Define custom component
pub struct PillComponent {}

impl Component for PillComponent {}

impl PillTypeMapKey for PillComponent {
    type Storage = ComponentStorage<Self>;
}

// Game
pub struct Game {}

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // Create scene
        let active_scene = engine.create_scene("Default")?;
        engine.set_active_scene(active_scene)?;

        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<AudioListenerComponent>(active_scene)?;
        engine.register_component::<AudioSourceComponent>(active_scene)?;
        engine.register_component::<PillComponent>(active_scene)?;
        engine.register_component::<RigidBodyComponent>(active_scene)?;
        engine.register_component::<ColliderComponent>(active_scene)?;

        // Add systems
        //engine.add_system("PillRotation", pill_rotation_system)?;

        // Add meshes
        let pill_mesh_handle = engine.add_resource(Mesh::from_cooked_mesh_bytes(
            "Pill",
            include_bytes!("../res/models/pill.cooked_mesh"),
        )?)?;

        // Add textures
        let pill_color_texture = Texture::from_bytes(
            "PillColor",
            TextureType::Color,
            include_bytes!("../res/textures/pill_color.cooked_tex"),
        );
        let pill_color_texture_handle = engine.add_resource::<Texture>(pill_color_texture)?;
        let pill_normal_texture = Texture::from_bytes(
            "PillNormal",
            TextureType::Normal,
            include_bytes!("../res/textures/pill_normal.cooked_tex"),
        );
        let pill_normal_texture_handle = engine.add_resource::<Texture>(pill_normal_texture)?;

        // Add materials
        let pill_material = Material::builder("Pill")
            .texture("color", pill_color_texture_handle)?
            .texture("normal", pill_normal_texture_handle)?
            .color_parameter("tint", Color::new(1.0, 1.0, 1.0))?
            .scalar_parameter("specularity", 0.5)?
            .build();
        let pill_material_handle = engine.add_resource::<Material>(pill_material)?;

        let ground_material = Material::builder("Ground")
            .texture("color", pill_color_texture_handle)?
            .texture("normal", pill_normal_texture_handle)?
            .color_parameter("tint", Color::new(0.0, 1.0, 0.0))?
            .scalar_parameter("specularity", 0.5)?
            .build();
        let ground_material_handle = engine.add_resource::<Material>(ground_material)?;

        // Create camera entity
        let camera = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(0.0, 6.0, -10.0))
            .rotation(Vector3f::new(0.0, 0.0, -20.0))
            .build();
        engine.add_component_to_entity(active_scene, camera, transform_component)?;
        let camera_component = CameraComponent::builder().enabled(true).build();
        engine.add_component_to_entity(active_scene, camera, camera_component)?;

        // Create pill entity
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 15.0, 0.0))
                    .rotation(Vector3f::new(-210.0, 0.0, 0.0))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&pill_mesh_handle)
                    .material(&pill_material_handle)
                    .build(),
            )
            .with_component(PillComponent {})
            .with_component(
                RigidBodyComponent::builder()
                    .body_type(RigidBodyType::Dynamic)
                    .build(),
            )
            .with_component(
                ColliderComponent::builder()
                    .shape(SharedShape::ball(3.0))
                    .mass(100.0)
                    .build(),
            )
            .build();

        let ground_mesh_handle = engine.add_resource(Mesh::from_cooked_mesh_bytes(
            "Ground",
            include_bytes!("../res/models/plane.cooked_mesh"),
        )?)?;

        // Create ground entity
        engine
            .build_entity(active_scene)
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
            .with_component(
                RigidBodyComponent::builder()
                    .body_type(RigidBodyType::Fixed)
                    .build(),
            )
            .with_component(
                ColliderComponent::builder()
                    .shape(SharedShape::cuboid(200.0, 0.5, 200.0))
                    .build(),
            )
            .build();

        Ok(())
    }
}

fn pill_rotation_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component_mut::<InputComponent>()?;

    // Rotate pill if spacebar is not pressed
    if !input_component.get_key_pressed(KeyboardKey::Space) {
        for (_, transform_component, _) in
            engine.iterate_two_components_mut::<TransformComponent, PillComponent>()?
        {
            transform_component.rotate_around_axis(90.0 * delta_time, Vector3f::new(0.0, 1.0, 0.0));
        }
    }

    Ok(())
}
