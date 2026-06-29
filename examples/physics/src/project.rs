use pill_engine::project::*;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

pub struct Project {}
create_project!(Project {}, PillProject);

// Define custom component
pub struct PillComponent {}

impl Component for PillComponent {}

impl PillTypeMapKey for PillComponent {
    type Storage = ComponentStorage<Self>;
}

define_global_component!(PlinkoBoard {
    ball_mesh: MeshHandle,
    ball_material: MaterialHandle,
    backplate_mesh: MeshHandle,
    backplate_material: MaterialHandle,
    peg_mesh: MeshHandle,
    peg_material: MaterialHandle,
    compartment_material: MaterialHandle,
    triangle_mesh: MeshHandle,
    triangle_material: MaterialHandle,
});

define_component!(BallComponent { index: u64 });

// set threshold of spawned objects and remove the oldest N if we go beyond the limit, allow for
// tweaking that in UI
define_global_component!(StateComponent {
    elapsed: f32,
    timeout: f32,
    spawn_index: u64,
    ball_radius: f32,
});

define_component!(CameraMovementComponent {
    orbit_speed: f32,
    zoom_speed: f32,
    angle: f32,
    radius: f32,
    delta_y: f32,
    delta_z: f32,
});

pub enum PlinkoUiCommand {
    SpawnBall,
    ClearBalls,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlinkoSettings {
    pub ball_radius: f32,
    pub gravity_y_mps2: f32,
    pub spawn_interval: f32,
    pub palette_index: u8, // which colour palette to use
}

const METERS_PER_WORLD_UNIT: f32 = 0.20;

impl Default for PlinkoSettings {
    fn default() -> Self {
        PlinkoSettings {
            ball_radius: 0.25,
            gravity_y_mps2: -9.81,
            spawn_interval: 1.0,
            palette_index: 0,
        }
    }
}

pub struct PlinkoUiState {
    pub command_queue: VecDeque<PlinkoUiCommand>,
    pub commited: PlinkoSettings,
    pub draft: PlinkoSettings,
}

impl Default for PlinkoUiState {
    fn default() -> Self {
        let settings = PlinkoSettings::default();

        PlinkoUiState {
            command_queue: VecDeque::new(),
            commited: settings,
            draft: settings,
        }
    }
}

define_global_component!(PlinkoTuningComponent {
    state: Arc<Mutex<PlinkoUiState>>,
    applied: PlinkoSettings,
});

impl Default for PlinkoTuningComponent {
    fn default() -> Self {
        PlinkoTuningComponent {
            state: Arc::new(Mutex::new(PlinkoUiState::default())),
            applied: PlinkoSettings::default(),
        }
    }
}

impl PillProject for Project {
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
        engine.register_component::<RigidBodyComponent>(active_scene)?;
        engine.register_component::<ColliderComponent>(active_scene)?;
        engine.register_component::<BallComponent>(active_scene)?;
        engine.register_component::<CameraMovementComponent>(active_scene)?;

        // Add systems
        engine.add_system("update_plinko_ui_system", update_plinko_ui_system)?;
        engine.add_system("ball_spawning_system", ball_spawning_system)?;
        engine.add_system("camera_movement_system", camera_movement_system)?;
        engine.add_system("demo_keyboard_control_system", demo_keyboard_control_system)?;
        engine.add_global_component(StateComponent {
            elapsed: 0.0,
            timeout: 1.0,
            spawn_index: 0,
            ball_radius: 0.25,
        })?;

        // TODO: later add variable materials and colours
        // Ball mesh and material
        let ball_mesh_handle = engine.add_resource::<Mesh>(Mesh::from_cooked_mesh_bytes(
            "Ball",
            include_bytes!("../res/models/ball.cooked_mesh"),
        )?)?;

        let ball_material_handle = engine.add_resource::<Material>(
            Material::builder("Ball")
                .color_parameter("tint", Color::new(0.97, 0.72, 0.09))?
                .scalar_parameter("specularity", 0.5)?
                .build(),
        )?;

        // Backplate mesh and material
        let backplate_mesh_handle = engine.add_resource(Mesh::from_cooked_mesh_bytes(
            "Backplate",
            include_bytes!("../res/models/plane.cooked_mesh"),
        )?)?;

