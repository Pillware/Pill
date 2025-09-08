//! # Networking System
//!
//! This module handles replication, connection lifecycle, and conflict arbitration.
//! The diagrams below are rendered from `docs/uml/*.puml` during the docs build.

#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/uml_out/connection.svg"))]
use anyhow::Result;
use log::debug;
use pill_core::{NetworkClient, client_connect, client_disconnect, client_update, client_get_events, server_update,
    server_get_events, client_send, server_broadcast, server_broadcast_except, server_send_one, server_disconnect_client,
    client_flush, server_flush, NetworkPacket, NetworkAction, ExitNotice, is_not_ready};

use crate::ecs::components::network_manager_component::OfflinePolicy;
use crate::ecs::components::transform_component;
use crate::engine::Engine;
use crate::ecs::{EntityHandle, TransformComponent, TimeComponent, NetworkStateComponent, NetworkEntityState,
    NetworkSide, NetworkManagerComponent, ConnectionState, ClientState};
use pill_core::{Vector3f};

use serde::{Deserialize, Serialize};
use std::time::Duration;
use std::collections::HashSet;
use bincode;
use rand::{rng, Rng, SeedableRng};
use cgmath::InnerSpace;


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

// ------------- CLIENT HOOKS -------------

#[inline(always)]
fn exp_blend(delta_time: f32) -> f32 {
        1.0 - (-LERP_RATE * delta_time).exp()           // ≈ LERP_RATE * delta_time for small delta_time
}

fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0, 1.0)
}

