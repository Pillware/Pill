use pill_engine::game::*;
use rand::Rng;

// Draft import for renderer vNext API (as per DESIGN.md)
// use pill_engine::pill_renderer as pr;

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
        let _scene_benchmark = create_scene_benchmark(engine, "Benchmark 60k pills")?;
        let scene_pbr_grid: SceneHandle = create_scene_pbr_grid(engine, "PBR Grid")?;
        engine.set_active_scene(scene_pbr_grid)?;
        Ok(())
    }
}

fn create_scene_benchmark(engine: &mut Engine, name: &str) -> Result<SceneHandle> {
    let scene = engine.create_scene(name)?;

    // Register components
    engine.register_component::<TransformComponent>(scene)?;
    engine.register_component::<MeshRenderingComponent>(scene)?;
    engine.register_component::<CameraComponent>(scene)?;
    engine.register_component::<AudioListenerComponent>(scene)?;
    engine.register_component::<AudioSourceComponent>(scene)?;
    engine.register_component::<PillComponent>(scene)?;

    // Add systems
    engine.add_system("PillRotation", pill_rotation_system)?;

    // Add meshes
    let pill_mesh_handle = get_or_add_mesh(engine, "Pill", "models/pill.obj")?;
    // Add textures
    let pill_color_texture = Texture::new(
        "PillColor",
        TextureType::Gamma,
        ResourceLoadType::Path("textures/pill_color.png".into()),
    );
    let pill_color_texture_handle = engine.add_resource::<Texture>(pill_color_texture)?;
    let pill_normal_texture = Texture::new(
        "PillNormal",
        TextureType::Linear,
        ResourceLoadType::Path("textures/pill_normal.png".into()),
    );
    let pill_normal_texture_handle = engine.add_resource::<Texture>(pill_normal_texture)?;

    // Material properties showcase: create a small set of materials to reuse
    let mut rng = rand::thread_rng();
    let mut materials = Vec::new();
    for j in 0..10 {
        let mut mat: PBRMaterial = PBRMaterial::new(&format!("PillMat{}", j));
        mat.set_albedo_texture(pill_color_texture_handle.clone());
        mat.set_normal_texture(pill_normal_texture_handle.clone());
        let tint = Color::new(
            rng.gen_range(0.2..=0.8),
            rng.gen_range(0.2..=0.8),
            rng.gen_range(0.2..=0.8),
        );
        let metallic = rng.gen_range(0.0..=1.0);
        let roughness = rng.gen_range(0.2..=1.0);
        mat.set_base_color_factor(tint);
        mat.set_metallic_factor(metallic);
        mat.set_roughness_factor(roughness);
        let handle = engine.add_resource::<PBRMaterial>(mat)?;
        materials.push(handle);
    }

    // Create camera entity
    let camera = engine.create_entity(scene)?;
    let transform_component = TransformComponent::builder()
        .position(Vector3f::new(0.0, 0.0, -16.0))
        .rotation(Vector3f::new(0.0, 0.0, -20.0))
        .build();
    engine.add_component_to_entity(scene, camera, transform_component)?;
    let camera_component = CameraComponent::builder().enabled(true).build();
    engine.add_component_to_entity(scene, camera, camera_component)?;

    // Create pill entity
    for i in 0..60000 {
        let pill = engine.create_entity(scene)?;
        let posx = rng.gen_range(-10.0..=20.0);
        let posy = rng.gen_range(-10.0..=10.0);
        let posz = rng.gen_range(-1.0..=1.0);
        let rotx = rng.gen_range(-180.0..=180.0);
        let transform_component = TransformComponent::builder()
            .rotation(Vector3f::new(rotx, 0.0, 0.0))
            .position(Vector3f::new(posx, posy, posz))
            .build();
        engine.add_component_to_entity(scene, pill, transform_component)?;
        let mesh_rendering_component = MeshRenderingComponent::builder()
            .mesh(&pill_mesh_handle)
            .material(&materials[i as usize % materials.len()])
            .build();
        engine.add_component_to_entity(scene, pill, mesh_rendering_component)?;
        // engine.add_component_to_entity(scene, pill, PillComponent {})?;
    }

    Ok(scene)
}

fn create_scene_pbr_grid(engine: &mut Engine, name: &str) -> Result<SceneHandle> {
    let scene = engine.create_scene(name)?;

    // Register components
    engine.register_component::<TransformComponent>(scene)?;
    engine.register_component::<MeshRenderingComponent>(scene)?;
    engine.register_component::<CameraComponent>(scene)?;
    engine.register_component::<PillComponent>(scene)?;

    // Camera
    let camera = engine.create_entity(scene)?;
    let camera_transform = TransformComponent::builder()
        .position(Vector3f::new(0.0, 0.0, -16.0))
        .rotation(Vector3f::new(0.0, 0.0, -20.0))
        .build();
    engine.add_component_to_entity(scene, camera, camera_transform)?;
    let camera_component = CameraComponent::builder().enabled(true).build();
    engine.add_component_to_entity(scene, camera, camera_component)?;

    // Mesh (use pill as sphere substitute)
    let pill_mesh_handle = get_or_add_mesh(engine, "Pill", "models/pill.obj")?;

    // Create 5x5 grid where:
    // - X axis: roughness [0.0 .. 1.0]
    // - Y axis: metallic  [0.0 .. 1.0]
    // - Base color: (0.35, 0.35, 0.35)
    let grid_size = 5;
    let spacing = 2.5f32;
    for y in 0..grid_size {
        for x in 0..grid_size {
            let roughness = (x as f32) / ((grid_size - 1) as f32);
            let metallic = (y as f32) / ((grid_size - 1) as f32);

            let mut mat: PBRMaterial = PBRMaterial::new(&format!("PBR_{}_{}", x, y));
            mat.set_base_color_factor(Color::new(0.35, 0.35, 0.35));
            mat.set_metallic_factor(metallic);
            mat.set_roughness_factor(roughness);
            let mat_handle = engine.add_resource::<PBRMaterial>(mat)?;

            let entity = engine.create_entity(scene)?;
            let pos_x = (x as f32 - ((grid_size as f32 - 1.0) * 0.5)) * spacing;
            let pos_y = (y as f32 - ((grid_size as f32 - 1.0) * 0.5)) * spacing;
            let transform = TransformComponent::builder()
                .position(Vector3f::new(pos_x, pos_y, 0.0))
                .rotation(Vector3f::new(45.0, 45.0, 45.0))
                .build();
            engine.add_component_to_entity(scene, entity, transform)?;

            let mesh_render = MeshRenderingComponent::builder()
                .mesh(&pill_mesh_handle)
                .material(&mat_handle)
                .build();
            engine.add_component_to_entity(scene, entity, mesh_render)?;
        }
    }

    Ok(scene)
}

fn get_or_add_mesh(engine: &mut Engine, name: &str, path: &str) -> Result<MeshHandle> {
    match engine.get_resource_handle::<Mesh>(name) {
        Ok(handle) => Ok(handle),
        Err(_) => {
            let mesh = Mesh::new(name, path.into());
            engine.add_resource(mesh)
        }
    }
}

fn pill_rotation_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component_mut::<InputComponent>()?;

    // Exit on Escape
    if input_component.get_key_pressed(KeyboardKey::Escape) {
        // TODO: engine.request_shutdown();
        // TODO: standalone handle request_shutdown
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