        let backplate_material_handle = engine.add_resource::<Material>(
            Material::builder("Backplate")
                .color_parameter("tint", Color::new(1.0, 0.0, 0.0))?
                .scalar_parameter("specularity", 0.5)?
                .build(),
        )?;

        // Compartment material
        let compartment_material_handle = engine.add_resource::<Material>(
            Material::builder("Compratment")
                .color_parameter("tint", Color::new(0.0, 1.0, 0.0))?
                .scalar_parameter("specularity", 0.5)?
                .build(),
        )?;

        // Pegs meshes and materials
        let peg_mesh_handle = engine.add_resource(Mesh::from_cooked_mesh_bytes(
            "Peg",
            include_bytes!("../res/models/cylinder.cooked_mesh"),
        )?)?;

        let peg_material_handle = engine.add_resource::<Material>(
            Material::builder("Peg")
                .color_parameter("tint", Color::new(0.0, 0.0, 1.0))?
                .scalar_parameter("specularity", 0.5)?
                .build(),
        )?;

        // Side triangles meshes and materials
        let triangle_mesh_handle = engine.add_resource(Mesh::from_cooked_mesh_bytes(
            "Triangle",
            include_bytes!("../res/models/triangle.cooked_mesh"),
        )?)?;

        let triangle_material_handle = engine.add_resource::<Material>(
            Material::builder("Triangle")
                .color_parameter("tint", Color::new(0.5, 0.5, 0.5))?
                .scalar_parameter("specularity", 0.5)?
                .build(),
        )?;

        // Create camera entity
        engine
            .build_entity(active_scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(0.0, 2.0, -30.0))
                    .rotation(Vector3f::new(0.0, 0.0, 0.0))
                    .build(),
            )
            .with_component(CameraMovementComponent {
                orbit_speed: 60.0,
                zoom_speed: 5.0,
                angle: -90.0,
                radius: 30.0,
                delta_y: 2.0,
                delta_z: 0.0,
            })
            .with_component(CameraComponent::builder().enabled(true).build())
            .build();

        // Store handles for spawning in the plinko global component
        let plinko = PlinkoBoard {
            ball_material: ball_material_handle,
            ball_mesh: ball_mesh_handle,
            backplate_material: backplate_material_handle,
            backplate_mesh: backplate_mesh_handle,
            peg_material: peg_material_handle,
            peg_mesh: peg_mesh_handle,
            compartment_material: compartment_material_handle,
            triangle_mesh: triangle_mesh_handle,
            triangle_material: triangle_material_handle,
        };
        spawn_plinko_board(engine, active_scene, &plinko)?;

        engine.add_global_component(plinko)?;

        let physics_world_component = engine.get_global_component_mut::<PhysicsWorldComponent>()?;
        physics_world_component.set_gravity(Vector3f::new(0.0, -9.81 / METERS_PER_WORLD_UNIT, 0.0));

        // Setup egui components
        engine.add_global_component(PlinkoTuningComponent::default())?;
        register_plinko_ui(engine)?;

        Ok(())
    }
}

fn ball_spawning_system(engine: &mut Engine) -> Result<()> {
    // if a timeout has passed, spawn the ball (every 1s for now)
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
    let state = engine.get_global_component_mut::<StateComponent>()?;

    if state.elapsed + dt > state.timeout {
        state.elapsed = 0.0;

        spawn_one_ball(engine)?;
    } else {
        state.elapsed += dt;
    }
    Ok(())
}

