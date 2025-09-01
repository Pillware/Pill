use anyhow::Result;
use log::debug;
use pill_core::{NetClient, cli_update, cli_get_events, srv_update, srv_get_events, cli_send, srv_broadcast, srv_broadcast_except, srv_send_one, cli_flush, srv_flush, WireMsg, WireTag};

use crate::ecs::components::transform_component;
use crate::engine::Engine;
use crate::ecs::{EntityHandle, TransformComponent, TimeComponent, NetworkStateComponent, NetEntityState, NetSide, GlobalNetState};
use pill_core::{Vector3f};

#[cfg(not(feature = "headless"))]
use crate::{
    ecs::{MeshRenderingComponent},
    resources::{Material, MaterialHandle, Mesh, MeshHandle},
};

use serde::{Deserialize, Serialize};
use std::time::Duration;
use bincode;
use rand::{rng, Rng, SeedableRng};


#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetEntityAction {
    Spawn,
    Despawn,
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityUpdate {
    pub action: NetEntityAction,
    pub net_state: NetworkStateComponent,
    // TODO: maybe need net_scene_id to identify the scene
    pub transform: Option<TransformComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkUpdatePayload {
    pub client_id: u64,
    pub updates: Vec<EntityUpdate>,
    pub timestamp: f32,
}

#[inline(always)]
fn exp_blend(dt: f32) -> f32 {
        1.0 - (-LERP_RATE * dt).exp()           // ≈ LERP_RATE * dt for small dt
}

fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0, 1.0)
}

const LERP_RATE: f32 = 0.001;

fn run_client_interpolation(engine: &mut Engine) -> Result<()> {
    // Run the client-side interpolation for non-owned entities TODO: this can be injected by the
    // game later
    {
        let my_id = engine.get_global_component::<GlobalNetState>()?.my_id;
        let delta_time = engine.frame_delta_time;
        // Peform interpolation for the components that have a transform and are not owned by the
        // client
        for (_, transform, net_state) in engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()? {
            if let Some(tr) = &net_state.transform  {
                if net_state.owner_id != my_id {
                    //println!("interpolating: source {:?} dest {:?} delta_time={}", transform.position, tr.position, delta_time);
                    // Interpolate the transform based on the current time and the last known state
                                       //let t         = blend(delta_time, LERP_HALFLIFE);
                    let t = exp_blend(delta_time);
                    transform.set_position(lerp_vec3(transform.position, tr.position, t));
                    transform.set_rotation(lerp_vec3(transform.rotation, tr.rotation, t));
                    // TODO: scale?
                }
            }
        }
    }
    Ok(())
}

fn default_despawn_hook(engine: &mut Engine, net_state: &NetworkStateComponent) -> Result<()> {
    // Default despawn hook - simply remove the entity with the given net_entity_id
    let mut to_despawn: Vec<EntityHandle> = Vec::new();
    for (handle, net_state_comp) in engine.iterate_one_component_mut::<NetworkStateComponent>()? {
        if net_state_comp.net_entity_id == net_state.net_entity_id {
            to_despawn.push(handle);
        }
    }
    for handle in to_despawn {
        engine.remove_entity_default_scene(handle)?; // TODO: maybe we want to just ghost-posess the
                                               // entity with AI?
    }
    Ok(())
}

fn is_not_ready(err: &anyhow::Error) -> bool {
    let s = err.to_string().to_lowercase();
    s.contains("disconnected")
        || s.contains("timed out")
        || s.contains("timeout")
        || s.contains("not connected")
        || s.contains("connecting")
}

fn pump_transport(engine: &mut Engine) -> Result<()> {
    let dt = {
        let frame_dt = engine.get_global_component::<TimeComponent>()?.delta_time;
        Duration::from_secs_f32(frame_dt.max(0.0))
    };

    let mut state = engine.get_global_component_mut::<GlobalNetState>()?;
    match &mut state.side {
        NetSide::Client(net) => {
            if let Err(e) = cli_update(net, dt) { if !is_not_ready(&e) { return Err(e); } }
            if let Err(e) = cli_flush(net)     { if !is_not_ready(&e) { return Err(e); } }
        }
        NetSide::Server(net) => {
            if let Err(e) = srv_update(net, dt) { if !is_not_ready(&e) { return Err(e); } }
            if let Err(e) = srv_flush(net) { if !is_not_ready(&e) { return Err(e); } }
        }
    }
    Ok(())
}

