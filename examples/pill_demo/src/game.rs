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
        let pill_mesh = Mesh::new("Pill", "models/pill.obj".into());
        let pill_mesh_handle = engine.add_resource(pill_mesh)?;
        // Add textures
        let pill_color_texture = Texture::new(
            "PillColor",
            TextureType::Color,
            ResourceLoadType::Path("textures/pill_color.png".into()),
        );
        let pill_color_texture_handle = engine.add_resource::<Texture>(pill_color_texture)?;
        let pill_normal_texture = Texture::new(
            "PillNormal",
            TextureType::Normal,
            ResourceLoadType::Path("textures/pill_normal.png".into()),
        );
        let pill_normal_texture_handle = engine.add_resource::<Texture>(pill_normal_texture)?;

        // Material properties showcase: create a small set of materials to reuse
        let mut rng = rand::thread_rng();
        let mut materials = Vec::new();
        for j in 0..10 {
            let mut mat = Material::new(&format!("PillMat{}", j));
            mat.set_albedo_texture(pill_color_texture_handle.clone());
            mat.set_normal_texture(pill_normal_texture_handle.clone());
            let tint = Color::new(
                rng.gen_range(0.2..=0.8),
                rng.gen_range(0.2..=0.8),
                rng.gen_range(0.2..=0.8),
            );
            let spec = rng.gen_range(0.0..=1.0);
            mat.set_base_color_factor(tint);
            mat.set_metallic_factor(spec);
            let handle = engine.add_resource::<Material>(mat)?;
            materials.push(handle);
        }

        // Create camera entity
        let camera = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(0.0, 0.0, -16.0))
            .rotation(Vector3f::new(0.0, 0.0, -20.0))
            .build();
        engine.add_component_to_entity(active_scene, camera, transform_component)?;
        let camera_component = CameraComponent::builder().enabled(true).build();
        engine.add_component_to_entity(active_scene, camera, camera_component)?;

        // Create pill entity
        for i in 0..60000 {
            let pill = engine.create_entity(active_scene)?;
            let posx = rng.gen_range(-10.0..=20.0);
            let posy = rng.gen_range(-10.0..=10.0);
            let posz = rng.gen_range(-1.0..=1.0);
            let rotx = rng.gen_range(-180.0..=180.0);
            let transform_component = TransformComponent::builder()
                .rotation(Vector3f::new(rotx, 0.0, 0.0))
                .position(Vector3f::new(posx, posy, posz))
                .build();
            engine.add_component_to_entity(active_scene, pill, transform_component)?;
            let mesh_rendering_component = MeshRenderingComponent::builder()
                .mesh(&pill_mesh_handle)
                .material(&materials[i as usize % materials.len()])
                .build();
            engine.add_component_to_entity(active_scene, pill, mesh_rendering_component)?;
            engine.add_component_to_entity(active_scene, pill, PillComponent {})?;
        }

        /* --- Overlay Logo Pass (userland draft using pill_renderer vNext API) ---
        {
            // Acquire renderer and its resource manager
            let renderer = engine.renderer_mut()?; // Draft API accessor
            let mut rm = renderer.resources();

            // Create logo texture + sampler → material bind group (set 1)
            let logo_tex =
                rm.create_texture(pr::TextureDesc::from_path("textures/pill_logo.png").srgb(true));
            let logo_smp = rm.create_sampler(pr::SamplerDesc::linear_clamp());

            // Overlay pipeline (fixed-function screen space quad shader)
            let overlay_pso = rm.create_pipeline(pr::GraphicsPipelineDesc::overlay_logo(
                renderer.surface_format(),
            ));

            // Material BG uses @group(1): texture_2d + sampler
            let overlay_material_bg = rm.create_bind_group(pr::BindGroupDesc {
                debug_name: Some("overlay_logo_material"),
                layout: overlay_pso.material_layout(),
                textures: vec![logo_tex],
                samplers: vec![logo_smp],
                ..Default::default()
            });

            // Per-pass globals (@group(0)) with screen-space rect in pixels and tint
            #[repr(C)]
            #[derive(Copy, Clone)]
            struct OverlayGlobals {
                rect_px: [f32; 4],
                tint: [f32; 4],
            }
            let globals = OverlayGlobals {
                rect_px: [16.0, 16.0, 160.0, 64.0],
                tint: [1.0, 1.0, 1.0, 1.0],
            };
            let globals_bg = {
                // 256B-aligned uniform buffer with initial data
                let globals_buf = rm.create_buffer(pr::BufferDesc::uniform_init_aligned(
                    "overlay_logo_globals",
                    pr::bytes_of(&globals),
                ));
                rm.create_bind_group(pr::BindGroupDesc {
                    debug_name: Some("overlay_logo_globals"),
                    layout: overlay_pso.globals_layout(),
                    buffers: vec![pr::BufferBinding {
                        buffer: globals_buf,
                        byte_offset: 0,
                    }],
                    ..Default::default()
                })
            };

            // Tiny quad mesh (two triangles in NDC via VS or a unit quad mesh)
            let quad_mesh = rm.upload_mesh(pr::CpuMesh::unit_quad());

            // Add pass to master pipeline that draws the logo over the swapchain
            let pass = pr::RenderPassDesc {
                name: "overlay_logo",
                target: pr::TargetDesc::Swapchain,
                clear: pr::ClearDesc {
                    color: None,
                    depth: None,
                    stencil: None,
                },
                subpipeline: pr::Subpipeline {
                    pipeline: overlay_pso,
                    globals: globals_bg,
                    draws: pr::DrawRecipe::Inline(Box::new(
                        move |dlb: &mut pr::DrawListBuilder| {
                            dlb.set_pipeline(overlay_pso);
                            dlb.set_bind_groups_with_offsets(
                                globals_bg,
                                Some(overlay_material_bg),
                                None,
                                None,
                                None,
                                None,
                            );
                            dlb.set_mesh(quad_mesh, 0, 0);
                            dlb.draw_indexed(6, 0);
                        },
                    )),
                },
            };
            renderer.master().add_pass(pass);
        } */

        Ok(())
    }
}

fn pill_rotation_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component_mut::<InputComponent>()?;

    // Exit on Escape
    if input_component.get_key_pressed(KeyboardKey::Escape) {
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
