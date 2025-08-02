use pill_engine::game::*;
use pill_engine::TransformComponent;

use rand::Rng;

#[cfg(feature = "net")]
use pill_engine::{NetState, NetStats, NetworkStateComponent, NetEntityState};

// ----- CONSTANTS -----

// Move speed in world units per second
const PILL_MOVE_SPEED: f32 = 3.0;
const UPDATE_FREQ_HZ: f32 = 24.0;
const UPDATE_FREQ_SEC: f32 = 1.0 / UPDATE_FREQ_HZ;

//const REMOTE_SERVER_ADDR: &str = "145.223.100.1";
const REMOTE_SERVER_ADDR: &str = "127.0.0.1";
const REMOTE_SERVER_PORT: u16 = 5000;

// TODO: temporarily add the time accumulator component
pub struct TimeAccumulationComponent {
    pub accumulator: f32,
}

impl GlobalComponent for TimeAccumulationComponent { }
impl PillTypeMapKey for TimeAccumulationComponent {
    type Storage = GlobalComponentStorage<Self>;
}

// Define custom component
pub struct PillComponent { }

impl Component for PillComponent { }

impl PillTypeMapKey for PillComponent {
    type Storage = ComponentStorage<Self>;
}

// Game
pub struct Game { }

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
        //engine.add_system("PillRotation", pill_rotation_system)?;
        engine.add_system("PillMovement", pill_movement_system)?;

        //engine.add_system("SendOwnTransform", send_own_tr_system)?;

        // Add meshes
        let pill_mesh = Mesh::new("Pill", "models/Pill.obj".into());
        let pill_mesh_handle = engine.add_resource(pill_mesh)?;

        // Add textures
        let pill_color_texture = Texture::new("PillColor", TextureType::Color, ResourceLoadType::Path("textures/PillColor.png".into()));
        let pill_color_texture_handle = engine.add_resource::<Texture>(pill_color_texture)?;
        let pill_normal_texture = Texture::new("PillNormal", TextureType::Normal, ResourceLoadType::Path("textures/PillNormal.png".into()));
        let pill_normal_texture_handle = engine.add_resource::<Texture>(pill_normal_texture)?;

        // Add materials
        let mut pill_material = Material::new("Pill");
        pill_material.set_texture("Color", pill_color_texture_handle)?;
        pill_material.set_texture("Normal", pill_normal_texture_handle)?;
        pill_material.set_color("Tint", Color::new( 1.0, 1.0, 1.0))?;
        pill_material.set_scalar("Specularity", 0.5)?;
        let pill_material_handle = engine.add_resource::<Material>(pill_material)?;

        // Create camera entity
        let camera = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new(0.0,0.0, 8.0))
            .rotation(Vector3f::new(0.0,0.0,-20.0))
            .build();
        engine.add_component_to_entity(active_scene, camera, transform_component)?;
        let camera_component = CameraComponent::builder().enabled(true).build();
        engine.add_component_to_entity(active_scene, camera, camera_component)?;

        // Create pill entity
        let pill = engine.create_entity(active_scene)?;
        let transform_component = TransformComponent::builder()
            .position(Vector3f::new( rand::thread_rng().gen_range(-2.0..=2.0), 0.0, 0.0))
            .rotation(Vector3f::new(-210.0,0.0,0.0))
            .build();
        engine.add_component_to_entity(active_scene, pill, transform_component.clone())?;
        let mesh_rendering_component = MeshRenderingComponent::builder()
            .mesh(&pill_mesh_handle)
            .material(&pill_material_handle)
            .build();
        engine.add_component_to_entity(active_scene, pill, mesh_rendering_component)?;
        engine.add_component_to_entity(active_scene, pill, PillComponent {})?;

        #[cfg(feature = "net")]
        {
            engine.add_global_component(NetStats::new())?;
            let client_id = rand::thread_rng().gen_range(1..=10_000_000);
            let server_addr = format!("{REMOTE_SERVER_ADDR}:{REMOTE_SERVER_PORT}");
            engine.add_global_component(NetState::new_client(&server_addr, client_id)?)?;

            log::info!("Client will connect to {server_addr} with ID {client_id}");

            engine.add_system("NetHUD", net_hud_system)?;
            // Add the network component marker
            engine.add_component_to_entity(active_scene, pill, NetworkStateComponent { owner_id: client_id, state: NetEntityState::Spawn, transform: Some(transform_component) })?;
            println!("Pill entity created");
        }

        //engine.add_global_component(TimeAccumulationComponent { accumulator: 0.0 })?;

        //{
        //    let state = engine.get_global_component_mut::<NetState>()?;
        //    state.entity_by_client.insert(state.my_id, pill);
        //}

        //let mut packets = Vec::new();
        //for (_, transform, _) in engine.iterate_two_components::<TransformComponent, PillComponent>()? {
        //    packets.push(TrPacket::from(transform));
        //}
        //let state = engine.get_global_component_mut::<NetState>()?;
        //if let NetSide::Client(net) = &mut state.side {
        //    cli_send(net, &Msg::Join {
        //        client_id: state.my_id,
        //        tr: Some(packets[0]),
        //    })?;
        //    log::info!("Cli ▸ JOIN sent  cid={} pkt={:?}", state.my_id, packets[0]);
        //}

        Ok(())
    }
}

