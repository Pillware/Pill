use pill_engine::{define_component, game::*};

use crate::cartoon_pass::{CartoonComponent, CartoonPass};

define_component!(TagAlphaComponent {});

pub struct Game {}

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // --- Basic setup ---

        // Create scene
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CartoonComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<AudioListenerComponent>(active_scene)?;
        engine.register_component::<AudioSourceComponent>(active_scene)?;
        engine.register_component::<TagAlphaComponent>(active_scene)?;

        // Add systems
        engine.add_system("rotation_system", rotation_system)?;

        // Add render passes
        engine.add_render_pass(Box::new(CartoonPass::new()));

        // --- Create resources ---

        // Add meshes
        let chimpanzini_bananini_mesh_handle = engine.add_resource(
            Mesh::new(
                "chimpanzini_bananini",
                "models/chimpanzini_bananini.obj".into(),
            )
            .with_uv_flip(true),
        )?;

        // Add textures
        let chimpanzini_bananini_color_texture_handle =
            engine.add_resource::<Texture>(Texture::new(
                "chimpanzini_bananini",
                TextureType::Color,
                ResourceLoader::Path("textures/chimpanzini_bananini_color.png".into()),
            ))?;

        // Add materials
        // Unlit look: black albedo (no lighting response) + texture as emission
        let chimpanzini_bananini_material_handle_unlit = engine.add_resource::<Material>(
            Material::new("chimpanzini_bananini_unlit")
                .albedo(Color::new(0.0, 0.0, 0.0))
                .emissive(Color::new(1.0, 1.0, 1.0))
                .emissive_texture(chimpanzini_bananini_color_texture_handle),
        )?;

        let chimpanzini_bananini_material_handle_lit = engine.add_resource::<Material>(
            Material::new("chimpanzini_bananini_lit")
                .albedo_texture(chimpanzini_bananini_color_texture_handle)
                .roughness(0.5),
        )?;

        // --- Create entities ---

        // Create camera entity
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.0, -3.0))
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(60.0)
                    .clear_color(Color::new(0.5, 0.5, 0.5))
                    .build(),
            )
            .build();

        // // Create chimpanzini bananini entity
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(-1.0, 0.0, 0.0))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .material(&chimpanzini_bananini_material_handle_lit)
                    .mesh(&chimpanzini_bananini_mesh_handle)
                    .build(),
            )
            .with_component(TagAlphaComponent {})
            .build();

        // Create chimpanzini bananini entity unlit
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(1.0, 0.0, 0.0))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .material(&chimpanzini_bananini_material_handle_unlit)
                    .mesh(&chimpanzini_bananini_mesh_handle)
                    .build(),
            )
            .with_component(TagAlphaComponent {})
            .build();

        // Create chimpanzini bananini entity drawn by the game-defined CartoonPass (posterize)
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.0, 1.5))
                    .build(),
            )
            .with_component(CartoonComponent {
                mesh: chimpanzini_bananini_mesh_handle,
                texture: chimpanzini_bananini_color_texture_handle,
                posterize_level: 3.0,
            })
            .with_component(TagAlphaComponent {})
            .build();

        Ok(())
    }
}

// --- Systems ---

fn rotation_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    println!("Delta time: {}", delta_time);

    for (_, transform_component, _) in
        engine.iterate_two_components_mut::<TransformComponent, TagAlphaComponent>()?
    {
        transform_component.rotate_around_axis(90.0 * delta_time, Vector3f::new(0.0, 1.0, 0.0));
    }

    Ok(())
}