fn spawn_plinko_board(engine: &mut Engine, scene: SceneHandle, plinko: &PlinkoBoard) -> Result<()> {
    // Board lies in the X/Y plane.
    // Camera looks from -Z toward +Z.
    // Plane mesh is X/Z in model space, so rotate +90° around X.
    // Cylinder mesh is Y-axis aligned, so rotate +90° around X.

    const BOARD_Z: f32 = 1.0;
    const BOARD_HALF_WIDTH: f32 = 10.0;
    const BOARD_HALF_HEIGHT: f32 = 14.0;
    const BOARD_CENTER_Y: f32 = 4.0;

    const WALL_THICKNESS: f32 = 0.35;

    const PEG_RADIUS: f32 = 0.38;
    const PEG_HALF_DEPTH: f32 = 0.55;

    const PEG_ROW_COUNT: usize = 6;
    const PEG_ROW_TOP_Y: f32 = 14.0;
    const PEG_ROW_STEP_Y: f32 = 3.0;

    const BIN_COUNT: usize = 9;
    const DIVIDER_HALF_WIDTH: f32 = 0.18;
    const DIVIDER_HALF_HEIGHT: f32 = 1.8;

    const SIDE_TRIANGLE_COUNT: usize = PEG_ROW_COUNT - 1;
    const SIDE_TRIANGLE_GAP_CENTER_Y: f32 = PEG_ROW_TOP_Y - PEG_ROW_STEP_Y * 0.5;
    const SIDE_TRIANGLE_STEP_Y: f32 = PEG_ROW_STEP_Y;

    // Keep the wooden-guide look, but pull the point back enough that
    // it cannot create a narrow route beside the outer peg.
    const SIDE_TRIANGLE_INSET_X: f32 = 0.72;
    const SIDE_TRIANGLE_HALF_HEIGHT: f32 = 1.00;
    const SIDE_TRIANGLE_HALF_DEPTH: f32 = 0.34;

    // The collider is a trapezoid very close to the visual triangle.
    // Flat/rounded nose avoids a degenerate sharp-point contact.
    const SIDE_TRIANGLE_TIP_HALF_HEIGHT: f32 = 0.12;
    const SIDE_TRIANGLE_CORNER_RADIUS: f32 = 0.06;
    const SIDE_TRIANGLE_WALL_OVERLAP_X: f32 = 0.12;

    let board_rotation = Vector3f::new(90.0, 0.0, 0.0);
    let bottom_y = BOARD_CENTER_Y - BOARD_HALF_HEIGHT;

    // ---------------------------------------------------------------------
    // Backplate
    // ---------------------------------------------------------------------
    engine
        .build_entity(scene)
        .with_component(
            TransformComponent::builder()
                .position(Vector3f::new(0.0, BOARD_CENTER_Y, BOARD_Z))
                .rotation(board_rotation)
                .scale(Vector3f::new(BOARD_HALF_WIDTH, 1.0, BOARD_HALF_HEIGHT))
                .build(),
        )
        .with_component(
            MeshRenderingComponent::builder()
                .mesh(&plinko.backplate_mesh)
                .material(&plinko.backplate_material)
                .build(),
        )
        .with_component(
            RigidBodyComponent::builder()
                .body_type(RigidBodyType::Fixed)
                .build(),
        )
        .with_component(
            ColliderComponent::builder()
                .shape(SharedShape::cuboid(
                    BOARD_HALF_WIDTH,
                    0.25,
                    BOARD_HALF_HEIGHT,
                ))
                .friction(0.20)
                .restitution(0.05)
                .build(),
        )
        .build();

    // ---------------------------------------------------------------------
    // Left wall
    // ---------------------------------------------------------------------
    engine
        .build_entity(scene)
        .with_component(
            TransformComponent::builder()
                .position(Vector3f::new(-BOARD_HALF_WIDTH, BOARD_CENTER_Y, 0.0))
                .rotation(board_rotation)
                .scale(Vector3f::new(WALL_THICKNESS, 1.0, BOARD_HALF_HEIGHT))
                .build(),
        )
        .with_component(
            MeshRenderingComponent::builder()
                .mesh(&plinko.backplate_mesh)
                .material(&plinko.compartment_material)
                .build(),
        )
        .with_component(
            RigidBodyComponent::builder()
                .body_type(RigidBodyType::Fixed)
                .build(),
        )
        .with_component(
            ColliderComponent::builder()
                .shape(SharedShape::cuboid(WALL_THICKNESS, 0.35, BOARD_HALF_HEIGHT))
                .friction(0.0)
                .restitution(0.08)
                .build(),
        )
        .build();

    // ---------------------------------------------------------------------
    // Right wall
    // ---------------------------------------------------------------------
    engine
        .build_entity(scene)
        .with_component(
            TransformComponent::builder()
                .position(Vector3f::new(BOARD_HALF_WIDTH, BOARD_CENTER_Y, 0.0))
                .rotation(board_rotation)
                .scale(Vector3f::new(WALL_THICKNESS, 1.0, BOARD_HALF_HEIGHT))
                .build(),
        )
        .with_component(
            MeshRenderingComponent::builder()
                .mesh(&plinko.backplate_mesh)
                .material(&plinko.compartment_material)
                .build(),
        )
        .with_component(
            RigidBodyComponent::builder()
                .body_type(RigidBodyType::Fixed)
                .build(),
        )
        .with_component(
            ColliderComponent::builder()
                .shape(SharedShape::cuboid(WALL_THICKNESS, 0.35, BOARD_HALF_HEIGHT))
                .friction(0.0)
                .restitution(0.08)
                .build(),
        )
        .build();

    // ---------------------------------------------------------------------
    // Bottom wall
    // ---------------------------------------------------------------------
    engine
        .build_entity(scene)
        .with_component(
            TransformComponent::builder()
                .position(Vector3f::new(0.0, bottom_y, 0.0))
                .rotation(board_rotation)
                .scale(Vector3f::new(BOARD_HALF_WIDTH, 1.0, WALL_THICKNESS))
                .build(),
        )
        .with_component(
            MeshRenderingComponent::builder()
                .mesh(&plinko.backplate_mesh)
                .material(&plinko.compartment_material)
                .build(),
        )
        .with_component(
            RigidBodyComponent::builder()
                .body_type(RigidBodyType::Fixed)
                .build(),
        )
        .with_component(
            ColliderComponent::builder()
                .shape(SharedShape::cuboid(BOARD_HALF_WIDTH, 0.35, WALL_THICKNESS))
                .friction(0.35)
                .restitution(0.10)
                .build(),
        )
        .build();

    // ---------------------------------------------------------------------
    // Side triangle barriers
    // ---------------------------------------------------------------------
    let left_wall_inner_x = -BOARD_HALF_WIDTH + WALL_THICKNESS;
    let right_wall_inner_x = BOARD_HALF_WIDTH - WALL_THICKNESS;

    for i in 0..SIDE_TRIANGLE_COUNT {
        let center_y = SIDE_TRIANGLE_GAP_CENTER_Y - i as f32 * SIDE_TRIANGLE_STEP_Y;

        // Your existing mesh helper wants this slightly lower anchor.
        let visual_anchor_y = center_y - SIDE_TRIANGLE_HALF_HEIGHT * 0.5;

        // Left visual tooth.
        let left_visual_center = Vector3f::new(
            left_wall_inner_x + SIDE_TRIANGLE_INSET_X * 0.5,
            visual_anchor_y,
            -0.03,
        );

        spawn_side_triangles_visual(
            engine,
            scene,
            plinko,
            left_visual_center,
            true,
            SIDE_TRIANGLE_INSET_X,
            SIDE_TRIANGLE_HALF_HEIGHT,
            SIDE_TRIANGLE_HALF_DEPTH,
        )?;

        // Left physical tooth: one full convex solid.
        spawn_side_tooth_collider(
            engine,
            scene,
            Vector3f::new(
                left_wall_inner_x - SIDE_TRIANGLE_WALL_OVERLAP_X + SIDE_TRIANGLE_INSET_X * 0.5,
                center_y,
                0.0,
            ),
            true,
            SIDE_TRIANGLE_INSET_X,
            SIDE_TRIANGLE_HALF_HEIGHT,
            SIDE_TRIANGLE_HALF_DEPTH,
            SIDE_TRIANGLE_TIP_HALF_HEIGHT,
            SIDE_TRIANGLE_CORNER_RADIUS,
        )?;

        // Right visual tooth.
        let right_visual_center = Vector3f::new(
            right_wall_inner_x - SIDE_TRIANGLE_INSET_X * 0.5,
            visual_anchor_y,
            -0.03,
        );

        spawn_side_triangles_visual(
            engine,
            scene,
            plinko,
            right_visual_center,
            false,
            SIDE_TRIANGLE_INSET_X,
            SIDE_TRIANGLE_HALF_HEIGHT,
            SIDE_TRIANGLE_HALF_DEPTH,
        )?;

        // Right physical tooth: mirrored, one full convex solid.
        spawn_side_tooth_collider(
            engine,
            scene,
            Vector3f::new(
                right_wall_inner_x + SIDE_TRIANGLE_WALL_OVERLAP_X - SIDE_TRIANGLE_INSET_X * 0.5,
                center_y,
                0.0,
            ),
            false,
            SIDE_TRIANGLE_INSET_X,
            SIDE_TRIANGLE_HALF_HEIGHT,
            SIDE_TRIANGLE_HALF_DEPTH,
            SIDE_TRIANGLE_TIP_HALF_HEIGHT,
            SIDE_TRIANGLE_CORNER_RADIUS,
        )?;
    }
    // ---------------------------------------------------------------------
    // Peg field
    // ---------------------------------------------------------------------
    for row in 0..PEG_ROW_COUNT {
        let y = PEG_ROW_TOP_Y - row as f32 * PEG_ROW_STEP_Y;
        let columns = if row % 2 == 0 { 5 } else { 6 };
        let x_start = if row % 2 == 0 { -6.0 } else { -7.5 };

        for column in 0..columns {
            let x = x_start + column as f32 * 3.0;

            engine
                .build_entity(scene)
                .with_component(
                    TransformComponent::builder()
                        .position(Vector3f::new(x, y, 0.0))
                        .rotation(board_rotation)
                        .scale(Vector3f::new(PEG_RADIUS, PEG_HALF_DEPTH, PEG_RADIUS))
                        .build(),
                )
                .with_component(
                    MeshRenderingComponent::builder()
                        .mesh(&plinko.peg_mesh)
                        .material(&plinko.peg_material)
                        .build(),
                )
                .with_component(
                    RigidBodyComponent::builder()
                        .body_type(RigidBodyType::Fixed)
                        .build(),
                )
                .with_component(
                    ColliderComponent::builder()
                        .shape(SharedShape::cylinder(PEG_HALF_DEPTH, PEG_RADIUS))
                        .friction(0.20)
                        .restitution(0.35)
                        .build(),
                )
                .build();
        }
    }

    // ---------------------------------------------------------------------
    // Bottom compartments
    // ---------------------------------------------------------------------
    let inner_left = -BOARD_HALF_WIDTH + WALL_THICKNESS * 1.5;
    let inner_right = BOARD_HALF_WIDTH - WALL_THICKNESS * 1.5;
    let inner_width = inner_right - inner_left;
    let bin_width = inner_width / BIN_COUNT as f32;

    for i in 1..BIN_COUNT {
        let x = inner_left + i as f32 * bin_width;
        let divider_y = bottom_y + DIVIDER_HALF_HEIGHT + WALL_THICKNESS * 0.75;

        engine
            .build_entity(scene)
            .with_component(
                TransformComponent::builder()
                    .position(Vector3f::new(x, divider_y, 0.0))
                    .rotation(board_rotation)
                    .scale(Vector3f::new(DIVIDER_HALF_WIDTH, 1.0, DIVIDER_HALF_HEIGHT))
                    .build(),
            )
            .with_component(
                MeshRenderingComponent::builder()
                    .mesh(&plinko.backplate_mesh)
                    .material(&plinko.compartment_material)
                    .build(),
            )
            .with_component(
                RigidBodyComponent::builder()
                    .body_type(RigidBodyType::Fixed)
                    .build(),
            )
            .with_component(
                ColliderComponent::builder()
                    .shape(SharedShape::cuboid(
                        DIVIDER_HALF_WIDTH,
                        0.35,
                        DIVIDER_HALF_HEIGHT,
                    ))
                    .friction(0.30)
                    .restitution(0.08)
                    .build(),
            )
            .build();
    }

    Ok(())
}

