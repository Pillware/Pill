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
        // Release/wasm builds embed the cooked res/ as an asset pack (see build.rs); debug builds
        // leave it empty and load from disk
        let pack: &'static [u8] = include_bytes!(concat!(env!("OUT_DIR"), "/assets.pak"));
        if !pack.is_empty() {
            engine.mount_asset_pack(pack)?;
        }

        // Environment and IBL maps cooked by pill_assets from res/models/studio_small_08.hdr
        let mut load_environment = |name: &str, path: &str| -> Result<PassTextureHandle> {
            let bytes = engine.read_asset(path)?;
            engine.load_cooked_texture(name, &bytes)
        };
        let bg_h = load_environment("equirect", "models/studio_small_08_equirect.cooked_tex")?;
        let diff_h = load_environment(
            "diffuse_ibl",
            "models/studio_small_08_diffuse_ibl.cooked_tex",
        )?;
        let spec_h = load_environment(
            "specular_ibl",
            "models/studio_small_08_specular_ibl.cooked_tex",
        )?;

        {
            let rs = engine.get_global_component_mut::<RenderStateComponent>()?;
            rs.background = Some(bg_h);
            rs.ibl_diffuse = diff_h;
            rs.ibl_specular = spec_h;
            // Post chain for bloom on the emissive parts; its other effects stay off
            rs.pass_chain = PassChain::PostFx;
            rs.motion_blur = 0.0;
            rs.ssao.strength = 0.0;
        }

        let scene = engine.create_scene("helmet")?;
        engine.set_active_scene(scene)?;

        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<CameraComponent>(scene)?;
        engine.register_component::<MeshRenderingComponent>(scene)?;
        engine.register_component::<OrbitCamera>(scene)?;

        // Path-based: resolved from the asset pack when mounted, else res/<path>.cooked_*
        let mesh_handle =
            engine.add_resource(Mesh::new("helmet_mesh", "models/DamagedHelmet".into()))?;
        let mut add_texture = |name: &str, kind: TextureType, path: &str| {
            engine.add_resource(Texture::new(name, kind, ResourceLoader::Path(path.into())))
        };
        let albedo_handle = add_texture(
            "helmet_albedo",
            TextureType::Color,
            "models/DamagedHelmet_albedo",
        )?;
        let normal_handle = add_texture(
            "helmet_normal",
            TextureType::Normal,
            "models/DamagedHelmet_normal",
        )?;
        let metallic_roughness_handle = add_texture(
            "helmet_metallic_roughness",
            TextureType::MetallicRoughness,
            "models/DamagedHelmet_metallic_roughness",
        )?;
        let emissive_handle = add_texture(
            "helmet_emissive",
            TextureType::Emissive,
            "models/DamagedHelmet_emissive",
        )?;

        let material_handle = engine.add_resource(
            Material::new("helmet_material")
                .albedo_texture(albedo_handle)
                .normal_texture(normal_handle)
                .metallic_roughness_texture(metallic_roughness_handle)
                .metallic(1.0)
                .roughness(1.0)
                .emissive(Color::new(100.0, 100.0, 100.0))
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