/*
fn pill_rotation_system(engine: &mut Engine) -> Result<()> {
    let delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input_component = engine.get_global_component_mut::<InputComponent>()?;

    // Rotate pill if spacebar is not pressed
    if !input_component.get_key_pressed(KeyboardKey::Space) {
        for (_, transform_component, _) in engine.iterate_two_components_mut::<TransformComponent, PillComponent>()? {
            transform_component.rotation += Vector3f::new(0.0,1.0,0.0) * 100.0 * delta_time;
        }
    }

    Ok(())
}
*/

fn net_hud_system(engine: &mut Engine) -> Result<()> {
    let stats = match engine.get_global_component::<NetStats>() {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };

    //log::info!("Net counter = {0}", stats.last_counter);
    Ok(())
}

//fn send_own_tr_system(engine: &mut Engine) -> Result<()> {
//	let (my_id, my_ent) = {
//		let state = engine.get_global_component::<NetState>()?;
//        //println!("Trying to send own transform, my_id={}", state.my_id);
//		match state.entity_by_client.get(&state.my_id) {
//			Some(&e) => (state.my_id, e),
//			None      => { println!("Early exit no such id, size {}", state.entity_by_client.len()); return Ok(()) },               // not spawned yet
//        }
//    };
//
//    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
//    {
//        let mut timer = engine.get_global_component_mut::<TimeAccumulationComponent>()?;
//        timer.accumulator += dt;
//        if timer.accumulator < UPDATE_FREQ_SEC { // TODO: tweak it
//            return Ok(()); // not enough time passed
//        }
//        timer.accumulator = 0.0; // reset
//    }
//
//    // find our transform
//    let tr_pkt = engine.iterate_one_component::<TransformComponent>()?
//        .find(|(eh, _)| *eh == my_ent).map(|(_, t)| TrPacket::from(t));
//
//    //println!("Continuing with sending");
//	if let Some(pkt) = tr_pkt {
//        let state = engine.get_global_component_mut::<NetState>()?;
//        if let NetSide::Client(net) = &mut state.side {
//            cli_send(net, &Msg::Tr {
//                client_id: my_id,
//                tr:        pkt,
//            })?;
//            //log::info!("Cli ▸ TR sent  cid={my_id} pkt={:?}", pkt);
//        }
//    }
//    Ok(())
//}

fn pill_movement_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
    let input = engine.get_global_component_mut::<InputComponent>()?;

    // Build a direction vector from arrow-key input
    let mut dir = Vector3f::new(0.0, 0.0, 0.0);
    if input.get_key(KeyboardKey::ArrowUp)    { dir.z -= 1.0; }
    if input.get_key(KeyboardKey::ArrowDown)  { dir.z += 1.0; }
    if input.get_key(KeyboardKey::ArrowLeft)  { dir.x -= 1.0; }
    if input.get_key(KeyboardKey::ArrowRight) { dir.x += 1.0; }
    if input.get_key(KeyboardKey::ControlLeft)  { dir.y += 1.0; }
    if input.get_key(KeyboardKey::ShiftLeft) { dir.y -= 1.0; }

    // Move every pill entity
    if dir.x != 0.0 || dir.y != 0.0 {
        // Normalize only the XY part so diagonal speed == straight speed
        let len = (dir.x * dir.x + dir.y * dir.y).sqrt(); // 1.0 straight, √2 diagonal
        let inv = 1.0 / len;
        dir.x *= inv;
        dir.y *= inv;

        //for (_, transform, _) in engine.iterate_two_components_mut::<TransformComponent, PillComponent>()? {
        //    transform.position += dir * PILL_MOVE_SPEED * dt;
        //    println!("Direction: {:?}", dir);
        //}
    }

    Ok(())
}


