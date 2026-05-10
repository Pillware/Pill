use pill_engine::{define_component, game::*};

use crate::bake;

define_component!(OrbitCamera {
    yaw: f32,
    pitch: f32,
    radius: f32,
});

pub struct Game {}

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        let (equirect, equirect_width, equirect_height) = bake::generate();
        let (diffuse, specular_mips, brdf_lut) =
            bake::bake_all(&equirect, equirect_width, equirect_height);

        let background_handle = engine.create_gpu_texture_f32(
            "equirect",
            &equirect,
            equirect_width,
            equirect_height,
        )?;
        let diffuse_handle = engine.create_gpu_texture_f32("diffuse_ibl", &diffuse, 32, 16)?;
        let specular_handle =
            engine.create_gpu_mipped_texture_f32("specular_ibl", &specular_mips, 128, 64)?;
        let brdf_lut_handle = engine.create_gpu_texture_f32("brdf_lut", &brdf_lut, 256, 256)?;

        let render_state = engine.get_global_component_mut::<RenderStateComponent>()?;
        render_state.background = background_handle;
        render_state.ibl_diffuse = diffuse_handle;
        render_state.ibl_specular = specular_handle;
        render_state.ibl_brdf_lut = brdf_lut_handle;

        let scene = engine.create_scene("pbr_balls")?;
        engine.set_active_scene(scene)?;

        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<CameraComponent>(scene)?;
        engine.register_component::<PbrRenderableComponent>(scene)?;
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
            PBRMaterial::new("spheres_mat")
                .albedo_texture(albedo_handle)
                .metallic_roughness_texture(metallic_roughness_handle)
                .metallic(1.0)
                .roughness(1.0),
        )?;

        engine
            .build_entity(scene)
            .with_component(TransformComponent::builder().build())
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
                    .position(Vector3f::new(0.0, 0.0, 10.0))
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(60.0)
                    .clear_color(Color::new(0.0, 0.0, 0.0))
                    .look_at(Some(Vector3f::new(0.0, 0.0, 0.0)))
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
    }
    Ok(())
}
