use pill_engine::{define_component, game::*};

define_component!(TagAlphaComponent { });

pub struct Game { } 

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {

		// --- Basic setup ---

		// Create scene
		let active_scene = engine.create_scene("Default")?;
        engine.set_active_scene(active_scene)?;
   
        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
		engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
		engine.register_component::<AudioListenerComponent>(active_scene)?;
		engine.register_component::<AudioSourceComponent>(active_scene)?;
		engine.register_component::<TagAlphaComponent>(active_scene)?;

		// Add systems
        engine.add_system("RotationSystem", rotation_system)?;

		// --- Create resources ---

		// Add meshes
        let chimpanzini_bananini_mesh_handle = engine.add_resource(
			Mesh::new("ChimpanziniBananini", "./res/models/ChimpanziniBananini.obj".into())
		)?;

		// Add textures
        let chimpanzini_bananini_color_texture_handle = engine.add_resource::<Texture>(
			Texture::new(
				"ChimpanziniBananini", 
				TextureType::Color, 
				ResourceLoadType::Path("./res/textures/ChimpanziniBananini.jpg".into())
			)
		)?;
       
		// Add materials
		let chimpanzini_bananini_material_handle = engine.add_resource::<Material>(
			Material::builder("ChimpanziniBananini")
				.texture("Color", chimpanzini_bananini_color_texture_handle)?
				.color("Tint", Color::new(1.0, 1.0, 1.0))?
				.scalar("Specularity", 0.5)?
				.build()
		)?;

		// --- Create entities ---

		// Create camera entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(0.0, 0.0, -3.0))
				.build())
			.with_component(CameraComponent::builder()
				.enabled(true)
				.fov(60.0)
				.clear_color(Color::new(0.5, 0.5, 0.5))
				.build())
			.build();

		// Create chimpanzini bananini entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::new())
			.with_component(MeshRenderingComponent::builder()
				.material(&chimpanzini_bananini_material_handle)
				.mesh(&chimpanzini_bananini_mesh_handle)
				.build())
			.with_component(TagAlphaComponent {})
			.build();

		Ok(())
	}

}

// --- Systems ---

fn rotation_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;

	for (_, transform_transform, _) in engine.iterate_two_components_mut::<TransformComponent, TagAlphaComponent>()? {
		transform_transform.rotation.y += 90.0 * delta_time;
	}

	Ok(())
}