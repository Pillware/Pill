use pill_engine::game::*;

// Define custom component
pub struct PillComponent { }

impl Component for PillComponent { }

impl PillTypeMapKey for PillComponent {
    type Storage = ComponentStorage<Self>;
}

// Game
pub struct Game { } 

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
        let pill_mesh = Mesh::new("Pill", "./res/models/Pill.obj".into());
        let pill_mesh_handle = engine.add_resource(pill_mesh)?;

        // Add textures
        let pill_color_texture = Texture::new("PillColor", TextureType::Color, ResourceLoadType::Path("./res/textures/PillColor.png".into()));
        let pill_color_texture_handle = engine.add_resource::<Texture>(pill_color_texture)?;
        let pill_normal_texture = Texture::new("PillNormal", TextureType::Normal, ResourceLoadType::Path("./res/textures/PillNormal.png".into()));
        let pill_normal_texture_handle = engine.add_resource::<Texture>(pill_normal_texture)?;

        // Add materials
        let mut pill_material = Material::new("Pill");
        pill_material.set_texture("Color", pill_color_texture_handle)?;
        pill_material.set_texture("Normal", pill_normal_texture_handle)?;
        pill_material.set_color("Tint", Color::new( 1.0, 1.0, 1.0))?;
        pill_material.set_scalar("Specularity", 0.5)?; 
        let pill_material_handle = engine.add_resource::<Material>(pill_material)?; 

        // Create camera entity
        let camera = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(0.0,0.0,-8.0))
            .rotation(Vector3f::new(0.0,0.0,-20.0))
            .build();
        engine.add_component_to_entity(active_scene, camera, transform_component)?;
        let camera_component = CameraComponent::builder().enabled(true).build();
        engine.add_component_to_entity(active_scene, camera, camera_component)?;

        // Create pill entity
        let pill = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .rotation(Vector3f::new(-210.0,0.0,0.0))
            .build();
        engine.add_component_to_entity(active_scene, pill, transform_component)?;
        let mesh_rendering_component = MeshRenderingComponent::builder()
            .mesh(&pill_mesh_handle)
            .material(&pill_material_handle)
            .build();
        engine.add_component_to_entity(active_scene, pill, mesh_rendering_component)?;  
        engine.add_component_to_entity(active_scene, pill, PillComponent {})?; 

        Ok(())
    }
}

fn pill_rotation_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component_mut::<InputComponent>()?;

    // Rotate pill if spacebar is not pressed
    if !input_component.get_key_pressed(KeyboardKey::Space) {
        for (_, transform_component, _) in engine.iterate_two_components_mut::<TransformComponent, PillComponent>()? {
            transform_component.rotation += Vector3f::new(0.0,1.0,0.0) * 100.0 * delta_time;
        }
    }

    Ok(())
}
