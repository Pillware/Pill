use pill_engine::game::*;

use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

use pill_engine::internal::{
    TransformComponent,
    NetworkManagerComponent, NetworkSide, NetworkStateComponent, NetworkEntityState, EntityUpdate, NetworkUpdatePayload, NetworkEntityAction,
    networking_system_client,
};

use pill_core::{NetworkPacket, NetworkAction, client_send, client_flush, DISTINCT_COLOR_PALETTE};

// ----- CONSTANTS -----------------------------------------------------------

// Move speed in world units per second
const PILL_MOVE_SPEED: f32 = 3.0;
const UPDATE_FREQUENCY_HZ: f32 = 24.0;
const UPDATE_FREQUENCY_SEC: f32 = 1.0 / UPDATE_FREQUENCY_HZ;

//const REMOTE_SERVER_ADDRESS: &str = "145.223.100.1";
const REMOTE_SERVER_ADDRESS: &str = "127.0.0.1";
const REMOTE_SERVER_PORT: u16 = 5000;

pub struct PillComponent;

impl Component for PillComponent {}
impl PillTypeMapKey for PillComponent {
    type Storage = ComponentStorage<Self>;
}

// ───────────────────────────────────────────────────────────────────────────
//                                GAME
// ───────────────────────────────────────────────────────────────────────────

pub struct Game;

impl PillGame for Game {
    fn start(&self, engine: &mut Engine) -> Result<()> {
        // Create scene
        let active_scene = engine.create_scene("NetMinimal")?;
        engine.set_active_scene(active_scene)?;

        // Register components
        engine.register_component::<TransformComponent>(active_scene)?;
        engine.register_component::<MeshRenderingComponent>(active_scene)?;
        engine.register_component::<CameraComponent>(active_scene)?;
        engine.register_component::<AudioListenerComponent>(active_scene)?;
        engine.register_component::<AudioSourceComponent>(active_scene)?;
        engine.register_component::<PillComponent>(active_scene)?;
        engine.register_component::<NetworkStateComponent>(active_scene)?;

        // Add systems
        engine.add_system("NetworkingSystemClient", networking_system_client)?;
        engine.add_system("PillMovement", pill_movement_system)?;

        // Add meshes
        let pill_mesh = Mesh::new("Pill", "models/pill.obj".into());
        let pill_mesh_handle = engine.add_resource(pill_mesh)?;

        // Add textures
        let pill_color_texture = Texture::new(
            "pill_color",
            TextureType::Color,
            ResourceLoadType::Path("textures/pill_color.png".into()),
        );
        let pill_color_texture_handle = engine.add_resource::<Texture>(pill_color_texture)?;
        let pill_normal_texture = Texture::new(
            "pill_normal",
            TextureType::Normal,
            ResourceLoadType::Path("textures/pill_normal.png".into()),
        );
        let pill_normal_texture_handle = engine.add_resource::<Texture>(pill_normal_texture)?;

        // Add materials
        let mut pill_material = Material::new("pill");
        pill_material.set_texture("color", pill_color_texture_handle)?;
        pill_material.set_texture("normal", pill_normal_texture_handle)?;
        pill_material.set_color("tint", Color::new(1.0, 1.0, 1.0))?;
        pill_material.set_scalar("specularity", 0.5)?;
        let pill_material_handle = engine.add_resource::<Material>(pill_material)?;

        // Create camera entity
        let camera = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(0.0, 0.0, -8.0))
            .rotation(Vector3f::new(0.0, 0.0, -20.0))
            .build();
        engine.add_component_to_entity(active_scene, camera, transform_component)?;
        let camera_component = CameraComponent::builder().enabled(true).build();
        engine.add_component_to_entity(active_scene, camera, camera_component)?;