fn spawn_side_triangles_visual(
    engine: &mut Engine,
    scene: SceneHandle,
    plinko: &PlinkoBoard,
    center: Vector3f,
    is_left: bool,
    inset_x: f32,
    height: f32,
    half_depth: f32,
) -> Result<()> {
    // Your Triangle.obj has a right-triangle footprint in local X/Z.
    //
    // With rotation +90° around X:
    // - local X maps to board X
    // - local Z maps to board Y
    // - local Y maps to world Z/depth
    //
    // Left side needs horizontal mirroring, hence negative X scale.
    let x_scale = if is_left {
        -inset_x * 0.5
    } else {
        inset_x * 0.5
    };

    let z_scale = height * 0.5;

    // Draw twice with local-Y flipped to avoid front/back normal/culling issues.
    // This preserves the same X/Y footprint, because local Y is only depth.
    let depth_scales = [half_depth, -half_depth];

    // draw them twice so we get the big wedge shape
    for i in (0..2).rev() {
        let new_center = Vector3f::new(center.x, center.y + height * i as f32, center.z);
        for y_depth_scale in depth_scales {
            engine
                .build_entity(scene)
                .with_component(
                    TransformComponent::builder()
                        .position(new_center)
                        .rotation(Vector3f::new(90.0 + 180.0 * i as f32, 0.0, 0.0))
                        .scale(Vector3f::new(x_scale, y_depth_scale, z_scale))
                        .build(),
                )
                .with_component(
                    MeshRenderingComponent::builder()
                        .mesh(&plinko.triangle_mesh)
                        .material(&plinko.triangle_material)
                        .build(),
                )
                .build();
        }
    }
    Ok(())
}

