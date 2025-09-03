use anyhow::Result;
use log::debug;
use pill_core::{NetworkClient, client_update, client_get_events, server_update, server_get_events, client_send, server_broadcast, server_broadcast_except, server_send_one, client_flush, server_flush, NetworkPacket, NetworkAction, ExitNotice, is_not_ready};

use crate::ecs::components::transform_component;
use crate::engine::Engine;
use crate::ecs::{EntityHandle, TransformComponent, TimeComponent, NetworkStateComponent, NetworkEntityState, NetworkSide, NetworkManagerComponent};
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
pub enum NetworkEntityAction {
    Spawn,
    Despawn,
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityUpdate {
    pub action: NetworkEntityAction,
    pub net_state: NetworkStateComponent,
    // TODO: maybe need net_scene_id to identify the scene
    // TODO: update this as a Vec of Nettable components after Hist0r merge, inject the
    // arbiter/conflict resolver function
    pub transform: Option<TransformComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkUpdatePayload {
    pub client_id: u64,
    pub updates: Vec<EntityUpdate>,
    pub timestamp: f32,
}

#[inline(always)]
fn exp_blend(delta_time: f32) -> f32 {
        1.0 - (-LERP_RATE * delta_time).exp()           // ≈ LERP_RATE * delta_time for small delta_time
}

fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0, 1.0)
}

const LERP_RATE: f32 = 0.001;

fn run_client_interpolation(engine: &mut Engine) -> Result<()> {
    // Run the client-side interpolation for non-owned entities
    let my_id = engine.get_global_component::<NetworkManagerComponent>()?.my_id;
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
    Ok(())
}

fn run_client_interpolation_hook(engine: &mut Engine) -> Result<()> {
    // Run the client-side interpolation for non-owned entities
    let transform_interpolation_hook = {
        let global_net_state = engine.get_global_component::<NetworkManagerComponent>()?;
        global_net_state.client_interpolation_hook
    };
    if let Some(hook) = transform_interpolation_hook {
        hook(engine)?;
    } else {
        run_client_interpolation(engine)?;
    }
    Ok(())
}

fn default_despawn_hook(engine: &mut Engine, net_state: &NetworkStateComponent) -> Result<()> {
    // Default despawn hook - simply remove the entity with the given network_entity_id
    let mut to_despawn: Vec<EntityHandle> = Vec::new();
    for (handle, net_state_comp) in engine.iterate_one_component_mut::<NetworkStateComponent>()? {
        if net_state_comp.network_entity_id == net_state.network_entity_id {
            to_despawn.push(handle);
        }
    }
    for handle in to_despawn {
        engine.remove_entity_default_scene(handle)?; // TODO: maybe we want to just ghost-posess the
                                               // entity with AI?
    }
    Ok(())
}

fn pump_transport(engine: &mut Engine) -> Result<()> {
    let delta_time = {
        let frame_delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
        Duration::from_secs_f32(frame_delta_time.max(0.0))
    };

    let mut network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
    match network_manager.side {
        NetworkSide::Client(ref mut state) => {
            if let Err(e) = client_update(&mut state.client, delta_time) { if !is_not_ready(&e) { return Err(e); } }
            if let Err(e) = client_flush(&mut state.client)     { if !is_not_ready(&e) { return Err(e); } }
        }
        NetworkSide::Server(ref mut state) => {
            if let Err(e) = server_update(&mut state.server, delta_time) { if !is_not_ready(&e) { return Err(e); } }
            if let Err(e) = server_flush(&mut state.server) { if !is_not_ready(&e) { return Err(e); } }
        }
    }
    Ok(())
}

