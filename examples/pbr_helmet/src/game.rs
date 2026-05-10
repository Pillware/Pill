use pill_engine::{define_component, game::*};

use crate::bake;

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
    }
    Ok(())
}

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        let (eq, eq_w, eq_h) = bake::load_equirect();
        let (diffuse, specular_mips, brdf_lut) = bake::bake_all(&eq, eq_w, eq_h);

        let bg_h = engine.create_gpu_texture_f32("equirect", &eq, eq_w, eq_h)?;
        let diff_h = engine.create_gpu_texture_f32("diffuse_ibl", &diffuse, 32, 16)?;
        let spec_h =
            engine.create_gpu_mipped_texture_f32("specular_ibl", &specular_mips, 128, 64)?;
        let lut_h = engine.create_gpu_texture_f32("brdf_lut", &brdf_lut, 256, 256)?;

        {
            let rs = engine.get_global_component_mut::<RenderStateComponent>()?;
            rs.background = bg_h;
            rs.ibl_diffuse = diff_h;
            rs.ibl_specular = spec_h;
            rs.ibl_brdf_lut = lut_h;
        }

        let scene = engine.create_scene("helmet")?;
        engine.set_active_scene(scene)?;

        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<CameraComponent>(scene)?;
        engine.register_component::<PbrRenderableComponent>(scene)?;
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
            PBRMaterial::new("helmet_material")
                .albedo_texture(albedo_handle)
                .normal_texture(normal_handle)
                .metallic_roughness_texture(metallic_roughness_handle)
                .metallic(1.0)
                .roughness(1.0)
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
                PbrRenderableComponent::builder()
                    .mesh(&mesh_handle)
                    .pbr_material(&material_handle)
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
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(60.0)
                    .clear_color(Color::new(0.05, 0.05, 0.06))
                    .look_at(Some(Vector3f::new(0.0, 0.0, 0.0)))
                    .build(),
            )
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