fn spawn_side_tooth_collider(
    engine: &mut Engine,
    scene: SceneHandle,
    center: Vector3f,
    is_left: bool,
    inset_x: f32,
    half_height: f32,
    half_depth: f32,
    tip_half_height: f32,
    corner_radius: f32,
) -> Result<()> {
    let half_inset = inset_x * 0.5;

    // Local X/Y is already board X/Y, so this entity deliberately has
    // no +90° rotation. Its prism is authored directly in board space.
    let (base_x, tip_x) = if is_left {
        (-half_inset, half_inset)
    } else {
        (half_inset, -half_inset)
    };

    // A rounded trapezoidal prism:
    //
    // Left:                 Right:
    //
    // | \                   / |
    // |  | <- flat nose     |  |
    // | /                   \ |
    //
    // This is one convex collider, so there is no seam or isolated cap
    // in which a ball can be caught.
    let points = [
        Vector3f::new(base_x, -half_height, -half_depth),
        Vector3f::new(base_x, half_height, -half_depth),
        Vector3f::new(tip_x, -tip_half_height, -half_depth),
        Vector3f::new(tip_x, tip_half_height, -half_depth),
        Vector3f::new(base_x, -half_height, half_depth),
        Vector3f::new(base_x, half_height, half_depth),
        Vector3f::new(tip_x, -tip_half_height, half_depth),
        Vector3f::new(tip_x, tip_half_height, half_depth),
    ];

    let shape = SharedShape::round_convex_hull(&points, corner_radius)
        .expect("side tooth points must form a valid convex prism");

    engine
        .build_entity(scene)
        .with_component(TransformComponent::builder().position(center).build())
        .with_component(
            RigidBodyComponent::builder()
                .body_type(RigidBodyType::Fixed)
                .build(),
        )
        .with_component(
            ColliderComponent::builder()
                .shape(shape)
                .friction(0.05)
                .restitution(0.03)
                .build(),
        )
        .build();

    Ok(())
}