fn receive_updates(engine: &mut Engine) -> Result<Vec<NetworkUpdatePayload>> {
    let mut updates:    Vec<NetworkUpdatePayload> = Vec::new();
    let mut join_cids:  Vec<u64>                  = Vec::new();
    let mut despawn_cids: Vec<u64>                  = Vec::new();
    let state = engine.get_global_component_mut::<GlobalNetState>()?;

    match &mut state.side {
        NetSide::Client(net) => {
            match cli_get_events(net) {
                Ok(msgs) => {
                    for msg in &msgs {
                        if msg.tag == WireTag::Update {
                            let pkt: NetworkUpdatePayload = bincode::deserialize(&msg.data)?;
                            debug!(
                                "[Client] ◂ received pkt from srv at time {}", pkt.timestamp
                            );
                            updates.push(pkt);
                        }
                    }
                }
                Err(e) if is_not_ready(&e) => {
                    println!("[Client] ▸ not connected yet – update skipped");
                    return Ok(updates);
                }
                Err(e) => return Err(e),
            }
        }

        NetSide::Server(net) => {
            for (cid, msg) in srv_get_events(net)? {
                println!("[Server] ◂ received msg from cid={cid} with tag {:?}", msg.tag);

                if msg.tag == WireTag::Update {
                    let pkt: NetworkUpdatePayload = bincode::deserialize(&msg.data)?;
                    println!(
                        "[Server] ◂ received pkt from cid={cid} at time {}", pkt.timestamp
                    );
                    updates.push(pkt);
                } else if msg.tag == WireTag::Join {
                    println!("[Server] Client {cid} JOIN with cid={cid}");
                    join_cids.push(cid);
                } else if msg.tag == WireTag::Exit {
                    println!("[Server] Client {cid} EXIT");
                    // TODO: please review - we might not want to despawn all or just ghost-posess
                    // the player with AI in some games - or maybe we want to keep the entities
                    // For now this needs to despawn all entities owned by this client
                    despawn_cids.push(cid);
                }
            }
        }
    }

    if !join_cids.is_empty() {
        send_existing_entities(engine, join_cids)?;
    }

    if !despawn_cids.is_empty() {
        despawn_entities(engine, despawn_cids)?;
    }

	Ok(updates)
}

fn send_existing_entities(engine: &mut Engine, join_cids: Vec<u64>) -> Result<()> {
    // Inform every new client about all existing entities on the server
    for cid in join_cids {
        let mut entity_updates: Vec<EntityUpdate> = Vec::new();
        for (_, transform, net_state) in
            engine.iterate_two_components_mut::<TransformComponent,
                                                NetworkStateComponent>()?
        {
            entity_updates.push(EntityUpdate {
                action:    NetEntityAction::Spawn,
                net_state: net_state.clone(),
                transform: Some(transform.clone()),
            });
        }

        let snapshot = NetworkUpdatePayload {
            client_id: 0,   // 0 = "server"
            updates:   entity_updates,
            timestamp: engine.get_global_component::<TimeComponent>()?.time,
        };

        let state = engine.get_global_component_mut::<GlobalNetState>()?;
        if let NetSide::Server(net) = &mut state.side {
            srv_send_one(
                net,
                cid,   // **only** the newcomer
                &WireMsg {
                    tag:  WireTag::Update,
                    data: bincode::serialize(&snapshot)?,
                },
            )?;
        }

        println!(
            "[Server] → sent {} existing entities to cid={cid}",
            snapshot.updates.len()
        );
    }
    Ok(())
}

