use pill_engine::game::*;
use rand::{thread_rng, Rng};
use std::collections::VecDeque;

use crate::shared;

// --- Input ---
pub const SPAWN_PILL_BUTTON: KeyboardKey = KeyboardKey::Space;

// --- Colors ---
const GRAY_MATERIAL_TINT: (f32, f32, f32) = (0.3, 0.3, 0.3);
const ORANGE_MATERIAL_TINT: (f32, f32, f32) = (1.0, 0.5, 0.0);
const CLEAR_COLOR: (f32, f32, f32) = (0.08, 0.12, 0.18);

// --- Camera ---
const CAMERA_POSITION: (f32, f32, f32) = (24.0, 24.0, 24.0);
const CAMERA_FOV: f32 = 55.0;
const CAMERA_LOOK_AT: (f32, f32, f32) = (0.0, 0.0, 0.0);

// --- World ---
const PLANE_POSITION: (f32, f32, f32) = (0.0, -2.0, 0.0);
const PLANE_SCALE: (f32, f32, f32) = (2.0, 1.0, 2.0);

// --- Asset paths ---
const CUBE_MESH_PATH: &str = "models/cube.obj";
const PILL_MESH_PATH: &str = "models/pill.obj";
const PLANE_MESH_PATH: &str = "models/plane.obj";
const GRAY_MATERIAL_NAME: &str = "gray";
const ORANGE_MATERIAL_NAME: &str = "orange";
const CUBE_MESH_NAME: &str = "cube";
const PILL_MESH_NAME: &str = "pill";
const PLANE_MESH_NAME: &str = "plane";

pub struct Game {}

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // --- Basic setup ---
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<PbrRenderableComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<shared::CitizenComponent>(active_scene)?;

        // Add systems
        engine.add_system("pill_spawner", pill_spawner_system)?;
        engine.add_system("citizen_movement", citizen_movement_system)?;

        // --- Create resources ---
        let gray_material_handle = engine.add_resource::<Material>(
            Material::builder(GRAY_MATERIAL_NAME)
                .color_parameter(
                    "tint",
                    Color::new(
                        GRAY_MATERIAL_TINT.0,
                        GRAY_MATERIAL_TINT.1,
                        GRAY_MATERIAL_TINT.2,
                    ),
                )?
                .build(),
        )?;

        let orange_material_handle = engine.add_resource::<Material>(
            Material::builder(ORANGE_MATERIAL_NAME)
                .color_parameter(
                    "tint",
                    Color::new(
                        ORANGE_MATERIAL_TINT.0,
                        ORANGE_MATERIAL_TINT.1,
                        ORANGE_MATERIAL_TINT.2,
                    ),
                )?
                .build(),
        )?;

        // Mesh for the ground plane
        let plane_mesh_handle =
            engine.add_resource(Mesh::new(PLANE_MESH_NAME, PLANE_MESH_PATH.into()))?;

        // Mesh for the pill
        let pill_mesh_handle =
            engine.add_resource(Mesh::new(PILL_MESH_NAME, PILL_MESH_PATH.into()))?;

        // --- Create entities ---

        // Ground plane (cube scaled very flat, gray)
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(
                        PLANE_POSITION.0,
                        PLANE_POSITION.1,
                        PLANE_POSITION.2,
                    ))
                    .scale(Vector3f::new(PLANE_SCALE.0, PLANE_SCALE.1, PLANE_SCALE.2))
                    .build(),
            )
            .with_component(
                PbrRenderableComponent::builder()
                    .material(&gray_material_handle)
                    .mesh(&plane_mesh_handle)
                    .build(),
            )
            .build();

        // Initial pill at center, orange
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(
                        shared::ORIGIN.0,
                        shared::ORIGIN.1,
                        shared::ORIGIN.2,
                    ))
                    .build(),
            )
            .with_component(
                PbrRenderableComponent::builder()
                    .material(&orange_material_handle)
                    .mesh(&pill_mesh_handle)
                    .build(),
            )
            .build();

        // Camera at 45° diagonal perspective looking at origin
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(
                        CAMERA_POSITION.0,
                        CAMERA_POSITION.1,
                        CAMERA_POSITION.2,
                    ))
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(CAMERA_FOV)
                    .fog_density(0.0)
                    .fog_color(Color::new(0.0, 0.0, 0.0))
                    .clear_color(Color::new(CLEAR_COLOR.0, CLEAR_COLOR.1, CLEAR_COLOR.2))
                    .look_at(Some(Vector3f::new(
                        CAMERA_LOOK_AT.0,
                        CAMERA_LOOK_AT.1,
                        CAMERA_LOOK_AT.2,
                    )))
                    .build(),
            )
            .build();

        Ok(())
    }
}

// --- Systems ---

fn pill_spawner_system(engine: &mut Engine) -> Result<()> {
    let input = engine.get_global_component::<InputComponent>()?;

    if input.get_key_pressed(SPAWN_PILL_BUTTON) {
        let scene = engine.get_active_scene_handle()?;
        let orange_material = engine.get_resource_handle::<Material>(ORANGE_MATERIAL_NAME)?;
        let pill_mesh = engine.get_resource_handle::<Mesh>(PILL_MESH_NAME)?;
        let mut rng = thread_rng();

        engine
            .build_entity(scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(
                        shared::ORIGIN.0,
                        shared::ORIGIN.1,
                        shared::ORIGIN.2,
                    ))
                    .build(),
            )
            .with_component(
                PbrRenderableComponent::builder()
                    .material(&orange_material)
                    .mesh(&pill_mesh)
                    .build(),
            )
            .with_component(shared::CitizenComponent {
                path_points: VecDeque::new(),
                current_movement_speed: shared::CITIZEN_INITIAL_SPEED,
                max_movement_speed: rng
                    .gen_range(shared::CITIZEN_MAX_SPEED_MIN..shared::CITIZEN_MAX_SPEED_MAX),
                acceleration: rng
                    .gen_range(shared::CITIZEN_ACCELERATION_MIN..shared::CITIZEN_ACCELERATION_MAX),
                facing_angle: rng.gen_range(0.0..360.0),
                rotation_speed: rng.gen_range(
                    shared::CITIZEN_ROTATION_SPEED_MIN..shared::CITIZEN_ROTATION_SPEED_MAX,
                ),
            })
            .build();
    }

    Ok(())
}

fn citizen_movement_system(engine: &mut Engine) -> Result<()> {
    shared::move_citizens_toward_targets(engine, thread_rng())
}