fn spawn_one_ball(engine: &mut Engine) -> Result<()> {
    let scene = engine.get_active_scene_handle()?;
    let ball_mesh = engine.get_global_component::<PlinkoBoard>()?.ball_mesh;
    let ball_material = engine.get_global_component::<PlinkoBoard>()?.ball_material;
    let state = engine.get_global_component_mut::<StateComponent>()?;
    let ball_radius = state.ball_radius;

    let spawn_index = state.spawn_index;
    state.spawn_index = state.spawn_index.wrapping_add(1);
    spawn_ball(
        engine,
        scene,
        &ball_mesh,
        &ball_material,
        spawn_index,
        ball_radius,
    )?;

    Ok(())
}

// Spawn a ball in the same place falling down, called periodically
fn spawn_ball(
    engine: &mut Engine,
    scene: SceneHandle,
    mesh: &MeshHandle,
    material: &MaterialHandle,
    spawn_index: u64,
    ball_radius: f32,
) -> Result<()> {
    const SPAWN_XS: [f32; 8] = [-0.9, 0.75, -0.35, 1.15, -1.25, 0.45, -0.65, 0.95];
    let spawn_x = SPAWN_XS[spawn_index as usize % SPAWN_XS.len()];

    engine
        .build_entity(scene)
        .with_component(
            TransformComponent::builder()
                .position(Vector3f::new(spawn_x, 18.0, 0.0))
                .scale(Vector3f::new(ball_radius, ball_radius, ball_radius))
                .build(),
        )
        .with_component(
            MeshRenderingComponent::builder()
                .mesh(mesh)
                .material(material)
                .build(),
        )
        .with_component(BallComponent { index: spawn_index })
        .with_component(
            RigidBodyComponent::builder()
                .body_type(RigidBodyType::Dynamic)
                .locked_axes(LockedAxes::TRANSLATION_LOCKED_Z)
                .ccd_enabled(false)
                .can_sleep(true)
                .build(),
        )
        .with_component(
            ColliderComponent::builder()
                .shape(SharedShape::ball(ball_radius * 2.0 + 0.05))
                .mass(0.1)
                .friction(0.2)
                .restitution(0.25)
                .build(),
        )
        .build();

    Ok(())
}

