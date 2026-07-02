use pill_engine::project::*;
use rand::{thread_rng, Rng};
use std::collections::VecDeque;

use crate::shared;

// --- Input ---
pub const SPAWN_PILL_BUTTON: KeyboardKey = KeyboardKey::Space;

pub struct Project {}

impl PillProject for Project {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // --- Basic setup ---
        let active_scene = engine.create_scene("default")?;
        engine.set_active_scene(active_scene)?;

        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<shared::CitizenComponent>(active_scene)?;

        // Add systems
        engine.add_system("pill_spawner", pill_spawner_system)?;
        engine.add_system("citizen_movement", citizen_movement_system)?;

        // --- Create resources ---
        let gray_material_handle = engine.add_resource::<Material>(
            Material::builder(shared::GRAY_MATERIAL_NAME)
                .color_parameter("tint", shared::GRAY_MATERIAL_TINT)?
                .build(),
        )?;

        let orange_material_handle = engine.add_resource::<Material>(
            Material::builder(shared::ORANGE_MATERIAL_NAME)
                .color_parameter("tint", shared::ORANGE_MATERIAL_TINT)?
                .build(),
        )?;

        // Mesh for the ground plane
        let plane_mesh_handle = engine.add_resource(Mesh::new(
            shared::PLANE_MESH_NAME,
            shared::PLANE_MESH_PATH.into(),
        ))?;

        // Mesh for the pill
        let pill_mesh_handle = engine.add_resource(Mesh::new(
            shared::PILL_MESH_NAME,
            shared::PILL_MESH_PATH.into(),
        ))?;

        // --- Create entities ---

        // Ground plane (cube scaled very flat, gray)
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(shared::PLANE_POSITION)
                    .scale(shared::PLANE_SCALE)
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
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
                    .position(shared::WORLD_ORIGIN)
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
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
                    .position(shared::CAMERA_POSITION)
                    .rotation(shared::CAMERA_ROTATION)
                    .build(),
            )
            .with_component(
                CameraComponent::builder()
                    .enabled(true)
                    .fov(shared::CAMERA_FOV)
                    .fog_density(0.0)
                    .fog_color(Color::new(0.0, 0.0, 0.0))
                    .clear_color(shared::CLEAR_COLOR)
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
        let orange_material =
            engine.get_resource_handle::<Material>(shared::ORANGE_MATERIAL_NAME)?;
        let pill_mesh = engine.get_resource_handle::<Mesh>(shared::PILL_MESH_NAME)?;
        let mut rng = thread_rng();

        engine
            .build_entity(scene)
            .with_component(
                TransformComponent::builder()
                    .position(shared::WORLD_ORIGIN)
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
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