fn changed_enough(current: &TransformComponent, previous: &TransformComponent) -> bool {
    // Use squared thresholds (0.1 units in postion)
    let pos_diff = (current.position - previous.position).magnitude2();
    let rot_diff = (current.rotation - previous.rotation).magnitude2();
    pos_diff > 0.01 || rot_diff > 0.01
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

// ------------- NETWORKING HELPERS -------------

fn mark_disconnected(engine: &mut Engine, reason: &str) -> Result<()> {
    let time = engine.get_global_component::<TimeComponent>()?.time;
    let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
    if network_manager.is_client() {
        if let Some(client) = network_manager.client_mut() {
            if client.connection_state == ConnectionState::Disconnected {
                // already disconnected
                return Ok(());
            }
            println!("[Client] Disconnected: {reason}");
            client.connection_state = ConnectionState::Disconnected;
            client.want_reconnect = true;
            client.backoff_ms = (client.backoff_ms.saturating_mul(2)).min(2_000);
            client.next_try_s = time + (client.backoff_ms as f32 / 1000.0);
        }
    }
    Ok(())
}

fn mark_connected(engine: &mut Engine) -> Result<()> {
    let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
    if network_manager.is_client() {
        if let Some(client) = network_manager.client_mut() {
            if client.connection_state == ConnectionState::Connected {
                // already connected
                return Ok(());
            }
            println!("[Client] Connected to server at {}", client.server_address);
            client.connection_state = ConnectionState::Connected;
            client.want_reconnect = false;
            client.backoff_ms = 500;
            client.next_try_s = 0.0;
        }
    }
    Ok(())
}


fn client_try_reconnect(engine: &mut Engine) -> Result<()> {
    let time = engine.get_global_component::<TimeComponent>()?.time;
    let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
    let my_id = network_manager.my_id;

    if let Some(client) = network_manager.client_mut() {
        //println!("[Client] connection_state={:?} want_reconnect={} next_try_s={} time={}", client.connection_state, client.want_reconnect, client.next_try_s, time);
        if !(client.connection_state == ConnectionState::Disconnected && client.want_reconnect) { return Ok(()); }
        if time < client.next_try_s { return Ok(()); }

        println!("[Client] Attempting to reconnect to server at {}...", client.server_address);
        if let Ok(new_client) = client_connect(&client.server_address, my_id) {
            println!("[Client] Reconnected to server at {}", client.server_address);
            network_manager.side = NetworkSide::Client(ClientState {
                net: new_client,
                server_address: client.server_address.clone(),
                connection_state: ConnectionState::Connecting,
                want_reconnect: false,
                backoff_ms: 500,
                next_try_s: 0.0,
                world_epoch: client.world_epoch,
                connection_seq: client.connection_seq + 1,
            });
        } else {
            return Ok(());
        }
    }

    // Re-announce entities if server despawned them during our absence
    for (_, net_state) in engine.iterate_one_component_mut::<NetworkStateComponent>()? {
        if net_state.owner_id == my_id {
            net_state.state = NetworkEntityState::Spawn;
        }
    }

    Ok(())
}

/// Call this when the client wants to go offline (e.g. on user request)
pub fn client_go_offline(engine: &mut Engine, reason: &str) -> Result<()> {
    let time = engine.get_global_component::<TimeComponent>()?.time;

    {
        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        if let Some(client) = network_manager.client_mut() {
            let msg = NetworkPacket {
                tag: NetworkAction::Exit,
                data: bincode::serialize(&ExitNotice {
                    reason: reason.into(),
                    when_ms: (time * 1000.0) as u64,
                })?,
            };
            let _ = client_send(&mut client.net, &msg);
        }
    }

    mark_disconnected(engine, reason)?;

    // close the underlying connection
    {
        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        if let Some(client) = network_manager.client_mut() {
            client_disconnect(&mut client.net)?;
        }
    }

    Ok(())
}

fn pump_transport(engine: &mut Engine) -> Result<()> {
    let delta_time = {
        let frame_delta_time = engine.get_global_component::<TimeComponent>()?.delta_time;
        Duration::from_secs_f32(frame_delta_time.max(0.0))
    };

    let mut disconnect_reason: Option<String> = None;

    {
        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        match &mut network_manager.side {
            NetworkSide::Client(ref mut state) => {
                if let Err(e) = client_update(&mut state.net, delta_time) { if is_not_ready(&e) { disconnect_reason = Some(e.to_string()); } else { return Err(e); }}
                if let Err(e) = client_flush(&mut state.net)     { if is_not_ready(&e) { disconnect_reason.get_or_insert(e.to_string()); } else { return Err(e); }}
            }
            NetworkSide::Server(ref mut state) => {
                if let Err(e) = server_update(&mut state.net, delta_time) { if !is_not_ready(&e) { return Err(e); } }
                if let Err(e) = server_flush(&mut state.net) { if !is_not_ready(&e) { return Err(e); } }
            }
        }
    }

    if let Some(reason) = disconnect_reason {
        mark_disconnected(engine, &reason)?;
    }
    Ok(())
}

fn receive_updates(engine: &mut Engine) -> Result<Vec<NetworkUpdatePayload>> {
    let mut updates:    Vec<NetworkUpdatePayload> = Vec::new();
    let mut join_cids:  Vec<u64>                  = Vec::new();
    let mut exit_cids: Vec<u64>                  = Vec::new();
    {
        let state = engine.get_global_component_mut::<NetworkManagerComponent>()?;

        match &mut state.side {
            NetworkSide::Client(state) => {
                match client_get_events(&mut state.net) {
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
                                mark_disconnected(engine, &notice.reason)?;
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
                for (cid, msg) in server_get_events(&mut state.net)? {
                    println!("[Server] ◂ received msg from cid={cid} with tag {:?}", msg.tag);
                    if msg.tag == NetworkAction::Update {
                        let pkt: NetworkUpdatePayload = bincode::deserialize(&msg.data)?;
                        println!(
                            "[Server] ◂ received pkt from cid={cid} at time {}", pkt.timestamp
                        );

                        // Drop the client's connection if caught cheating
                        if pkt.client_id != cid {
                            println!("[Server] Client {cid} sent a message with mismatched client_id {}, dropping connection", pkt .client_id);
                            server_disconnect_client(&mut state.net, cid)?;
                            continue;
                        }

                        updates.push(pkt);
                    } else if msg.tag == NetworkAction::Join {
                        println!("[Server] Client {cid} JOIN with cid={cid}");
                        join_cids.push(cid);
                    } else if msg.tag == NetworkAction::Exit {
                        println!("[Server] Client {cid} EXIT");
                        exit_cids.push(cid);
                    }
                }
            }
        }
    }

    // Server only functionality
    if !join_cids.is_empty() {
        send_existing_entities(engine, join_cids)?;
    }

    if !exit_cids.is_empty() {
        handle_exit_policy(engine, exit_cids)?;
    }

	Ok(updates)
}

fn send_existing_entities(engine: &mut Engine, join_cids: Vec<u64>) -> Result<()> {
    // Send it only if we are fully connected
    let connected = {
        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        network_manager.client_mut().map(|c| c.connection_state == ConnectionState::Connected).unwrap_or(true)
    };
    if !connected {
        return Ok(());
    }
    // Inform every new client about all existing entities on the server
    for cid in join_cids {
        let mut entity_updates: Vec<EntityUpdate> = Vec::new();
        for (_, transform, net_state) in
            engine.iterate_two_components_mut::<TransformComponent,
                                                NetworkStateComponent>()?
        {
            if net_state.owner_id == cid {
                // don't reannounce entities owned by the client itself
                continue;
            }
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

        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        if let NetworkSide::Server(state) = &mut network_manager.side {
            server_send_one(
                &mut state.net,
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

fn handle_exit_policy(engine: &mut Engine, exit_cids: Vec<u64>) -> Result<()> {
    let state = engine.get_global_component_mut::<NetworkManagerComponent>()?;
    let policy = state.server_mut().map(|s| s.offline_policy).unwrap_or(OfflinePolicy::Despawn);
    match policy {
        OfflinePolicy::Despawn => {
            despawn_entities(engine, exit_cids)?;
        },
        OfflinePolicy::Freeze => {
            println!("Freezing entities from disconnected clients is not implemented yet");
        },
        OfflinePolicy::AI => {
            println!("AI takeover for entities from disconnected clients is not implemented yet");
        }
    }
    Ok(())
}

fn despawn_entities(engine: &mut Engine, despawn_cids: Vec<u64>) -> Result<()> {
    let set: HashSet<u64> = despawn_cids.iter().copied().collect();

    // Despawn all entities owned by the clients that disconnected
    let entity_updates: Vec<EntityUpdate> = {
        let mut entity_updates: Vec<EntityUpdate> = Vec::new();
        for (_, net_state) in
            engine.iterate_one_component_mut::<NetworkStateComponent>()?
        {
            if set.contains(&net_state.owner_id) {
                entity_updates.push(EntityUpdate {
                    action:    NetworkEntityAction::Despawn,
                    net_state: net_state.clone(),
                    transform: None,
                });
            }
        }
        entity_updates
    };

    if !entity_updates.is_empty() {
        let snapshot = NetworkUpdatePayload {
            client_id: 0,   // 0 = "server"
            updates:   entity_updates.clone(),
            timestamp: engine.get_global_component::<TimeComponent>()?.time,
        };

        let network_manager = engine.get_global_component_mut::<NetworkManagerComponent>()?;
        if let NetworkSide::Server(state) = &mut network_manager.side {
            server_broadcast(
                &mut state.net,
                &NetworkPacket {
                    tag:  NetworkAction::Update,
                    data: bincode::serialize(&snapshot)?,
                },
            )?;
        }
    }

    // Server needs to despawn them locally as well
    for entity_update in &entity_updates {
        run_despawn_hooks(engine, entity_update)?;
    }

    println!(
        "[Server] → despawned {} entities from disconnected clients",
        entity_updates.len()
    );
    Ok(())
}

fn run_update_for_existing_entity(engine: &mut Engine, entity_update: &EntityUpdate) -> Result<bool> {
    let nid = entity_update.net_state.network_entity_id;
    // Check if the entity with the given network_entity_id already exists
    for (_, transform, net_state)
        in engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()? {
        if nid == net_state.network_entity_id {
            net_state.transform = entity_update.transform.clone();
            // authoritative change on the server
            if let Some(tr) = &entity_update.transform {
                *transform = *tr;
            }

            // refresh net state
            *net_state = entity_update.net_state.clone();
            return Ok(true);
        }
    }
    Ok(false)
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
                updates.push(EntityUpdate {
                    action: NetworkEntityAction::Spawn,
                    net_state: net_state.clone(),
                    transform: Some(transform.clone()),
                });
                net_state.state = NetworkEntityState::Alive; // mark the entity as alive
                net_state.last_transform = Some(transform.clone());
                continue;
            }

            // Update transform only if it has changed since the last update
            let need_send = match &net_state.last_transform {
                Some(previous) => changed_enough(&transform, previous),
                None => true,
            };

            if need_send {
                if need_send {
                    updates.push(EntityUpdate {
                        action: NetworkEntityAction::Update,
                        net_state: net_state.clone(),
                        transform: Some(transform.clone()),
                    });
                    net_state.last_transform = Some(transform.clone());
                }
            }
        }

        if updates.is_empty() {
            // Nothing to send
            return Ok(());
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
			match client_send(&mut state.net, &msg) {
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

// ------------- NETWORKING PUBLIC SYSTEMS -------------

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
                            // TODO: to verify if we need to verify if we are not spawning a
                            // duplicate entity on the server
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
                    server_broadcast_except(&mut state.net, update.client_id, &NetworkPacket {
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

    client_try_reconnect(engine)?;
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
            let got_updates = !updates.is_empty();

            // there is just one Update in the vector
            for update in &updates {
                for entity_update in &update.updates {
                    match entity_update.action {
                        NetworkEntityAction::Spawn => {
                            println!("Spawn ◂ from cid={}  nid={:?}", update.client_id, entity_update.net_state.network_entity_id);
                            if !run_update_for_existing_entity(engine, entity_update)? {
                                run_spawn_hooks(engine, entity_update)?;
                            }
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
                                        break;
                                    }
                                }
                            }
                        },
                    }
                }
            }

            if got_updates {
                mark_connected(engine)?;
            }
        },
        Err(e) => {
            println!("Failed to receive network updates: {}", e);
            return Err(e);
        }
    }

    Ok(())
}