fn camera_movement_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component_mut::<InputComponent>()?;

    // Get input
    let a_key = input_component.get_key(KeyboardKey::KeyA);
    let d_key = input_component.get_key(KeyboardKey::KeyD);
    let right_mouse_button = input_component.get_mouse_button(MouseButton::Right);
    let mouse_scroll_delta = input_component.get_mouse_scroll_delta();
    let mouse_delta = input_component.get_mouse_delta();

    // Get gamepad input
    let gamepad_left_stick =
        input_component.get_gamepad_axis(PlayerId::Player1, GamepadAxis::LeftStickX);

    // Pressing left bumper causes rumble (Example of haptics usage)
    let left_bumper =
        input_component.get_gamepad_button(PlayerId::Player1, GamepadButton::LeftBumper);
    if left_bumper {
        input_component.enqueue_rumble(PlayerId::Player1, 1.0, 1.0, 500);
    }

    for (_, transform_transform, camera_movement_component) in
        engine.iterate_two_components_mut::<TransformComponent, CameraMovementComponent>()?
    {
        // Zoom
        let zoom_speed = camera_movement_component.zoom_speed;
        camera_movement_component.radius -= mouse_scroll_delta.y * zoom_speed;

        // Orbit
        let mut change_value: f32 = 0.0;
        // TODO: make it progressive for gamepad
        if d_key {
            change_value -= 1.0;
        } else if gamepad_left_stick < -0.1 {
            change_value += 1.0;
        }
        if a_key {
            change_value += 1.0;
        } else if gamepad_left_stick > 0.1 {
            change_value -= 1.0;
        }
        let orbit_speed = camera_movement_component.orbit_speed;
        camera_movement_component.angle += change_value * orbit_speed * delta_time;
        let angle = camera_movement_component.angle;
        let radius = camera_movement_component.radius;

        let x_position = angle.to_radians().cos() * radius;
        let z_position = angle.to_radians().sin() * radius;

        // Mouse movement
        let mut z_change_value = 0.0;
        if mouse_delta.x > 0.0 {
            z_change_value -= 0.2;
        }
        if mouse_delta.x < 0.0 {
            z_change_value += 0.2;
        }

        let mut y_change_value = 0.0;
        if mouse_delta.y > 0.0 {
            y_change_value -= 0.2;
        }
        if mouse_delta.y < 0.0 {
            y_change_value += 0.2;
        }

        if right_mouse_button {
            camera_movement_component.delta_z += z_change_value;
            camera_movement_component.delta_y += y_change_value;
        }

        let delta_y = camera_movement_component.delta_y;
        let delta_z = camera_movement_component.delta_z;

        // Set position
        transform_transform.set_position(Vector3f::new(x_position, delta_y, z_position + delta_z));

        // Set rotation
        transform_transform.set_rotation(Vector3f::new(0.0, -angle - 90.0, 0.0));
    }

    Ok(())
}

// TODO: this currently does not remove all physic entities, the colliders and RBs remain
// hence we get ghost balls
//
// TODO: fix after new ECS merge, should be fixed by then
fn clear_all_balls(engine: &mut Engine) -> Result<()> {
    let mut entities: Vec<EntityHandle> = vec![];
    for (entity, _) in engine.iterate_one_component_mut::<BallComponent>()? {
        entities.push(entity);
    } // TODO: could just bulk remove all entities with BallComponent
    for entity in entities {
        engine.remove_entity_default_scene(entity)?;
    }
    Ok(())
}

