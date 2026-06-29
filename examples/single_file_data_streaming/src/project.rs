use pill_engine::{define_component, define_global_component, project::*};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

pub struct Project {}
create_project!(Project {}, PillProject);

const GRID_SIZE: usize = 100;
const CUBE_SPACING: f32 = 2.0;
const MAX_HEIGHT: f32 = 60.0;

/// Decode a greyscale PNG into a flat array of height values (0.0 – 1.0).
fn load_height_map(png_bytes: &[u8]) -> Result<Vec<f32>> {
    let mut decoder = png::Decoder::new(png_bytes);
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let raw = &buf[..info.buffer_size()];

    let pixel_count = info.width as usize * info.height as usize;
    let mut heights = Vec::with_capacity(pixel_count);

    match info.color_type {
        png::ColorType::Grayscale => {
            for &g in raw.iter().take(pixel_count) {
                heights.push(g as f32 / 255.0);
            }
        }
        png::ColorType::Rgb => {
            for px in raw.chunks(3).take(pixel_count) {
                let g = 0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32;
                heights.push(g / 255.0);
            }
        }
        png::ColorType::Rgba => {
            for px in raw.chunks(4).take(pixel_count) {
                let g = 0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32;
                heights.push(g / 255.0);
            }
        }
        _ => heights.resize(pixel_count, 0.0),
    }

    Ok(heights)
}

/// Sample the height map at grid position (x, z).
/// The height map is stretched to cover the full grid.
fn sample_height(heights: &[f32], map_w: u32, map_h: u32, grid_x: usize, grid_z: usize) -> f32 {
    let px = (grid_x as f32 / (GRID_SIZE - 1) as f32 * (map_w - 1) as f32) as usize;
    let py = (grid_z as f32 / (GRID_SIZE - 1) as f32 * (map_h - 1) as f32) as usize;
    let idx = py * map_w as usize + px;
    heights.get(idx).copied().unwrap_or(0.0)
}

define_component!(CameraMovementComponent {
    move_speed: f32,
    rotate_speed: f32,
});

