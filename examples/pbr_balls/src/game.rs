use pill_engine::{define_component, game::*};

define_component!(OrbitCamera {
    yaw: f32,
    pitch: f32,
    radius: f32,
});

pub struct Game {}

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // Environment and IBL maps cooked by pill_assets from res/studio.procedural_equirect
        let background_handle = engine.load_cooked_texture(
            "equirect",
            include_bytes!("../res/studio_equirect.cooked_tex"),
        )?;
        let diffuse_handle = engine.load_cooked_texture(
            "diffuse_ibl",
            include_bytes!("../res/studio_diffuse_ibl.cooked_tex"),
        )?;
        let specular_handle = engine.load_cooked_texture(
            "specular_ibl",
            include_bytes!("../res/studio_specular_ibl.cooked_tex"),
        )?;
        let brdf_lut_handle = engine.load_cooked_texture(
            "brdf_lut",
            include_bytes!("../res/brdf_lut.cooked_tex"),
        )?;

        let render_state = engine.get_global_component_mut::<RenderStateComponent>()?;
        render_state.background = Some(background_handle);
        render_state.ibl_diffuse = diffuse_handle;
        render_state.ibl_specular = specular_handle;
        render_state.ibl_brdf_lut = brdf_lut_handle;

        let scene = engine.create_scene("pbr_balls")?;
        engine.set_active_scene(scene)?;

        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<CameraComponent>(scene)?;
        engine.register_component::<MeshRenderingComponent>(scene)?;
        engine.register_component::<OrbitCamera>(scene)?;

        engine.add_system("orbit_camera", orbit_camera_system)?;

        let mesh_handle = engine.add_resource(Mesh::from_cooked_mesh_bytes(
            "spheres_mesh",
            include_bytes!("../res/models/MetalRoughSpheres.cooked_mesh"),
        )?)?;

        let albedo_handle = engine.add_resource(Texture::from_bytes(
            "spheres_albedo",
            TextureType::Color,
            include_bytes!("../res/models/MetalRoughSpheres_albedo.cooked_tex"),
        ))?;

        let metallic_roughness_handle = engine.add_resource(Texture::from_bytes(
            "spheres_metallic_roughness",
            TextureType::MetallicRoughness,
            include_bytes!("../res/models/MetalRoughSpheres_metallic_roughness.cooked_tex"),
        ))?;

        let material_handle = engine.add_resource(
            Material::new("spheres_mat")
                .albedo_texture(albedo_handle)
                .metallic_roughness_texture(metallic_roughness_handle)
                .metallic(1.0)
                .roughness(1.0),
        )?;

        engine
            .build_entity(scene)
            .with_component(TransformComponent::builder().build())
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&mesh_handle)
                    .material(&material_handle)
                    .build(),
            )
            .build();

        engine
            .build_entity(scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 0.0, 10.0))
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(60.0)
                    .build(),
            )
            .with_component(OrbitCamera {
                yaw: 0.0,
                pitch: 0.0,
                radius: 10.0,
            })
            .build();

        Ok(())
    }
}

fn orbit_camera_system(engine: &mut Engine) -> Result<()> {
    let input = engine.get_global_component_mut::<InputComponent>()?;
    let mouse_delta = input.get_mouse_delta();
    let scroll_delta = input.get_mouse_scroll_delta();
    let left_mouse_button = input.get_mouse_button(MouseButton::Left);

    for (_, transform, orbit) in
        engine.iterate_two_components_mut::<TransformComponent, OrbitCamera>()?
    {
        if left_mouse_button {
            orbit.yaw -= mouse_delta.x * 0.3;
            orbit.pitch = (orbit.pitch + mouse_delta.y * 0.3).clamp(-70.0, 70.0);
        }
        orbit.radius = (orbit.radius - scroll_delta.y * 0.5).clamp(1.0, 50.0);

        let pitch_radians = orbit.pitch.to_radians();
        let yaw_radians = orbit.yaw.to_radians();
        transform.set_position(Vector3f::new(
            orbit.radius * pitch_radians.cos() * yaw_radians.sin(),
            orbit.radius * pitch_radians.sin(),
            orbit.radius * pitch_radians.cos() * yaw_radians.cos(),
        ));
        transform.set_rotation(Vector3f::new(orbit.pitch, orbit.yaw + 180.0, 0.0));
    }
    Ok(())
}
