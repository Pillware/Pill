use pill_engine::game::*;
use rand::Rng;

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

        // Add systems
        engine.add_system("PillRotation", pill_rotation_system)?;

        // Add meshes
        let pill_mesh = Mesh::new("Pill", "models/pill.obj".into());
        let pill_mesh_handle = engine.add_resource(pill_mesh)?;
        // Add textures
        let pill_color_texture = Texture::new(
            "PillColor",
            TextureType::Color,
            ResourceLoadType::Path("textures/pill_color.png".into()),
        );
        let pill_color_texture_handle = engine.add_resource::<Texture>(pill_color_texture)?;
        let pill_normal_texture = Texture::new(
            "PillNormal",
            TextureType::Normal,
            ResourceLoadType::Path("textures/pill_normal.png".into()),
        );
        let pill_normal_texture_handle = engine.add_resource::<Texture>(pill_normal_texture)?;
        // Material properties showcase: create a small set of materials to reuse
        let mut rng = rand::thread_rng();
        let mut materials = Vec::new();
        for j in 0..8 {
            let mut mat = Material::new(&format!("PillMat{}", j));
            mat.set_texture("color", pill_color_texture_handle.clone())?;
            mat.set_texture("normal", pill_normal_texture_handle.clone())?;
            let tint = Color::new(
                rng.gen_range(0.3..=1.0),
                rng.gen_range(0.3..=1.0),
                rng.gen_range(0.3..=1.0),
            );
            let spec = rng.gen_range(0.0..=1.0);
            mat.set_color("tint", tint)?;
            mat.set_scalar("specularity", spec)?;
            let handle = engine.add_resource::<Material>(mat)?;
            materials.push(handle);
        }

        // Create camera entity
        let camera = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(0.0, 0.0, -16.0))
            .rotation(Vector3f::new(0.0, 0.0, -20.0))
            .build();
        engine.add_component_to_entity(active_scene, camera, transform_component)?;
        let camera_component = CameraComponent::builder().enabled(true).build();
        engine.add_component_to_entity(active_scene, camera, camera_component)?;

        // Create pill entity
        for i in 0..50000 {
            let pill = engine.create_entity(active_scene)?;
            let posx = rng.gen_range(-10.0..=20.0);
            let posy = rng.gen_range(-10.0..=10.0);
            let posz = rng.gen_range(-1.0..=1.0);
            let rotx = rng.gen_range(-180.0..=180.0);
            let transform_component = TransformComponent::builder()
                .rotation(Vector3f::new(rotx, 0.0, 0.0))
                .position(Vector3f::new(posx, posy, posz))
                .build();
            engine.add_component_to_entity(active_scene, pill, transform_component)?;
            let mesh_rendering_component = MeshRenderingComponent::builder()
                .mesh(&pill_mesh_handle)
                .material(&materials[i as usize % materials.len()])
                .build();
            engine.add_component_to_entity(active_scene, pill, mesh_rendering_component)?;
            engine.add_component_to_entity(active_scene, pill, PillComponent {})?;
        }

        Ok(())
    }
}

fn pill_rotation_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component_mut::<InputComponent>()?;

    // Exit on Escape
    if input_component.get_key_pressed(KeyboardKey::Escape) {
        std::process::exit(0);
    }

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