fn despawn_entities(engine: &mut Engine, despawn_cids: Vec<u64>) -> Result<()> {
    // Despawn all entities owned by the clients that disconnected
    let mut entity_updates: Vec<EntityUpdate> = Vec::new();
    for cid in despawn_cids {
        for (_, net_state) in
            engine.iterate_one_component_mut::<NetworkStateComponent>()?
        {
            if net_state.owner_id == cid {
                entity_updates.push(EntityUpdate {
                    action:    NetEntityAction::Despawn,
                    net_state: net_state.clone(),
                    transform: None,
                });
            }
        }
    }

    if !entity_updates.is_empty() {
        let snapshot = NetworkUpdatePayload {
            client_id: 0,   // 0 = "server"
            updates:   entity_updates,
            timestamp: engine.get_global_component::<TimeComponent>()?.time,
        };

        let state = engine.get_global_component_mut::<GlobalNetState>()?;
        if let NetSide::Server(net) = &mut state.side {
            srv_broadcast(
                net,
                &WireMsg {
                    tag:  WireTag::Update,
                    data: bincode::serialize(&snapshot)?,
                },
            )?;
        }

        println!(
            "[Server] → despawned {} entities from disconnected clients",
            snapshot.updates.len()
        );
    }
    Ok(())
}

fn run_spawn_hooks(engine: &mut Engine, entity_update: &EntityUpdate) -> Result<()> {
    let tr = entity_update.transform
                          .clone()
                          .unwrap_or_else(TransformComponent::default);
    let spawn_fn = {
        let global_net_state = engine.get_global_component::<GlobalNetState>()?;
        global_net_state.spawn_handlers.get(&entity_update.net_state.kind).copied()
    };

    if let Some(spawn_fn) = spawn_fn {
        spawn_fn(engine, &entity_update.net_state, &tr)?;
    } else {
        println!("No spawn handler for entity kind '{}'", entity_update.net_state.kind);
    }
    Ok(())
}

fn run_despawn_hooks(engine: &mut Engine, entity_update: &EntityUpdate) -> Result<()> {
    let despawn_fn = {
        let global_net_state = engine.get_global_component::<GlobalNetState>()?;
        global_net_state.despawn_handlers.get(&entity_update.net_state.kind).copied()
    };

    if let Some(despawn_fn) = despawn_fn {
        despawn_fn(engine, &entity_update.net_state)?;
    } else {
        println!("No despawn handler for entity kind '{}' Running the default despawn hook", entity_update.net_state.kind);
       default_despawn_hook(engine, &entity_update.net_state)?;
    }
    Ok(())
}

fn send_client_updates(engine: &mut Engine) -> Result<()> {
    // Iterate over all entities living on the client that have Networking component and send them to the server
    {
        let mut updates: Vec<EntityUpdate> = Vec::new();
        let my_id = engine.get_global_component::<GlobalNetState>()?.my_id;
        for (_, transform, net_state)
            in engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()? {
            if net_state.state == NetEntityState::Spawn {
                // send the entity's state to the server only if this client is the owner of the
                // entity
                if net_state.owner_id != my_id {
                    continue; // skip entities not owned by this client
                }
               println!("▸ Queuing for spawn: new entity for nid={:?} from cid={my_id}", net_state.net_entity_id);
                let update = EntityUpdate {
                    action: NetEntityAction::Spawn,
                    net_state: net_state.clone(),
                    transform: Some(transform.clone()),
                };
                updates.push(update);
            }
            net_state.state = NetEntityState::Alive; // mark the entity as alive
        }
        let payload = NetworkUpdatePayload {
            client_id: my_id,
            updates,
            timestamp: engine.get_global_component::<TimeComponent>()?.time,
        };
        if let NetSide::Client(net) = &mut engine.get_global_component_mut::<GlobalNetState>()?.side {
            let msg = WireMsg {
                tag: WireTag::Update,
                data: bincode::serialize(&payload)?,
            };
			match cli_send(net, &msg) {
				Ok(_) => {}
				Err(e) if is_not_ready(&e) => {
					println!("[Client] ▸ not connected yet – send skipped");
					return Ok(());
				}
				Err(e) => return Err(e),
			}
        }
    }
    Ok(())
}