        // Create pill entity ------------------------------------------------
        let pill = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(rand::thread_rng().gen_range(-2.0..=2.0), 0.0, 0.0))
            .rotation(Vector3f::new(-210.0, 0.0, 0.0))
            .build();
        engine.add_component_to_entity(active_scene, pill, transform_component.clone())?;
        let mesh_rendering_component = MeshRenderingComponent::builder()
            .mesh(&pill_mesh_handle)
            .material(&pill_material_handle)
            .build();
        engine.add_component_to_entity(active_scene, pill, mesh_rendering_component)?;
        engine.add_component_to_entity(active_scene, pill, PillComponent)?;

        let client_id = {
            let args: Vec<String> = std::env::args().collect();
            if args.len() > 1 {
               args[1].parse::<u64>().unwrap_or(0)
            } else {
                rand::thread_rng().gen_range(1..=10_000_000)
            }
        };
		let server_address = format!("{REMOTE_SERVER_ADDRESS}:{REMOTE_SERVER_PORT}");

        let mut net_state = NetworkManagerComponent::new_client(&server_address, client_id)?;
        net_state.spawn_handlers.insert("player".into(), spawn_player);
        net_state.despawn_handlers.insert("player".into(), despawn_player);
        engine.add_global_component(net_state);

		println!("Client will connect to {server_address} with ID {client_id}");

		// Add the network component marker so the server can identify us
		let network_entity_id = rand::thread_rng().gen_range(1..=1000);
		engine.add_component_to_entity(
			active_scene,
			pill,
			NetworkStateComponent {
				network_entity_id,
				owner_id: client_id,
				state: NetworkEntityState::Spawn,
				transform: Some(transform_component),
                entity_type: "player".into(),
			},
		)?;

        Ok(())
    }
}

// ───────────────────────────────────────────────────────────────────────────
//  Helper: actually send the batch of entity updates after the ECS loop.
// ───────────────────────────────────────────────────────────────────────────
fn flush_updates_to_server(engine: &mut Engine, updates: Vec<EntityUpdate>) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }

    use bincode;

    //println!("Flushing {} updates to server", updates.len());

    let my_id = engine.get_global_component::<NetworkManagerComponent>()?.my_id;
    let payload = NetworkUpdatePayload {
        client_id: my_id as u64,
        updates,
        timestamp: engine.get_global_component::<TimeComponent>()?.time,
    };

    if let NetworkSide::Client(net) = &mut engine.get_global_component_mut::<NetworkManagerComponent>()?.side {
        client_send(
            net,
            &NetworkPacket {
                tag: NetworkAction::Update,
                data: bincode::serialize(&payload)?,
            },
        )?;
        client_flush(net)?;
    }
    Ok(())
}

// ───────────────────────────────────────────────────────────────────────────
//  Player-controlled pill movement & optional network sync
// ───────────────────────────────────────────────────────────────────────────
fn pill_movement_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
    let owner_id = engine.get_global_component::<NetworkManagerComponent>()?.my_id;
    let input = engine.get_global_component_mut::<InputComponent>()?;

    // Build a direction vector from arrow-key input ------------------------
    let mut dir = Vector3f::new(0.0, 0.0, 0.0);
    if input.get_key(KeyboardKey::ArrowUp) {
        dir.z -= 1.0;
    }
    if input.get_key(KeyboardKey::ArrowDown) {
        dir.z += 1.0;
    }
    if input.get_key(KeyboardKey::ArrowLeft) {
        dir.x -= 1.0;
    }
    if input.get_key(KeyboardKey::ArrowRight) {
        dir.x += 1.0;
    }
    if input.get_key(KeyboardKey::ControlLeft) {
        dir.y += 1.0;
    }
    if input.get_key(KeyboardKey::ShiftLeft) {
        dir.y -= 1.0;
    }

    if dir.x == 0.0 && dir.y == 0.0 {
        return Ok(()); // nothing to do this frame
    }

    // Normalize XY so diagonal speed == straight speed ---------------------
    let len = (dir.x * dir.x + dir.y * dir.y).sqrt();
    let inv = 1.0 / len;
    dir.x *= inv;
    dir.y *= inv;

    let mut pending_updates: Vec<EntityUpdate> = Vec::new();

    // ── first pass: move entities & collect updates -----------------------
    for (_, transform, _, net_state) in engine.iterate_three_components_mut::<
        TransformComponent,
        PillComponent,
        NetworkStateComponent,
    >()? {
        if net_state.owner_id != owner_id {
            continue; // only move entities we own
        }

        transform.translate_world(dt * PILL_MOVE_SPEED * dir);

        {
            net_state.transform = Some(transform.clone());
            pending_updates.push(EntityUpdate {
                action: NetworkEntityAction::Update,
                net_state: net_state.clone(),
                transform: Some(transform.clone()),
            });
            //println!("Pushed update for entity with ID {}", net_state.network_entity_id);
        }
    } // iterator dropped here – the &mut Engine borrow ends

    flush_updates_to_server(engine, pending_updates)?;

    Ok(())
}

