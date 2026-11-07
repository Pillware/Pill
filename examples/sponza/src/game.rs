use pill_engine::{define_component, game::*};

use crate::loader::{self, SponzaCpu};

define_component!(OrbitCamera {
    yaw: f32,
    pitch: f32,
    radius: f32,
});

pub struct Game {}

// The background load (spawn_local task on WASM, thread on native) can't hold &mut Engine; it
// leaves its result here for `sponza_drain_system` to upload on the next frame.
type Loaded = std::result::Result<SponzaCpu, String>;
static PENDING: std::sync::Mutex<Option<Loaded>> = std::sync::Mutex::new(None);

// Orbit around a point at mid-height of the atrium so the camera circles inside the hall.
const ORBIT_CENTER_Y: f32 = 5.0;

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
            orbit.pitch = (orbit.pitch + mouse_delta.y * 0.3).clamp(-80.0, 80.0);
        }
        orbit.radius = (orbit.radius - scroll_delta.y * 0.8).clamp(2.0, 40.0);

        let pitch_radians = orbit.pitch.to_radians();
        let yaw_radians = orbit.yaw.to_radians();
        let radius_cos_pitch = orbit.radius * pitch_radians.cos();
        transform.set_position(Vector3f::new(
            radius_cos_pitch * yaw_radians.sin(),
            orbit.radius * pitch_radians.sin() + ORBIT_CENTER_Y,
            radius_cos_pitch * yaw_radians.cos(),
        ));
        transform.look_at(Vector3f::new(0.0, ORBIT_CENTER_Y, 0.0));
    }
    Ok(())
}

// Uploads the loaded payload once it arrives; a failed load is logged and the empty scene keeps running.
fn sponza_drain_system(engine: &mut Engine) -> Result<()> {
    let loaded = PENDING
        .lock()
        .map_err(|_| PillError::from("Sponza load lock poisoned"))?
        .take();
    match loaded {
        Some(Ok(cpu)) => {
            let scene = engine.get_active_scene_handle()?;
            spawn_assets(engine, scene, cpu)
        }
        Some(Err(error)) => {
            log::error!("Sponza load failed: {error}");
            Ok(())
        }
        None => Ok(()),
    }
}

// Uploads the CPU payload to the GPU and builds one entity per primitive.
fn spawn_assets(engine: &mut Engine, scene: SceneHandle, cpu: SponzaCpu) -> Result<()> {
    // --- IBL (background + diffuse/specular; the BRDF LUT is engine-owned) ---
    let equirect = &cpu.equirect;
    let background = engine.create_gpu_texture_f32(
        "sponza_equirect",
        &equirect.pixels,
        equirect.width,
        equirect.height,
    )?;
    let (diffuse, specular_mips) = &cpu.ibl;
    let (diffuse_width, diffuse_height) = ibl::DIFFUSE_SIZE;
    let diffuse_handle = engine.create_gpu_texture_f32(
        "sponza_diffuse_ibl",
        diffuse,
        diffuse_width,
        diffuse_height,
    )?;
    let (specular_width, specular_height) = ibl::SPECULAR_SIZE;
    let specular_handle = engine.create_gpu_mipped_texture_f32(
        "sponza_specular_ibl",
        specular_mips,
        specular_width,
        specular_height,
    )?;
    {
        let render_state = engine.get_global_component_mut::<RenderStateComponent>()?;
        render_state.background = Some(background);
        render_state.ibl_diffuse = diffuse_handle;
        render_state.ibl_specular = specular_handle;
    }

    let GltfSceneData {
        textures,
        materials,
        meshes,
    } = cpu.scene;

    // --- Textures (moved, so each is freed once uploaded) ---
    let mut texture_handles: Vec<TextureHandle> = Vec::with_capacity(textures.len());
    for texture in textures {
        texture_handles.push(engine.add_resource(Texture::from_bytes_owned(
            &texture.name,
            texture.kind,
            texture.rtex,
        ))?);
    }

    // --- Materials ---
    let mut material_handles: Vec<MaterialHandle> = Vec::with_capacity(materials.len());
    for material in &materials {
        let [red, green, blue] = material.emissive_color;
        let mut pbr = Material::new(&material.name)
            .albedo(Color::new(
                material.base_color[0],
                material.base_color[1],
                material.base_color[2],
            ))
            .metallic(material.metallic)
            .roughness(material.roughness)
            .emissive(Color::new(red, green, blue));
        if let Some(i) = material.albedo {
            pbr = pbr.albedo_texture(texture_handles[i]);
        }
        if let Some(i) = material.normal {
            pbr = pbr.normal_texture(texture_handles[i]);
        }
        if let Some(i) = material.metallic_roughness {
            pbr = pbr.metallic_roughness_texture(texture_handles[i]);
        }
        if let Some(i) = material.emissive {
            pbr = pbr.emissive_texture(texture_handles[i]);
        }
        material_handles.push(engine.add_resource(pbr)?);
    }

    // --- Meshes + entities (world transform already baked into the vertices) ---
    for mesh in &meshes {
        let mesh_handle =
            engine.add_resource(Mesh::from_cooked_mesh_bytes(&mesh.name, &mesh.rmsh)?)?;
        engine
            .build_entity(scene)
            .with_component(TransformComponent::builder().build())
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&mesh_handle)
                    .material(&material_handles[mesh.material])
                    .build(),
            )
            .build();
    }

    log::info!("Sponza: uploaded {} meshes to GPU", meshes.len());
    Ok(())
}

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        let scene = engine.create_scene("sponza")?;
        engine.set_active_scene(scene)?;
        engine
            .get_global_component_mut::<RenderStateComponent>()?
            .background_color = [0.02, 0.02, 0.03];

        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<CameraComponent>(scene)?;
        engine.register_component::<MeshRenderingComponent>(scene)?;
        engine.register_component::<OrbitCamera>(scene)?;

        // Build the camera up front so the window renders (clear color + sky once IBL lands)
        // while the ~50 MB model downloads.
        engine
            .build_entity(scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(-9.0, ORBIT_CENTER_Y, 0.0))
                    .build(),
            )
            .with_component(CameraComponent::builder().enabled(true).fov(60.0).build())
            .with_component(OrbitCamera {
                yaw: 90.0,
                pitch: 5.0,
                radius: 9.0,
            })
            .build();

        engine.add_system("orbit_camera", orbit_camera_system)?;

        // Load in the background; frames render meanwhile and the drain system uploads the result
        let store = |result| {
            if let Ok(mut pending) = PENDING.lock() {
                *pending = Some(result);
            }
        };
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_futures::spawn_local(async move { store(loader::load().await) });
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::spawn(move || store(pollster::block_on(loader::load())));
        engine.add_system("sponza_drain", sponza_drain_system)?;

        Ok(())
    }
}
