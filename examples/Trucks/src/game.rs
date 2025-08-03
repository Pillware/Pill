use pill_engine::game::*;
use pill_engine::{define_component};
use crate::player_movement::{player_movement_system, CarControllerComponent};
use crate::free_camera::free_camera_system;
use crate::player_physics_movement;
use crate::player_physics_movement::player_physics_movement_system;
use rand::Rng;

#[cfg(feature = "net")]
use pill_engine::{
    NetState, NetSide, NetStats, NetworkStateComponent, NetEntityState, EntityUpdate, NetworkUpdatePayload, NetEntityAction,
};

#[cfg(feature = "net")]
use pill_net::{WireMsg, WireTag, cli_send, cli_flush};

// ----- CONSTANTS -----------------------------------------------------------

// Move speed in world units per second
const PILL_MOVE_SPEED: f32 = 3.0;
const UPDATE_FREQ_HZ: f32 = 24.0;
const UPDATE_FREQ_SEC: f32 = 1.0 / UPDATE_FREQ_HZ;

const REMOTE_SERVER_ADDR: &str = "145.223.100.1";
//const REMOTE_SERVER_ADDR: &str = "127.0.0.1";
const REMOTE_SERVER_PORT: u16 = 5000;


// ───────────────────────────────────────────────────────────────────────────
//  Track whether we already sent JOIN after connecting
// ───────────────────────────────────────────────────────────────────────────
#[cfg(feature = "net")]
pub struct JoinState {
    pub sent: bool,
}
#[cfg(feature = "net")]
impl GlobalComponent for JoinState {}
#[cfg(feature = "net")]
impl PillTypeMapKey for JoinState {
    type Storage = GlobalComponentStorage<Self>;
}

define_component!(PlayerTagComponent { });

pub struct TargetTransformComponent(pub TransformComponent);

impl PillTypeMapKey for TargetTransformComponent {
    type Storage = ComponentStorage<Self>;
}

impl TargetTransformComponent {
	pub fn new(transform_component: TransformComponent) -> Self {
		Self(transform_component)
	}
}

impl Component for TargetTransformComponent {}

const USE_PHYSICS: bool = false;

