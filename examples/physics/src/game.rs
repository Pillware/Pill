use pill_engine::{define_component, define_global_component, game::*};

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

define_component!(BallComponent {});

// TODO: add egui tweaks - ball size, pegs size, spawn frequency. physics parameters?
define_global_component!(StateComponent {
    elapsed: f32,
    timeout: f32,
    spawn_index: u64
});

define_component!(CameraMovementComponent {
    orbit_speed: f32,
    zoom_speed: f32,
    angle: f32,
    radius: f32,
    delta_y: f32,
    delta_z: f32,
});

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
        engine.register_component::<RigidBodyComponent>(active_scene)?;
        engine.register_component::<ColliderComponent>(active_scene)?;
        engine.register_component::<BallComponent>(active_scene)?;
        engine.register_component::<CameraMovementComponent>(active_scene)?;

        // Add systems
        //engine.add_system("ball_spawning_system", ball_spawning_system)?;
        engine.add_system("camera_movement_system", camera_movement_system)?;
        engine.add_global_component(StateComponent {
            elapsed: 0.0,
            timeout: 1.0,
            spawn_index: 0,
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

        Ok(())
    }
}

fn ball_spawning_system(engine: &mut Engine) -> Result<()> {
    // if a timeout has passed, spawn the ball (every 1s for now)
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
    let scene_handle = engine.get_active_scene_handle()?;
    let ball_mesh = engine.get_global_component::<PlinkoBoard>()?.ball_mesh;
    let ball_material = engine.get_global_component::<PlinkoBoard>()?.ball_material;
    let state: &mut StateComponent = engine.get_global_component_mut::<StateComponent>()?;
    if state.elapsed + dt > state.timeout {
        state.elapsed = 0.0;
        let spawn_index = state.spawn_index;
        state.spawn_index = state.spawn_index.wrapping_add(1);

        spawn_ball(
            engine,
            scene_handle,
            &ball_mesh,
            &ball_material,
            spawn_index,
        )?;
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
    //
    // Triangle mesh:
    // - local X/Z form the visible right-triangle footprint
    // - local Y is depth/thickness
    // - after +90° around X, local X/Z become board X/Y, local Y becomes world Z

    const BOARD_Z: f32 = 1.0;
    const BOARD_HALF_WIDTH: f32 = 10.0;
    const BOARD_HALF_HEIGHT: f32 = 14.0;
    const BOARD_CENTER_Y: f32 = 4.0;

    const WALL_THICKNESS: f32 = 0.35;

    const PEG_RADIUS: f32 = 0.38;
    const PEG_HALF_DEPTH: f32 = 0.55;

    const BIN_COUNT: usize = 9;
    const DIVIDER_HALF_WIDTH: f32 = 0.18;
    const DIVIDER_HALF_HEIGHT: f32 = 1.8;

    // Visible side triangles.
    const SIDE_TRIANGLE_COUNT: usize = 6;
    const SIDE_TRIANGLE_START_Y: f32 = 12.8;
    const SIDE_TRIANGLE_STEP_Y: f32 = 2.8;
    const SIDE_TRIANGLE_HEIGHT: f32 = 1.15;
    const SIDE_TRIANGLE_INSET_X: f32 = 1.35;
    const SIDE_TRIANGLE_HALF_DEPTH: f32 = 0.28;

    // Invisible physics kicker.
    // Slightly overlaps the wall to remove seam traps.
    const SIDE_KICKER_OVERLAP_X: f32 = 0.65;
    const SIDE_KICKER_HALF_THICKNESS: f32 = 0.12;

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
    // Outer frame: left wall
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
    // Outer frame: right wall
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
    // Outer frame: bottom wall
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
    // Side triangles
    //
    // Visual:
    // - actual Triangle mesh, no strip Christmas tree garbage
    //
    // Physics:
    // - invisible low-friction diagonal cuboid
    // - overlaps into side wall to avoid seam traps
    // ---------------------------------------------------------------------
    let left_wall_inner_x = -BOARD_HALF_WIDTH + WALL_THICKNESS;
    let right_wall_inner_x = BOARD_HALF_WIDTH - WALL_THICKNESS;

    for i in 0..SIDE_TRIANGLE_COUNT {
        let top_y = SIDE_TRIANGLE_START_Y - i as f32 * SIDE_TRIANGLE_STEP_Y;
        let bottom_y = top_y - SIDE_TRIANGLE_HEIGHT;

        // Left triangle.
        //
        // Desired footprint:
        //
        // wall top     *
        //              |\
        //              | \
        // wall bottom  *--* inward tip
        //
        let left_center = Vector3f::new(
            left_wall_inner_x + SIDE_TRIANGLE_INSET_X * 0.5,
            (top_y + bottom_y) * 0.5,
            -0.03,
        );

        spawn_side_triangles_visual(
            engine,
            scene,
            plinko,
            left_center,
            true,
            SIDE_TRIANGLE_INSET_X,
            SIDE_TRIANGLE_HEIGHT,
            SIDE_TRIANGLE_HALF_DEPTH,
        )?;

        let left_visual_end =
            Vector3f::new(left_wall_inner_x + SIDE_TRIANGLE_INSET_X, bottom_y, 0.0);

        let left_physics_start =
            Vector3f::new(left_wall_inner_x - SIDE_KICKER_OVERLAP_X, top_y, 0.0);

        spawn_invisible_side_kicker_collider(
            engine,
            scene,
            left_physics_start,
            left_visual_end,
            SIDE_KICKER_HALF_THICKNESS,
        )?;

        // Right triangle, mirrored.
        //
        // Desired footprint:
        //
        //        * wall top
        //       /|
        //      / |
        // tip *--* wall bottom
        //
        let right_center = Vector3f::new(
            right_wall_inner_x - SIDE_TRIANGLE_INSET_X * 0.5,
            (top_y + bottom_y) * 0.5,
            -0.03,
        );

        spawn_side_triangles_visual(
            engine,
            scene,
            plinko,
            right_center,
            false,
            SIDE_TRIANGLE_INSET_X,
            SIDE_TRIANGLE_HEIGHT,
            SIDE_TRIANGLE_HALF_DEPTH,
        )?;

        let right_visual_end =
            Vector3f::new(right_wall_inner_x - SIDE_TRIANGLE_INSET_X, bottom_y, 0.0);

        let right_physics_start =
            Vector3f::new(right_wall_inner_x + SIDE_KICKER_OVERLAP_X, top_y, 0.0);

        spawn_invisible_side_kicker_collider(
            engine,
            scene,
            right_physics_start,
            right_visual_end,
            SIDE_KICKER_HALF_THICKNESS,
        )?;
    }

    // ---------------------------------------------------------------------
    // Peg field
    // ---------------------------------------------------------------------
    for row in 0..6 {
        let y = 14.0 - row as f32 * 3.0;
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
    // Bottom compartments / score bins
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

fn spawn_invisible_side_kicker_collider(
    engine: &mut Engine,
    scene: SceneHandle,
    physics_start: Vector3f,
    physics_end: Vector3f,
    half_thickness: f32,
) -> Result<()> {
    let dx = physics_end.x - physics_start.x;
    let dy = physics_end.y - physics_start.y;

    let length = (dx * dx + dy * dy).sqrt();
    let half_length = length * 0.5;
    let angle_deg = dy.atan2(dx).to_degrees();

    let center = Vector3f::new(
        (physics_start.x + physics_end.x) * 0.5,
        (physics_start.y + physics_end.y) * 0.5,
        0.0,
    );

    engine
        .build_entity(scene)
        .with_component(
            TransformComponent::builder()
                .position(center)
                .rotation(Vector3f::new(90.0, 0.0, angle_deg))
                .scale(Vector3f::new(half_length, 1.0, half_thickness))
                .build(),
        )
        .with_component(
            RigidBodyComponent::builder()
                .body_type(RigidBodyType::Fixed)
                .build(),
        )
        .with_component(
            ColliderComponent::builder()
                .shape(SharedShape::cuboid(half_length, 0.35, half_thickness))
                .friction(0.0)
                .restitution(0.18)
                .build(),
        )
        .build();

    Ok(())
}

// Spawn a ball in the same place falling down, called periodically
fn spawn_ball(
    engine: &mut Engine,
    scene: SceneHandle,
    mesh: &MeshHandle,
    material: &MaterialHandle,
    spawn_index: u64,
) -> Result<()> {
    const SPAWN_XS: [f32; 8] = [-0.9, 0.75, -0.35, 1.15, -1.25, 0.45, -0.65, 0.95];
    let spawn_x = SPAWN_XS[spawn_index as usize % SPAWN_XS.len()];

    engine
        .build_entity(scene)
        .with_component(
            TransformComponent::builder()
                .position(Vector3f::new(spawn_x, 18.0, 0.0))
                .scale(Vector3f::new(0.25, 0.25, 0.25))
                .build(),
        )
        .with_component(
            MeshRenderingComponent::builder()
                .mesh(mesh)
                .material(material)
                .build(),
        )
        .with_component(BallComponent {})
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
                .shape(SharedShape::ball(0.55))
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