define_component!(CubeData {
    grid_x: usize,
    grid_z: usize,
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CubeSceneDataInner {
    pub heights: Vec<f32>,
    pub loaded: bool,
}

define_global_component!(CubeSceneData {
    inner: Arc<Mutex<CubeSceneDataInner>>,
});

impl Default for CubeSceneData {
    fn default() -> Self {
        CubeSceneData {
            inner: Arc::new(Mutex::new(CubeSceneDataInner {
                heights: Vec::new(),
                loaded: false,
            })),
        }
    }
}

// --- Camera debug UI (PlinkoTuningComponent pattern) ---

#[derive(Debug, Clone, Copy)]
pub struct CameraDebugInfo {
    pub pos_x: f32,
    pub pos_y: f32,
    pub pos_z: f32,
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

impl Default for CameraDebugInfo {
    fn default() -> Self {
        CameraDebugInfo {
            pos_x: 0.0,
            pos_y: 0.0,
            pos_z: 0.0,
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
        }
    }
}

define_global_component!(CameraDebugComponent {
    info: Arc<Mutex<CameraDebugInfo>>,
});

impl Default for CameraDebugComponent {
    fn default() -> Self {
        CameraDebugComponent {
            info: Arc::new(Mutex::new(CameraDebugInfo::default())),
        }
    }
}

impl PillProject for Project {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraMovementComponent>(active_scene)?;
        engine.register_component::<CubeData>(active_scene)?;

        // --- Meshes ---
        let pill_mesh_handle = engine.add_resource(Mesh::new("pill", "models/pill.obj".into()))?;
        let cube_mesh_handle = engine.add_resource(Mesh::cube("cube", 1.0))?;

        // --- Materials ---
        let pill_material_handle = engine.add_resource(
            Material::builder("pill_mat")
                .color_parameter("tint", Color::new(0.8, 0.8, 0.82))?
                .build(),
        )?;
        let cube_material_handle = engine.add_resource(
            Material::builder("cube_mat")
                .color_parameter("tint", Color::new(1.0, 0.5, 0.0))?
                .build(),
        )?;

        // --- Camera ---
        let grid_center = (GRID_SIZE as f32 * CUBE_SPACING) / 2.0;
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(122.06, 160.00, 131.79))
                    .rotation(Vector3f::new(-211.12, 42.07, 0.0))
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(60.0)
                    .clear_color(Color::new(0.1, 0.1, 0.11))
                    .build(),
            )
            .with_component(CameraMovementComponent {
                move_speed: 80.0,
                rotate_speed: 120.0,
            })
            .build();

        // --- Floating pill (kept from original) ---
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(grid_center, 3.0, -5.0))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&pill_mesh_handle)
                    .material(&pill_material_handle)
                    .build(),
            )
            .build();

        // --- Load height map ---
        let height_png = include_bytes!("../res/textures/height.png");
        let (map_w, map_h) = {
            let decoder = png::Decoder::new(height_png.as_slice());
            let reader = decoder.read_info()?;
            (reader.info().width, reader.info().height)
        };
        let heights = load_height_map(height_png)?;

        // --- Build per-cube vertical values and write JSON ---
        let mut vertical_values: Vec<f32> = Vec::with_capacity(GRID_SIZE * GRID_SIZE);
        for x in 0..GRID_SIZE {
            for z in 0..GRID_SIZE {
                let h = sample_height(&heights, map_w, map_h, x, z);
                vertical_values.push(0.5 + h * MAX_HEIGHT);
            }
        }
        let json = serde_json::to_string(&vertical_values)?;
        std::fs::write("height_data.json", &json)?;

        // --- 100×100 cube grid (spawn at default Y, streaming will set real height) ---
        let half_extent = (GRID_SIZE as f32 * CUBE_SPACING) / 2.0;
        for x in 0..GRID_SIZE {
            for z in 0..GRID_SIZE {
                let pos_x = x as f32 * CUBE_SPACING - half_extent + CUBE_SPACING / 2.0;
                let pos_z = z as f32 * CUBE_SPACING - half_extent + CUBE_SPACING / 2.0;

                engine
                    .build_entity(active_scene)
                    .with_component(
                        TransformComponent::builder()
                            .position(Vector3f::new(pos_x, 0.5, pos_z))
                            .build(),
                    )
                    .with_component(
                        MeshRenderingComponent::builder()
                            .mesh(&cube_mesh_handle)
                            .material(&cube_material_handle)
                            .build(),
                    )
                    .with_component(CubeData {
                        grid_x: x,
                        grid_z: z,
                    })
                    .build();
            }
        }

        engine.add_system("camera_movement_system", camera_movement_system)?;
        engine.add_system(
            "update_camera_debug_ui_system",
            update_camera_debug_ui_system,
        )?;
        engine.add_system("position_streaming_system", position_streaming_system)?;

        engine.add_global_component(CameraDebugComponent::default())?;
        engine.add_global_component(CubeSceneData::default())?;
        register_camera_debug_ui(engine)?;
        register_stream_debug_ui(engine)?;

        Ok(())
    }
}

// --- Systems ---

fn camera_movement_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;

    // Read all input state before the mutable borrow below
    let input = engine.get_global_component::<InputComponent>()?;
    let key_w = input.get_key(KeyboardKey::KeyW);
    let key_s = input.get_key(KeyboardKey::KeyS);
    let key_d = input.get_key(KeyboardKey::KeyD);
    let key_a = input.get_key(KeyboardKey::KeyA);
    let key_q = input.get_key(KeyboardKey::KeyQ);
    let key_e = input.get_key(KeyboardKey::KeyE);
    let key_left = input.get_key(KeyboardKey::ArrowLeft);
    let key_right = input.get_key(KeyboardKey::ArrowRight);
    let key_up = input.get_key(KeyboardKey::ArrowUp);
    let key_down = input.get_key(KeyboardKey::ArrowDown);
    drop(input);

    for (_entity, transform, movement) in
        engine.iterate_two_components_mut::<TransformComponent, CameraMovementComponent>()?
    {
        // --- WASD movement (camera-local space) ---
        let forward = transform.get_forward_direction();
        let right = transform.get_right_direction();

        let speed = movement.move_speed * dt;
        let mut move_delta = Vector3f::ZERO;
        if key_w {
            move_delta -= forward;
        }
        if key_s {
            move_delta += forward;
        }
        if key_d {
            move_delta += right;
        }
        if key_a {
            move_delta -= right;
        }
        if key_q {
            move_delta -= Vector3f::Y;
        }
        if key_e {
            move_delta += Vector3f::Y;
        }

        if move_delta != Vector3f::ZERO {
            move_delta = move_delta.normalize() * speed;
            transform.translate_world(move_delta);
        }

        // --- Arrow-key rotation ---
        let rot_speed = movement.rotate_speed * dt;
        if key_left {
            transform.rotate_around_axis(rot_speed, Vector3f::Y);
        }
        if key_right {
            transform.rotate_around_axis(-rot_speed, Vector3f::Y);
        }
        if key_up {
            transform.rotate_around_axis(-rot_speed, Vector3f::X);
        }
        if key_down {
            transform.rotate_around_axis(rot_speed, Vector3f::X);
        }
    }

    Ok(())
}