fn receive_updates(engine: &mut Engine) -> Result<Vec<NetworkUpdatePayload>> {
    let mut updates:    Vec<NetworkUpdatePayload> = Vec::new();
    let mut join_cids:  Vec<u64>                  = Vec::new();
    let mut despawn_cids: Vec<u64>                  = Vec::new();
    let state = engine.get_global_component_mut::<NetworkManagerComponent>()?;

    match &mut state.side {
        NetworkSide::Client(state) => {
            match client_get_events(&mut state.client) {
                Ok(msgs) => {
                    for msg in &msgs {
                        if msg.tag == NetworkAction::Update {
                            let pkt: NetworkUpdatePayload = bincode::deserialize(&msg.data)?;
                            debug!(
                                "[Client] ◂ received pkt from srv at time {}", pkt.timestamp
                            );
                            updates.push(pkt);
                        } else if msg.tag == NetworkAction::Exit {
                            let notice: ExitNotice = bincode::deserialize(&msg.data)
                              .unwrap_or(ExitNotice { reason: "Server exit".into(), when_ms: 0 });
                            println!("[Client] Server Exit: {} (t={})", notice.reason, notice.when_ms);
                            // TODO: implement the rest of complex system handling
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

        NetworkSide::Server(state) => {
            for (cid, msg) in server_get_events(&mut state.server)? {
                println!("[Server] ◂ received msg from cid={cid} with tag {:?}", msg.tag);

                if msg.tag == NetworkAction::Update {
                    let pkt: NetworkUpdatePayload = bincode::deserialize(&msg.data)?;
                    println!(
                        "[Server] ◂ received pkt from cid={cid} at time {}", pkt.timestamp
                    );
                    updates.push(pkt);
                } else if msg.tag == NetworkAction::Join {
                    println!("[Server] Client {cid} JOIN with cid={cid}");
                    join_cids.push(cid);
                } else if msg.tag == NetworkAction::Exit {
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
                action:    NetworkEntityAction::Spawn,
                net_state: net_state.clone(),
                transform: Some(transform.clone()),
            });
        }

        let snapshot = NetworkUpdatePayload {
            client_id: 0,   // 0 = "server"
            updates:   entity_updates,
            timestamp: engine.get_global_component::<TimeComponent>()?.time,
        };

        let state = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        if let NetworkSide::Server(state) = &mut state.side {
            server_send_one(
                &mut state.server,
                cid,   // **only** the newcomer
                &NetworkPacket {
                    tag:  NetworkAction::Update,
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
                    action:    NetworkEntityAction::Despawn,
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

        let state = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        if let NetworkSide::Server(state) = &mut state.side {
            server_broadcast(
                &mut state.server,
                &NetworkPacket {
                    tag:  NetworkAction::Update,
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
        let network_manager = engine.get_global_component::<NetworkManagerComponent>()?;
        network_manager.spawn_handlers.get(&entity_update.net_state.entity_type).copied()
    };

    if let Some(spawn_fn) = spawn_fn {
        spawn_fn(engine, &entity_update.net_state, &tr)?;
    } else {
        println!("No spawn handler for entity entity_type '{}'", entity_update.net_state.entity_type);
    }
    Ok(())
}

fn run_despawn_hooks(engine: &mut Engine, entity_update: &EntityUpdate) -> Result<()> {
    let despawn_fn = {
        let network_manager = engine.get_global_component::<NetworkManagerComponent>()?;
        network_manager.despawn_handlers.get(&entity_update.net_state.entity_type).copied()
    };

    if let Some(despawn_fn) = despawn_fn {
        despawn_fn(engine, &entity_update.net_state)?;
    } else {
        println!("No despawn handler for entity entity_type '{}' Running the default despawn hook", entity_update.net_state.entity_type);
        default_despawn_hook(engine, &entity_update.net_state)?;
    }
    Ok(())
}

fn send_client_updates(engine: &mut Engine) -> Result<()> {
    // Iterate over all entities living on the client that have Networking component and send them to the server
    {
        let mut updates: Vec<EntityUpdate> = Vec::new();
        let my_id = engine.get_global_component::<NetworkManagerComponent>()?.my_id;
        for (_, transform, net_state)
            in engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()? {
            if net_state.state == NetworkEntityState::Spawn {
                // send the entity's state to the server only if this client is the owner of the
                // entity
                if net_state.owner_id != my_id {
                    continue; // skip entities not owned by this client
                }
               println!("▸ Queuing for spawn: new entity for nid={:?} from cid={my_id}", net_state.network_entity_id);
                let update = EntityUpdate {
                    action: NetworkEntityAction::Spawn,
                    net_state: net_state.clone(),
                    transform: Some(transform.clone()),
                };
                updates.push(update);
            }
            net_state.state = NetworkEntityState::Alive; // mark the entity as alive
        }
        let payload = NetworkUpdatePayload {
            client_id: my_id,
            updates,
            timestamp: engine.get_global_component::<TimeComponent>()?.time,
        };
        if let NetworkSide::Client(state) = &mut engine.get_global_component_mut::<NetworkManagerComponent>()?.side {
            let msg = NetworkPacket {
                tag: NetworkAction::Update,
                data: bincode::serialize(&payload)?,
            };
			match client_send(&mut state.client, &msg) {
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
    let delta_time = engine.get_global_component::<TimeComponent>().map(|c| c.delta_time).unwrap_or(0.0);
    let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>().unwrap();
    network_manager.accumulator += delta_time;
    if network_manager.accumulator >= network_manager.timeout {
        network_manager.accumulator = 0.0;
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
                        NetworkEntityAction::Spawn => {
                            println!("Spawn ◂ from cid={}  nid={:?}", update.client_id, entity_update.net_state.network_entity_id);
                            run_spawn_hooks(engine, entity_update)?;
                        },
                        NetworkEntityAction::Despawn => {
                            run_despawn_hooks(engine, entity_update)?;
                        },
                        NetworkEntityAction::Update => {
                            // Handle updating the entity's transform
                            // TODO: we will handle multiple entities/components - allow for
                            // injection?
                            if let Some(tr) = &entity_update.transform {
                                for (_, transform, net_state)
                                    in engine.iterate_two_components_mut::<TransformComponent,
                                                                           NetworkStateComponent>()? {
                                    if entity_update.net_state.network_entity_id == net_state.network_entity_id {
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

                let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
                if let NetworkSide::Server(state) = &mut network_manager.side {
                    // Broadcast the update to everyone except the sender
                    server_broadcast_except(&mut state.server, update.client_id, &NetworkPacket {
                        tag: NetworkAction::Update,
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

    run_client_interpolation_hook(engine)?;

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
                        NetworkEntityAction::Spawn => {
                            println!("Spawn ◂ from cid={}  nid={:?}", update.client_id, entity_update.net_state.network_entity_id);
                            run_spawn_hooks(engine, entity_update)?;
                        },
                        NetworkEntityAction::Despawn => {
                            run_despawn_hooks(engine, entity_update)?;
                        },
                        NetworkEntityAction::Update => {
                            // Handle updating the entity's transform
                            if let Some(tr) = &entity_update.transform {
                                for (_, transform, net_state)
                                    in engine.iterate_two_components_mut::<TransformComponent,
                                                                           NetworkStateComponent>()? {
                                    if entity_update.net_state.network_entity_id == net_state.network_entity_id {
                                        net_state.transform = Some(tr.clone());
                                        debug!("▸ Updating entity with nid={:?} for cid={} net_state={:?} with transform {:?}",
                                                 net_state.network_entity_id, update.client_id, net_state, tr);

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