pub struct Game { }

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {

		// --- Basic setup ---

		// Create scene
		let active_scene = engine.create_scene("Default")?;
        engine.set_active_scene(active_scene)?;

        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
		engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
		engine.register_component::<AudioListenerComponent>(active_scene)?;
		engine.register_component::<AudioSourceComponent>(active_scene)?;
		engine.register_component::<PlayerTagComponent>(active_scene)?;
		engine.register_component::<TargetTransformComponent>(active_scene)?;
		engine.register_component::<RigidBodyComponent>(active_scene)?;
		engine.register_component::<ColliderComponent>(active_scene)?;
        engine.register_component::<NetworkStateComponent>(active_scene)?;
        engine.register_component::<CarControllerComponent>(active_scene)?;

        // Add systems
		#[cfg(feature = "net")]
        engine.add_system("NetworkingSystemClient", pill_engine::networking_system_client)?;
        #[cfg(feature = "net")]
        engine.add_system("ClientUpdateSystem", client_update_system)?;
		#[cfg(feature = "net")]
        engine.add_system("SendJoin", send_join_system)?;

		// Add systems
		if USE_PHYSICS {
			engine.add_system("PlayerPhysicsMovementSystem", player_physics_movement_system)?;
		} else {
			engine.add_system("PlayerMovementSystem", player_movement_system)?;
		}

		//engine.add_system("FreeCameraSystem", free_camera_system)?;


		// Add meshes
        let truck_mesh_handle = engine.add_resource(
			Mesh::new("Truck", "models/Truck.obj".into())
		)?;

		let ground_mesh_handle = engine.add_resource(
			Mesh::new("Ground", "models/Plane.obj".into())
		)?;

		let cube_mesh_handle = engine.add_resource(
			Mesh::new("Cube", "models/Cube.obj".into())
		)?;

		let crate_mesh_handle = engine.add_resource(
			Mesh::new("Crate", "models/Crate.obj".into())
		)?;

		let axis_gizmo_mesh_handle = engine.add_resource(
			Mesh::new("AxisGizmo", "models/AxisGizmo.obj".into())
		)?;

		let arena_mesh_handle = engine.add_resource(
			Mesh::new("Arena", "models/Arena.obj".into())
		)?;

		let stones_mesh_handle = engine.add_resource(
			Mesh::new("Stones", "models/Stones.obj".into())
		)?;

		// Add textures
		let stones_color_texture_handle = engine.add_resource::<Texture>(
			Texture::new(
				"StonesColor",
				TextureType::Color,
				ResourceLoadType::Path("textures/StonesColor.png".into())
			)
		)?;

		let stones_normal_texture_handle = engine.add_resource::<Texture>(
			Texture::new(
				"StonesNormal",
				TextureType::Normal,
				ResourceLoadType::Path("textures/StonesNormal.png".into())
			)
		)?;

		let stones_material_handle = engine.add_resource::<Material>(
			Material::builder("Stones")
				.texture("Color", stones_color_texture_handle)?
				.texture("Normal", stones_normal_texture_handle)?
				.scalar("Specularity", 0.3)?
				.build()
		)?;


		let dirt_texture_handle = engine.add_resource::<Texture>(
			Texture::new(
				"TruckColor",
				TextureType::Color,
				ResourceLoadType::Path("textures/Dirt.png".into())
			)
		)?;

		let crate_color_texture_handle = engine.add_resource::<Texture>(
			Texture::new(
				"CrateColor",
				TextureType::Color,
				ResourceLoadType::Path("textures/CrateColor.png".into())
			)
		)?;

		let crate_normal_texture_handle = engine.add_resource::<Texture>(
			Texture::new(
				"CrateNormal",
				TextureType::Normal,
				ResourceLoadType::Path("textures/CrateNormal.png".into())
			)
		)?;

		let axis_gizmo_colors_texture_handle = engine.add_resource::<Texture>(
			Texture::new(
				"AxisGizmoColors",
				TextureType::Color,
				ResourceLoadType::Path("textures/AxisGizmoColors.png".into())
			)
		)?;

		// Add materials
		let truck_material_handle = engine.add_resource::<Material>(
			Material::builder("Truck")
				.color("Tint", Color::new(0.97, 0.72, 0.09))?
				.scalar("Specularity", 0.5)?
				.build()
		)?;

		let ground_material_handle = engine.add_resource::<Material>(
			Material::builder("Ground")
				.texture("Color", dirt_texture_handle)?
				.color("Tint", Color::new(1.0, 1.0, 1.0))?
				.scalar("Specularity", 0.1)?
				.build()
		)?;

		let axis_gizmo_material_handle = engine.add_resource::<Material>(
			Material::builder("AxisGizmo")
				.texture("Color", axis_gizmo_colors_texture_handle)?
				.scalar("Specularity", 0.0)?
				.build()
		)?;

		let crate_material_handle = engine.add_resource::<Material>(
			Material::builder("Crate")
				.texture("Color", crate_color_texture_handle)?
				.texture("Normal", crate_normal_texture_handle)?
				.color("Tint", Color::new(1.0, 1.0, 1.0))?
				.scalar("Specularity", 0.3)?
				.build()
		)?;

		// --- Create entities ---

		// Create camera entity
		let initial_camera_transform = TransformComponent::builder()
			.position(Vector3f::new(0.0, 6.0, 10.0))
			.build();
		engine.build_entity(active_scene)
			.with_component(initial_camera_transform.clone())
			.with_component(CameraComponent::builder()
				.enabled(true)
				.fov(60.0)
				.clear_color(Color::new(0.3, 0.3, 0.3))
				.build())
				.with_component(TargetTransformComponent::new(initial_camera_transform))
			.build();

		// Create ground entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(0.0, 0.0, 0.0))
				.scale(Vector3f::new(1.0, 1.0, 1.0))
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&ground_material_handle)
				.mesh(&arena_mesh_handle)
				.build())
			.with_component(RigidBodyComponent::builder().body_type(RigidBodyType::Fixed)
				.build())
			.with_component(ColliderComponent::builder().shape(SharedShape::cuboid(200.0, 0.5, 200.0))
				.build())
			.build();

		// Create player truck entity
		let initial_player_transform = TransformComponent::builder()
			.position(Vector3f::new(0.0, 0.0, 0.0))
			.build();

		let mut player_entity_builder = engine.build_entity(active_scene)
			.with_component(initial_player_transform.clone())
			.with_component(MeshRenderingComponent::builder()
				.material(&truck_material_handle)
				.mesh(&truck_mesh_handle)
				.build())
			.with_component(PlayerTagComponent {})
            .with_component(CarControllerComponent { speed: 0.0, direction: 0.0, last_steer: 0.0 })
			.with_component(TargetTransformComponent::new(initial_player_transform.clone()));

		if USE_PHYSICS {
			player_entity_builder = player_entity_builder
			.with_component(RigidBodyComponent::builder().body_type(RigidBodyType::Dynamic)
				.build())
			.with_component(ColliderComponent::builder().shape(SharedShape::cuboid(3.0, 2.0, 3.0))
				.build());
		}

		let player = player_entity_builder.build();


		// Create cube entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(0.0, 13.0, 0.0))
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&truck_material_handle)
				.mesh(&cube_mesh_handle)
				.build())
			.with_component(RigidBodyComponent::builder()
				.build())
			.with_component(ColliderComponent::builder()
				.build())
			.build();

		// Create axis gizmo entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&axis_gizmo_material_handle)
				.mesh(&axis_gizmo_mesh_handle)
				.build())
			.build();

		// Create stones entity
		engine.build_entity(active_scene)
			.with_component(TransformComponent::builder()
				.position(Vector3f::new(0.0, 0.0, 0.0))
				.scale(Vector3f::new(1.0, 1.0, 1.0))
				.build())
			.with_component(MeshRenderingComponent::builder()
				.material(&stones_material_handle)
				.mesh(&stones_mesh_handle)
				.build())
			.build();

         //let mut rng = rand::thread_rng();
		// Create crates
		//for i in 0..10 {
		//	for j in 0..10 {
		//		let random_offset = Vector3f::new(rng.gen_range(-2.0..2.0), rng.gen_range(-2.0..2.0), rng.gen_range(-2.0..2.0));
		//		spawn_crates(engine, active_scene, &crate_material_handle, &crate_mesh_handle, random_offset);
		//	}
		//}

        // ───── net setup on client builds ─────────────────────────────────-
        #[cfg(feature = "net")]
        {
            use rand::SeedableRng;

            engine.add_global_component(NetStats::new())?;
			engine.add_global_component(JoinState { sent: false })?;
            let client_id = rand::thread_rng().gen_range(1..=10_000_000);
            let server_addr = format!("{REMOTE_SERVER_ADDR}:{REMOTE_SERVER_PORT}");
            engine.add_global_component(NetState::new_client(&server_addr, client_id)?)?;

            println!("Client will connect to {server_addr} with ID {client_id}");

            // Add the network component marker so the server can identify us
            let net_entity_id = rand::thread_rng().gen_range(1..=1000);
            engine.add_component_to_entity(
                active_scene,
                player,
                NetworkStateComponent {
                    net_entity_id,
                    owner_id: client_id,
                    state: NetEntityState::Spawn,
                    transform: Some(initial_player_transform.clone()),
                },
            )?;

			let truck_mat = engine.get_resource_mut::<Material>(&truck_material_handle)?;
			
			// Use net_entity_id as seed to generate a random color
			let mut rng = rand::rngs::StdRng::seed_from_u64(net_entity_id as u64);
			let r = rng.gen_range(0.2..1.0);
			let g = rng.gen_range(0.2..1.0);
			let b = rng.gen_range(0.2..1.0);
			truck_mat.set_color("Tint", Color::new(r, g, b))?;
        }

		Ok(())
	}
}

