use pill_engine::{define_component, game::*};

define_component!(OrbitCamera {
    yaw: f32,
    pitch: f32,
    radius: f32,
});

pub struct Game {}

fn orbit_camera_system(engine: &mut Engine) -> Result<()> {
    let input = engine.get_global_component_mut::<InputComponent>()?;
    let mouse_delta = input.get_mouse_delta();
    let scroll_delta = input.get_mouse_scroll_delta();
    let left_mouse_button_held = input.get_mouse_button(MouseButton::Left);

    for (_, transform, orbit) in
        engine.iterate_two_components_mut::<TransformComponent, OrbitCamera>()?
    {
        if left_mouse_button_held {
            orbit.yaw -= mouse_delta.x * 0.3;
            orbit.pitch = (orbit.pitch + mouse_delta.y * 0.3).clamp(-70.0, 70.0);
        }
        orbit.radius = (orbit.radius - scroll_delta.y * 0.5).clamp(1.0, 20.0);

        let pitch_radians = orbit.pitch.to_radians();
        let yaw_radians = orbit.yaw.to_radians();
        let radius_cos_pitch = orbit.radius * pitch_radians.cos();
        transform.set_position(Vector3f::new(
            radius_cos_pitch * yaw_radians.sin(),
            orbit.radius * pitch_radians.sin(),
            radius_cos_pitch * yaw_radians.cos(),
        ));
        transform.look_at(Vector3f::ZERO);
    }
    Ok(())
}

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // Environment and IBL maps cooked by pill_assets from res/models/studio_small_08.hdr
        let bg_h = engine.load_cooked_texture(
            "equirect",
            include_bytes!("../res/models/studio_small_08_equirect.cooked_tex"),
        )?;
        let diff_h = engine.load_cooked_texture(
            "diffuse_ibl",
            include_bytes!("../res/models/studio_small_08_diffuse_ibl.cooked_tex"),
        )?;
        let spec_h = engine.load_cooked_texture(
            "specular_ibl",
            include_bytes!("../res/models/studio_small_08_specular_ibl.cooked_tex"),
        )?;

        {
            let rs = engine.get_global_component_mut::<RenderStateComponent>()?;
            rs.background = Some(bg_h);
            rs.ibl_diffuse = diff_h;
            rs.ibl_specular = spec_h;
        }

        let scene = engine.create_scene("helmet")?;
        engine.set_active_scene(scene)?;

        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<CameraComponent>(scene)?;
        engine.register_component::<MeshRenderingComponent>(scene)?;
        engine.register_component::<OrbitCamera>(scene)?;

        let mesh_handle = engine.add_resource(Mesh::from_cooked_mesh_bytes(
            "helmet_mesh",
            include_bytes!("../res/models/DamagedHelmet.cooked_mesh"),
        )?)?;

        let albedo_handle = engine.add_resource(Texture::from_bytes(
            "helmet_albedo",
            TextureType::Color,
            include_bytes!("../res/models/DamagedHelmet_albedo.cooked_tex"),
        ))?;

        let normal_handle = engine.add_resource(Texture::from_bytes(
            "helmet_normal",
            TextureType::Normal,
            include_bytes!("../res/models/DamagedHelmet_normal.cooked_tex"),
        ))?;

        let metallic_roughness_handle = engine.add_resource(Texture::from_bytes(
            "helmet_metallic_roughness",
            TextureType::MetallicRoughness,
            include_bytes!("../res/models/DamagedHelmet_metallic_roughness.cooked_tex"),
        ))?;

        let emissive_handle = engine.add_resource(Texture::from_bytes(
            "helmet_emissive",
            TextureType::Emissive,
            include_bytes!("../res/models/DamagedHelmet_emissive.cooked_tex"),
        ))?;

        let material_handle = engine.add_resource(
            Material::new("helmet_material")
                .albedo_texture(albedo_handle)
                .normal_texture(normal_handle)
                .metallic_roughness_texture(metallic_roughness_handle)
                .metallic(1.0)
                .roughness(1.0)
                .emissive(Color::new(1.0, 1.0, 1.0))
                .emissive_texture(emissive_handle),
        )?;

        engine
            .build_entity(scene)
            .with_component(
                TransformComponent::builder()
                    .rotation(Vector3f::new(0.0, 0.0, 0.0))
                    .build(),
            )
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
                    .position(Vector3f::new(0.0, 0.0, 2.0))
                    .build(),
            )
            .with_component(CameraComponent::builder().enabled(true).fov(60.0).build())
            .with_component(OrbitCamera {
                yaw: 0.0,
                pitch: 0.0,
                radius: 2.5,
            })
            .build();

        engine.add_system("orbit_camera", orbit_camera_system)?;

        Ok(())
    }
}