fn spawn_player(engine: &mut Engine, net_state_component: &NetworkStateComponent, transform: &TransformComponent) -> Result<()> {
    let my_id = engine.get_global_component_mut::<NetworkManagerComponent>()?.my_id;
    let scene = engine.get_active_scene_handle()?;
    println!("[SPAWN] Spawning player with nid{ } for cid {} with transform {:?}", net_state_component.network_entity_id, my_id, transform);

    // randomness for capsules tint and transforms
    //let mut rng = rng();
    let network_entity_id = net_state_component.network_entity_id;

	let mut rng = StdRng::seed_from_u64(network_entity_id as u64);
	let index = rng.random_range(0..DISTINCT_COLOR_PALETTE.len());
	let (r, g, b) = DISTINCT_COLOR_PALETTE[index];
	// // Use network_entity_id as seed to generate a random color
	// let mut rng = rand::rngs::StdRng::seed_from_u64(network_entity_id as u64);
	// let r = rng.gen_range(0.2..1.0);
	// let g = rng.gen_range(0.2..1.0);
	// let b = rng.gen_range(0.2..1.0);

    let (mesh, mat) = {
        use pill_core::Color;
        //let mesh: MeshHandle = match engine.get_resource_handle::<Mesh>("Truck") {
        //    Ok(h) => h,
        //    Err(_) => engine.add_resource(Mesh::new("Truck", "models/Truck.obj".into()))?,
        let mesh: MeshHandle = match engine.get_resource_handle::<Mesh>("pill") {
            Ok(h) => h,
            Err(_) => engine.add_resource(Mesh::new("pill", "models/pill.obj".into()))?,
        };

        let mat = engine.add_resource::<Material>(
        Material::builder(&network_entity_id.to_string())
            .color("tint", Color::new(r, g, b))?
            .scalar("specularity", 0.5)?
            .build()
		)?;

        (mesh, mat)
    };


    let ent = engine.create_entity(scene)?;

	let mut ns = net_state_component.clone();
	ns.state = NetworkEntityState::Alive;

    engine.add_component_to_entity(scene, ent, ns)?;

    engine.add_component_to_entity(scene, ent,*transform)?;

    // TODO: missing playerTag and targetTransform components

	engine.add_component_to_entity(scene, ent, MeshRenderingComponent::builder().mesh(&mesh).material(&mat).build())?;

    println!("[SPAWN] finished with nid{ } for cid {} with transform {:?}", net_state_component.network_entity_id, my_id, transform);
    Ok(())
}

fn despawn_player(engine: &mut Engine, net_state_component: &NetworkStateComponent) -> Result<()> {
    let my_id = engine.get_global_component_mut::<NetworkManagerComponent>()?.my_id;
    let scene = engine.get_active_scene_handle()?;
    println!("[DESPAWN] Despawning player with nid{ } for cid {}", net_state_component.network_entity_id, my_id);

    let mut to_despawn = Vec::new();
    for (ent, ns) in engine.iterate_one_component::<NetworkStateComponent>()? {
        if ns.network_entity_id == net_state_component.network_entity_id {
            to_despawn.push(ent);
        }
    }

    for ent in to_despawn {
        engine.remove_entity_default_scene(ent)?;
        println!("[DESPAWN] Deleted entity {:?}", ent);
    }

    Ok(())
}