//fn spawn_crates(engine: &mut Engine, active_scene: SceneHandle, crate_material_handle: &MaterialHandle, crate_mesh_handle: &MeshHandle, offset: Vector3f) {
//	engine.build_entity(active_scene)
//	.with_component(TransformComponent::builder()
//		.position(Vector3f::new(0.0, 10.0, 0.0) + offset)
//		.scale(Vector3f::new(1.0, 1.0, 1.0))
//		.build())
//	.with_component(MeshRenderingComponent::builder()
//		.material(&crate_material_handle)
//		.mesh(&crate_mesh_handle)
//		.build())
//	.with_component(RigidBodyComponent::builder().body_type(RigidBodyType::Dynamic)
//		.build())
//	.with_component(ColliderComponent::builder().shape(SharedShape::cuboid(0.5, 0.5, 0.5))
//		.build())
//	.build();
//
//}

//  Helper: actually send the batch of entity updates after the ECS loop.
// ───────────────────────────────────────────────────────────────────────────
#[cfg(feature = "net")]
fn flush_updates_to_server(engine: &mut Engine, updates: Vec<EntityUpdate>) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }

    use bincode;

    //println!("Flushing {} updates to server", updates.len());

    let my_id = engine.get_global_component::<NetState>()?.my_id;
    let payload = NetworkUpdatePayload {
        client_id: my_id as u64,
        updates,
        timestamp: engine.get_global_component::<TimeComponent>()?.time,
        sequence: rand::thread_rng().gen(),
    };

    if let NetSide::Client(net) = &mut engine.get_global_component_mut::<NetState>()?.side {
        cli_send(
            net,
            &WireMsg {
                tag: WireTag::Update,
                data: bincode::serialize(&payload)?,
            },
        )?;
        cli_flush(net)?;
    }
    Ok(())
}