// --- Position streaming system ---

fn position_streaming_system(engine: &mut Engine) -> Result<()> {
    let inner = engine
        .get_global_component::<CubeSceneData>()?
        .inner
        .clone();

    // Load JSON once
    {
        let mut data = inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !data.loaded {
            let json = std::fs::read_to_string("height_data.json")?;
            data.heights = serde_json::from_str(&json)?;
            data.loaded = true;
        }
    }

    // Apply heights to cubes
    let data = inner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let heights = &data.heights;

    for (_entity, transform, cube_data) in
        engine.iterate_two_components_mut::<TransformComponent, CubeData>()?
    {
        let idx = cube_data.grid_z * GRID_SIZE + cube_data.grid_x;
        if let Some(&h) = heights.get(idx) {
            let pos = transform.position;
            transform.set_position(Vector3f::new(pos.x, h, pos.z));
        }
    }

    Ok(())
}

// --- Stream debug UI ---

fn register_stream_debug_ui(engine: &mut Engine) -> Result<()> {
    let inner = engine
        .get_global_component::<CubeSceneData>()?
        .inner
        .clone();

    engine
        .get_global_component_mut::<EguiManagerComponent>()?
        .register_ui("stream.debug", move |ctx| {
            egui::Window::new("Stream Data")
                .default_open(true)
                .show(ctx, |ui| {
                    let data = inner
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());

                    let loaded = data.loaded;
                    let count = data.heights.len();
                    let mem_bytes = count * std::mem::size_of::<f32>();

                    ui.label(format!(
                        "Height Data Loaded: {}",
                        if loaded { "Yes" } else { "No" }
                    ));
                    ui.label(format!("Entries: {}", count));
                    ui.label(format!(
                        "Memory: {} ({:.2} KB)",
                        format_bytes(mem_bytes),
                        mem_bytes as f64 / 1024.0
                    ));
                });
        });

    Ok(())
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

// --- Camera debug UI systems ---

fn register_camera_debug_ui(engine: &mut Engine) -> Result<()> {
    let info = engine
        .get_global_component::<CameraDebugComponent>()?
        .info
        .clone();

    engine
        .get_global_component_mut::<EguiManagerComponent>()?
        .register_ui("camera.debug", move |ctx| {
            egui::Window::new("Camera Debug")
                .default_open(true)
                .show(ctx, |ui| {
                    let info = info.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

                    ui.label(format!(
                        "Position: ({:.2}, {:.2}, {:.2})",
                        info.pos_x, info.pos_y, info.pos_z
                    ));
                    ui.label(format!(
                        "Rotation: ({:.2}°, {:.2}°, {:.2}°)",
                        info.pitch, info.yaw, info.roll
                    ));
                });
        });

    Ok(())
}

fn update_camera_debug_ui_system(engine: &mut Engine) -> Result<()> {
    let info = engine
        .get_global_component::<CameraDebugComponent>()?
        .info
        .clone();

    for (_entity, transform, _camera) in
        engine.iterate_two_components_mut::<TransformComponent, CameraComponent>()?
    {
        let mut info = info.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        info.pos_x = transform.position.x;
        info.pos_y = transform.position.y;
        info.pos_z = transform.position.z;
        info.pitch = transform.rotation.x;
        info.yaw = transform.rotation.y;
        info.roll = transform.rotation.z;
    }

    Ok(())
}