fn timeout_elapsed(engine: &mut Engine) -> bool {
    let dt = engine.get_global_component::<TimeComponent>().map(|c| c.delta_time).unwrap_or(0.0);
    let state = engine.get_global_component_mut::<GlobalNetState>().unwrap();
    state.accumulator += dt;
    if state.accumulator >= state.timeout {
        state.accumulator = 0.0;
        return true;
    }
    false
}

pub fn networking_system_server(engine: &mut Engine) -> Result<()> {
    // Run transport pump every tick to avoid timeouts
    pump_transport(engine)?;

    if !timeout_elapsed(engine) {
        // Not enough time has passed to process the next network update
        return Ok(());
    }

    // Step 1: Receive network updates from clients and broadcast them to all clients
    match receive_updates(engine) {
        Ok(updates) => {
            println!("Got {} updates from clients", updates.len());
            // Step 2: Process the updates and apply them to the entities in the server's world immediately
            for update in &updates {
                // handle each NetworkUpdatePayload from a client
                for entity_update in &update.updates {
                    match entity_update.action {
                        NetEntityAction::Spawn => {
                            println!("Spawn ◂ from cid={}  nid={:?}", update.client_id, entity_update.net_state.net_entity_id);
                            run_spawn_hooks(engine, entity_update)?;
                        },
                        NetEntityAction::Despawn => {
                            run_despawn_hooks(engine, entity_update)?;
                        },
                        NetEntityAction::Update => {
                            // Handle updating the entity's transform
                            // TODO: we will handle multiple entities/components - allow for
                            // injection?
                            if let Some(tr) = &entity_update.transform {
                                for (_, transform, net_state)
                                    in engine.iterate_two_components_mut::<TransformComponent,
                                                                           NetworkStateComponent>()? {
                                    if entity_update.net_state.net_entity_id == net_state.net_entity_id {
                                        net_state.transform = Some(tr.clone());

                                        // authoritative change on the server
                                        *transform = *tr;
                                        break;
                                    }
                                }
                            }
                        },
                    }
                }

                let state = engine.get_global_component_mut::<GlobalNetState>()?;
                if let NetSide::Server(net) = &mut state.side {
                    // Broadcast the update to everyone except the sender
                    srv_broadcast_except(net, update.client_id, &WireMsg {
                        tag: WireTag::Update,
                        data: bincode::serialize(&update)?,
                    })?;
                };
            }
        },
        Err(e) => {
            println!("Failed to receive or broadcast network updates: {}", e);
            return Err(e);
        }
    }
    Ok(())
}

pub fn networking_system_client(engine: &mut Engine) -> Result<()> {
    // Run transport pump every tick to avoid timeouts
    pump_transport(engine)?;

    run_client_interpolation(engine)?;

    if !timeout_elapsed(engine) {
        // Not enough time has passed to process the next network update
        return Ok(());
    }

    send_client_updates(engine)?;

    match receive_updates(engine) {
        Ok(updates) => {
            // there is just one Update in the vector
            for update in &updates {
                for entity_update in &update.updates {
                    match entity_update.action {
                        NetEntityAction::Spawn => {
                            println!("Spawn ◂ from cid={}  nid={:?}", update.client_id, entity_update.net_state.net_entity_id);
                            run_spawn_hooks(engine, entity_update)?;
                        },
                        NetEntityAction::Despawn => {
                            run_despawn_hooks(engine, entity_update)?;
                        },
                        NetEntityAction::Update => {
                            // Handle updating the entity's transform
                            if let Some(tr) = &entity_update.transform {
                                for (_, transform, net_state)
                                    in engine.iterate_two_components_mut::<TransformComponent,
                                                                           NetworkStateComponent>()? {
                                    if entity_update.net_state.net_entity_id == net_state.net_entity_id {
                                        net_state.transform = Some(tr.clone());
                                        debug!("▸ Updating entity with nid={:?} for cid={} net_state={:?} with transform {:?}",
                                                 net_state.net_entity_id, update.client_id, net_state, tr);

                                        // authoritative change on the server
                                        //*transform = *tr;
                                        break;
                                    }
                                }
                            }
                        },
                    }
                }
            }
        },
        Err(e) => {
            println!("Failed to receive network updates: {}", e);
            return Err(e);
        }
    }

    Ok(())
}