// ───────────────────────────────────────────────────────────────────────────
//  Player-controlled pill movement & optional network sync
// ───────────────────────────────────────────────────────────────────────────
fn client_update_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
    #[cfg(feature = "net")]
    let owner_id = engine.get_global_component::<NetState>()?.my_id;

    // TODO: this should be delayed because otherwise the client might not be connected yet
        // ─── 1. Is the socket ready … ? ───────────────────────────────────────
    #[cfg(feature = "net")]
    {
        let net_state = engine.get_global_component::<NetState>()?;
        let connected = matches!(
            &net_state.side,
            NetSide::Client(net) if net.client.is_connected()
        );
        // … and have we already told the server who we are?
        let join_sent = engine.get_global_component::<JoinState>()?.sent;

        if !(connected && join_sent) {
            // Handshake still running → do nothing this frame
            return Ok(());
        }
    }

    #[cfg(feature = "net")]
    let mut pending_updates: Vec<EntityUpdate> = Vec::new();

    // ── first pass: move entities & collect updates -----------------------
    for (_, transform, _, net_state) in engine.iterate_three_components_mut::<
        TransformComponent,
        PlayerTagComponent,
        NetworkStateComponent,
    >()? {
        #[cfg(feature = "net")]
        if net_state.owner_id != owner_id {
            continue; // only move entities we own
        }

        #[cfg(feature = "net")]
        {
            net_state.transform = Some(transform.clone());
            net_state.transform.as_mut().unwrap().net_dirty = false;
            pending_updates.push(EntityUpdate {
                action: NetEntityAction::Update,
                net_state: net_state.clone(),
                transform: Some(transform.clone()),
            });
            //println!("Pushed update for entity with ID {}", net_state.net_entity_id);
        }
    } // iterator dropped here – the &mut Engine borrow ends

    #[cfg(feature = "net")]
    flush_updates_to_server(engine, pending_updates)?;

    Ok(())
}

// ───────────────────────────────────────────────────────────────────────────
//  System: once connected, send JOIN exactly once
// ───────────────────────────────────────────────────────────────────────────
#[cfg(feature = "net")]
fn send_join_system(engine: &mut Engine) -> Result<()> {
    use pill_net::{NetClient, WireMsg, WireTag};

    // 1. Short immutable borrow: are we connected yet?
    let connected = {
        let state = engine.get_global_component::<NetState>()?;
        matches!(&state.side, NetSide::Client(net) if net.client.is_connected())
    };
    if !connected {
        return Ok(()); // handshake still in progress
    }

    // 2. Have we already sent JOIN?
    if engine.get_global_component::<JoinState>()?.sent {
        return Ok(());
    }

    // 3. We’re connected and haven’t sent JOIN – do it now (separate scope)
    {
        let mut state = engine.get_global_component_mut::<NetState>()?;
        if let NetSide::Client(net) = &mut state.side {
            cli_send(
                net,
                &WireMsg {
                    tag:  WireTag::Join,
                    data: Vec::new(),
                },
            )?;
            cli_flush(net)?;
        }
    }

    // 4. Mark as sent (new mutable borrow, no overlap with the one above)
    engine.get_global_component_mut::<JoinState>()?.sent = true;
    println!("JOIN sent after connection established");

    Ok(())
}
