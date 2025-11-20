use pill_engine::{define_component, game::*};
define_component!(TagAlphaComponent { });

pub struct Game { } 

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {

		// --- Basic setup ---

		// Create scene
		let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;
   
        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
		engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
		engine.register_component::<AudioListenerComponent>(active_scene)?;
		engine.register_component::<AudioSourceComponent>(active_scene)?;
		engine.register_component::<TagAlphaComponent>(active_scene)?;

		// Add systems
        engine.add_system("rotation_system", rotation_system)?;

		// --- Create resources ---

		// Add shaders
		let cartoon_shader_handle = engine.add_resource(
            Shader::new(
                "cartoon", 
                ResourceLoader::Path("shaders/default_vertex.glsl".into()),
                ResourceLoader::Path("shaders/cartoon_fragment.glsl".into()),
                vec![
                    ("posterize_level".to_string(), ShaderParameterSlot::new(ShaderParameterType::Scalar))
                ].into_iter().collect(),
                vec![
                    ("color".to_string(), ShaderTextureSlot::new(TextureType::Color, (0, 1)))
                ].into_iter().collect(),
                true,
                true
            )
        )?;


		// let force_field_shader_handle = engine.add_resource(
        //     Shader::new(
        //         "force_field", 
        //         ResourceLoader::Path("shaders/default_vertex.glsl".into()),
        //         ResourceLoader::Path("shaders/force_field_fragment.glsl".into()),
        //         vec![
        //             (
        //                 "posterize_level".to_string(), 
        //                 ShaderParameterSlot::newShaderParameterType::Scalar)
        //             )
        //         ].into_iter().collect(),
        //         vec![
        //             (
        //             "color".to_string(), 
        //                 ShaderTextureSlot::new(TextureType::Color, (0, 1))
        //             )
        //         ].into_iter().collect(),
        //         true,
        //         true
        //     )
        // )?;


		// Add meshes
        let chimpanzini_bananini_mesh_handle = engine.add_resource(
			Mesh::new("chimpanzini_bananini", "models/chimpanzini_bananini.obj".into()).with_uv_flip(true)
		)?;

		// Add textures
        let chimpanzini_bananini_color_texture_handle = engine.add_resource::<Texture>(
			Texture::new(
				"chimpanzini_bananini", 
				TextureType::Color, 
				ResourceLoader::Path("textures/chimpanzini_bananini_color.jpg".into())
			)
		)?;
       
	   println!("Added resources!!!!!!!!!!!!!!!");
		// Add materials
		let chimpanzini_bananini_material_handle = engine.add_resource::<Material>(
			Material::builder("chimpanzini_bananini_cartoon")
    			.shader(cartoon_shader_handle)?
				.texture("color", chimpanzini_bananini_color_texture_handle)?
				.scalar_parameter("posterize_level",  3.0)?
				.build()
		)?;

		
		let default_unlit_shader_handle = engine.get_resource_handle::<Shader>("pill_engine_default_unlit_shader")?;
		let chimpanzini_bananini_material_handle_unlit = engine.add_resource::<Material>(
			Material::builder("chimpanzini_bananini_unlit")
                .shader(default_unlit_shader_handle)?
				.texture("color", chimpanzini_bananini_color_texture_handle)?
				.color_parameter("tint", Color::new(1.0, 1.0, 1.0))?
				.build()
		)?;

		let chimpanzini_bananini_material_handle_lit = engine.add_resource::<Material>(
			Material::builder("chimpanzini_bananini_lit")
				.texture("color", chimpanzini_bananini_color_texture_handle)?
				.color_parameter("tint", Color::new(1.0, 1.0, 1.0))?
				.scalar_parameter("specularity", 0.5)?
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

		// // Create chimpanzini bananini entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(-1.0, 0.0, 0.0))
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&chimpanzini_bananini_material_handle_lit)
				.mesh(&chimpanzini_bananini_mesh_handle)
				.build())
			.with_component(TagAlphaComponent {})
			.build();

		// Create chimpanzini bananini entity unlit
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(1.0, 0.0, 0.0))
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&chimpanzini_bananini_material_handle_unlit)
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
	println!("Delta time: {}", delta_time);

	for (_, transform_component, _) in engine.iterate_two_components_mut::<TransformComponent, TagAlphaComponent>()? {
		transform_component.rotate_around_axis(90.0 * delta_time, Vector3f::new(0.0, 1.0, 0.0));
	}

	Ok(())
}