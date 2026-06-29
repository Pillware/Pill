// ── Modules ──────────────────────────────────────────────────────────────

mod components;
mod constants;
mod systems;
mod ui;
mod utils;

// ── Re-exports ───────────────────────────────────────────────────────────

pub use components::{CameraDebugComponent, CameraDebugInfo, CubeSceneData};
pub use constants::*;
pub use systems::*;
pub use ui::*;
pub use utils::*;

// ── Crate root ───────────────────────────────────────────────────────────

use components::{CameraMovementComponent, CubeData};
use pill_engine::project::*;

pub struct Project {}
create_project!(Project {}, PillProject);

impl PillProject for Project {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        let scene = engine.create_scene("default")?;
        engine.set_active_scene(scene)?;

        engine.register_component::<TransformComponent>(scene)?;
        engine.register_component::<CameraComponent>(scene)?;
        engine.register_component::<MeshRenderingComponent>(scene)?;
        engine.register_component::<CameraMovementComponent>(scene)?;
        engine.register_component::<CubeData>(scene)?;

        // ── Resources ────────────────────────────────────────────────

        let pill_mesh = engine.add_resource(Mesh::new("pill", "models/pill.obj".into()))?;
        let cube_mesh = engine.add_resource(Mesh::cube("cube", 1.0))?;

        let pill_mat = engine.add_resource(
            Material::builder("pill_mat")
                .color_parameter("tint", Color::new(0.8, 0.8, 0.82))?
                .build(),
        )?;
        let cube_mat = engine.add_resource(
            Material::builder("cube_mat")
                .color_parameter("tint", Color::new(1.0, 0.5, 0.0))?
                .build(),
        )?;

        // ── Camera ───────────────────────────────────────────────────

        let grid_center = (GRID_SIZE as f32 * CUBE_SPACING) / 2.0;
        engine
            .build_entity(scene)
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

        // ── Reference pill ───────────────────────────────────────────

        engine
            .build_entity(scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(grid_center, 3.0, -5.0))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&pill_mesh)
                    .material(&pill_mat)
                    .build(),
            )
            .build();

        // ── Write fixed-width data file ──────────────────────────────

        let height_png = include_bytes!("../res/textures/height.png");
        let (map_w, map_h) = {
            let dec = png::Decoder::new(height_png.as_slice());
            let r = dec.read_info()?;
            (r.info().width, r.info().height)
        };
        let map = decode_height_map(height_png)?;

        let n = GRID_SIZE * GRID_SIZE;
        let mut data = vec![0u8; n * LINE_BYTES];
        let mut idx = 0;
        for z in 0..GRID_SIZE {
            for x in 0..GRID_SIZE {
                let h = sample_height(&map, map_w, map_h, x, z);
                let s = format!("{:>8.4}\n", 0.5 + h * MAX_HEIGHT);
                data[idx * LINE_BYTES..(idx + 1) * LINE_BYTES].copy_from_slice(s.as_bytes());
                idx += 1;
            }
        }
        std::fs::create_dir_all("res/data")?;
        std::fs::write(DATA_PATH, &data)?;

        // ── 100×100 cube grid ────────────────────────────────────────

        let half = (GRID_SIZE as f32 * CUBE_SPACING) / 2.0;
        for x in 0..GRID_SIZE {
            for z in 0..GRID_SIZE {
                let px = x as f32 * CUBE_SPACING - half + CUBE_SPACING / 2.0;
                let pz = z as f32 * CUBE_SPACING - half + CUBE_SPACING / 2.0;

                engine
                    .build_entity(scene)
                    .with_component(
                        TransformComponent::builder()
                            .position(Vector3f::new(px, 0.5, pz))
                            .scale(Vector3f::new(1.8, 1.8, 1.8))
                            .build(),
                    )
                    .with_component(
                        MeshRenderingComponent::builder()
                            .mesh(&cube_mesh)
                            .material(&cube_mat)
                            .build(),
                    )
                    .with_component(CubeData {
                        grid_x: x,
                        grid_z: z,
                        target_y: 0.5,
                    })
                    .build();
            }
        }

        // ── Systems & globals ────────────────────────────────────────

        engine.add_system("camera_movement", camera_movement_system)?;
        engine.add_system("update_camera_debug", update_camera_debug_system)?;
        engine.add_system("position_streaming", position_streaming_system)?;
        engine.add_system("cube_lerp", cube_lerp_system)?;

        engine.add_global_component(CameraDebugComponent::default())?;
        engine.add_global_component(CubeSceneData::default())?;
        register_camera_debug_ui(engine)?;
        register_stream_debug_ui(engine)?;

        Ok(())
    }
}