fn demo_keyboard_control_system(engine: &mut Engine) -> Result<()> {
    let input = engine.get_global_component::<InputComponent>()?;

    let reset_pressed = input.get_key(KeyboardKey::KeyR);
    let spawn_pressed = input.get_key_pressed(KeyboardKey::Space);

    if !reset_pressed && !spawn_pressed {
        return Ok(());
    }

    let state = engine
        .get_global_component::<PlinkoTuningComponent>()?
        .state
        .clone();
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // remove all balls
    if reset_pressed {
        state.command_queue.push_back(PlinkoUiCommand::ClearBalls);
    }

    // spawn a ball
    if spawn_pressed {
        state.command_queue.push_back(PlinkoUiCommand::SpawnBall);
    }

    Ok(())
}

fn commit_slider(response: &egui::Response) -> bool {
    response.drag_stopped() || (response.changed() && !response.is_pointer_button_down_on())
}

fn register_plinko_ui(engine: &mut Engine) -> Result<()> {
    let state = engine
        .get_global_component::<PlinkoTuningComponent>()?
        .state
        .clone();

    engine
        .get_global_component_mut::<EguiManagerComponent>()?
        .register_ui("plinko.controls", move |ctx| {
            egui::Window::new("Plinko Controls")
                .default_open(true)
                .show(ctx, |ui| {
                    let mut state = state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());

                    let response = ui.add(
                        egui::Slider::new(&mut state.draft.ball_radius, 0.05..=0.40)
                            .text("Ball Radius"),
                    );

                    if commit_slider(&response) {
                        state.commited.ball_radius = state.draft.ball_radius;
                    }

                    let response = ui.add(
                        egui::Slider::new(&mut state.draft.gravity_y_mps2, -30.0..=10.0)
                            .text("Gravity Y (m/s^2)"),
                    );

                    if commit_slider(&response) {
                        state.commited.gravity_y_mps2 = state.draft.gravity_y_mps2;
                    }

                    let clicked = ui.button("Spawn a Ball").clicked();

                    if clicked {
                        state.command_queue.push_back(PlinkoUiCommand::SpawnBall);
                    }

                    let clicked = ui.button("Clear Balls").clicked();

                    if clicked {
                        state.command_queue.push_back(PlinkoUiCommand::ClearBalls);
                    }
                });
        });

    Ok(())
}

fn update_plinko_ui_system(engine: &mut Engine) -> Result<()> {
    let state = engine
        .get_global_component::<PlinkoTuningComponent>()?
        .state
        .clone();

    let (desired, commands) = {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        (state.commited, std::mem::take(&mut state.command_queue))
    };

    let applied = engine
        .get_global_component::<PlinkoTuningComponent>()?
        .applied;

    let ball_radius_changed = desired.ball_radius != applied.ball_radius;
    let gravity_changed = desired.gravity_y_mps2 != applied.gravity_y_mps2;
    let spawn_interval_changed = desired.spawn_interval != applied.spawn_interval;

    if ball_radius_changed {
        engine
            .get_global_component_mut::<StateComponent>()?
            .ball_radius = desired.ball_radius;
    }

    if gravity_changed {
        engine
            .get_global_component_mut::<PhysicsWorldComponent>()?
            .set_gravity(Vector3f::new(
                0.0,
                desired.gravity_y_mps2 / METERS_PER_WORLD_UNIT,
                0.0,
            ));
    }

    if spawn_interval_changed {
        engine.get_global_component_mut::<StateComponent>()?.timeout = desired.spawn_interval;
    }

    engine
        .get_global_component_mut::<PlinkoTuningComponent>()?
        .applied = desired;

    if commands
        .iter()
        .any(|command| matches!(command, PlinkoUiCommand::ClearBalls))
    {
        clear_all_balls(engine)?;
    } else {
        for command in commands {
            if matches!(command, PlinkoUiCommand::SpawnBall) {
                spawn_one_ball(engine)?;
            }
        }
    }

    Ok(())
}
