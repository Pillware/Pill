use anyhow::Result;
use log::debug;
use pill_core::{NetClient, cli_update, srv_update, cli_send, srv_broadcast, srv_send_one, cli_flush, srv_flush, WireMsg, WireTag};

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

fn is_not_ready(err: &anyhow::Error) -> bool {
        err.to_string().contains("disconnected or connecting")
}

fn try_send_and_flush(net: &mut NetClient,
                      msg: &WireMsg) -> Result<()> {

    // ── send ───────────────────────────────────────────────────────────────
    match cli_send(net, msg) {
        Ok(_) => {}
        Err(e) if is_not_ready(&e) => {
            println!("[Client] ▸ not connected yet – send skipped");
            return Ok(());                    // bail out early this frame
        }
        Err(e) => return Err(e),              // real error → bubble up
    }

    // ── flush ──────────────────────────────────────────────────────────────
    match cli_flush(net) {
        Ok(_) => {}
        Err(e) if is_not_ready(&e) => {
            println!("[Client] ▸ not connected yet – flush skipped");
        }
        Err(e) => return Err(e),
    }
    Ok(())
}

fn receive_updates(engine: &mut Engine) -> Result<Vec<NetworkUpdatePayload>> {
    // ────────────────────────────────────────────────────────────────
    // 0.  Immutable borrow just to read the timeout
    // ────────────────────────────────────────────────────────────────
    let timeout = {
        let state = engine.get_global_component::<GlobalNetState>()?;
        state.timeout
    };
    let dt = Duration::from_secs_f32(timeout);
    //println!("Advancing networking system with dt={}", dt.as_secs_f32());

    let mut updates:    Vec<NetworkUpdatePayload> = Vec::new();
    let mut join_cids:  Vec<u64>                  = Vec::new();   // remember JOINs

    // ────────────────────────────────────────────────────────────────
    // 1.  Handle wire traffic – first and only long-lived &mut borrow
    // ────────────────────────────────────────────────────────────────
    {
        let mut state = engine.get_global_component_mut::<GlobalNetState>()?;

        match &mut state.side {
            // ── CLIENT ────────────────────────────────────────────
            NetSide::Client(net) => {
                match cli_update(net, dt) {
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
                        //println!("Client ◂ received {} updates from srv", msgs.len());
                    }
                    Err(e) if is_not_ready(&e) => {
                        println!("[Client] ▸ not connected yet – update skipped");
                        return Ok(updates); // keep early-out behaviour
                    }
                    Err(e) => return Err(e),
                }
            }

            // ── SERVER ────────────────────────────────────────────
            NetSide::Server(net) => {
               // println!("[Server] receiving updates from clients...");
                for (cid, msg) in srv_update(net, dt)? {
                    println!("[Server] ◂ received msg from cid={cid} with tag {:?}", msg.tag);

                    if msg.tag == WireTag::Update {
                        let pkt: NetworkUpdatePayload = bincode::deserialize(&msg.data)?;
                        println!(
                            "[Server] ◂ received pkt from cid={cid} at time {}", pkt.timestamp
                        );
                        //println!("Server ◂ received {} updates from cid={cid}", pkt.updates.len());
                        updates.push(pkt);
                    } else if msg.tag == WireTag::Join {
                        // handle client joining
                        println!("[Server] Client {cid} JOIN with cid={cid}");
                        join_cids.push(cid);      // snapshot will be sent later
                    }
                }
            }
        }
    } // ← first &mut borrow ends here

    // ────────────────────────────────────────────────────────────────
    // 2.  For every JOIN, build & send a world snapshot
    //     (short, independent borrows – no double-borrow)
    // ────────────────────────────────────────────────────────────────
    for cid in join_cids {
        // gather all entities currently on the server
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
        //println!("[Server] Sending {} entities to client {cid}", entity_updates.len());

        // wrap them in the usual payload
        let snapshot = NetworkUpdatePayload {
            client_id: 0,   // 0 = "server"
            updates:   entity_updates,
            timestamp: engine.get_global_component::<TimeComponent>()?.time, // TODO: debug
        };

        // short-lived borrow only to send/flush
        {
            let mut state = engine.get_global_component_mut::<GlobalNetState>()?;
            if let NetSide::Server(net) = &mut state.side {
                srv_send_one(
                    net,
                    cid,   // **only** the newcomer
                    &WireMsg {
                        tag:  WireTag::Update,
                        data: bincode::serialize(&snapshot)?,
                    },
                )?;
                srv_flush(net)?;
            }
        }

        println!(
            "[Server] → sent {} existing entities to cid={cid}",
            snapshot.updates.len()
        );
    }

    Ok(updates)
}

pub fn networking_system_server(engine: &mut Engine) -> Result<()> {
    {
		let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
		let state = engine.get_global_component_mut::<GlobalNetState>()?;
		state.accumulator += dt;
		if state.accumulator < state.timeout {
			// Not enough time has passed to process the next network update
			return Ok(());
		}
		state.accumulator = 0.0;
	}
    // Step 1: Receive network updates from clients and broadcast them to all clients
    //println!("networking_system_server: receiving updates from clients...");
    match receive_updates(engine) {
        Ok(updates) => {
            println!("Got {} updates from clients", updates.len());
            // Step 2: Process the updates and apply them to the entities in the server's world immediately
            // Broadcast the updates to all clients
            for update in &updates {
                // handle each NetworkUpdatePayload from a client
                for entity_update in &update.updates {
                    match entity_update.action {
                        NetEntityAction::Spawn => {
                            println!("Spawn ◂ from cid={}  nid={:?}", update.client_id, entity_update.net_state.net_entity_id);
                            let tr = entity_update.transform
                                                  .clone()
                                                  .unwrap_or_else(TransformComponent::default);
                            let spawn_fn = {
                                let global_net_state = engine.get_global_component::<GlobalNetState>()?;
                                global_net_state.spawn_handlers.get(&entity_update.net_state.kind).copied()
                            };

                            if let Some(spawn_fn) = spawn_fn{
                                spawn_fn(engine, &entity_update.net_state, &tr)?;
                            } else {
                                println!("No spawn handler for entity kind '{}'", entity_update.net_state.kind);
                            }
                        },
                        NetEntityAction::Despawn => {
                            println!("Despawn action not yet implemented (nid={:?})", entity_update.net_state.net_entity_id);
                        },
                        NetEntityAction::Update => {
                            // Handle updating the entity's transform
                            if let Some(tr) = &entity_update.transform {
                                for (_, transform, net_state)
                                    in engine.iterate_two_components_mut::<TransformComponent,
                                                                           NetworkStateComponent>()? {
                                    if entity_update.net_state.net_entity_id == net_state.net_entity_id {
                                        net_state.transform = Some(tr.clone());
                                        net_state.transform.as_mut().unwrap().net_dirty = false;

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
                    srv_broadcast(net, update.client_id, &WireMsg {
                        tag: WireTag::Update,
                        data: bincode::serialize(&update)?,
                    })?;
                    srv_flush(net)?;
                };
            }
            //println!("Received and broadcasted {} network updates", updates.len());
        },
        Err(e) => {
            println!("Failed to receive or broadcast network updates: {}", e);
            return Err(e);
        }
    }
    Ok(())
}

const LERP_RATE: f32 = 0.001;

pub fn networking_system_client(engine: &mut Engine) -> Result<()> {
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

    // Run it with a given frequency
    {
		let dt = engine.frame_delta_time;
		let state = engine.get_global_component_mut::<GlobalNetState>()?;
		state.accumulator += dt;
		if state.accumulator < state.timeout {
			// Not enough time has passed to process the next network update
			return Ok(());
		}
		state.accumulator = 0.0;
	}
    // Step 0: Iterate over all entities living on the client that have Networking component and send them to the server
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
               println!("▸ Spawning new entity for nid={:?} from cid={my_id}", net_state.net_entity_id);
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
            //println!("▸ Sending {} updates to server", payload.updates.len());
            try_send_and_flush(net, &WireMsg {
                tag: WireTag::Update,
                data: bincode::serialize(&payload)?,
            })?;
        }
    }


    // Step 1: Receive network updates from server
    match receive_updates(engine) {
        Ok(updates) => {
            // Step 2: Process the updates and apply interpolation to the entities in the client's
            // world
            // there is just one Update in the vector
            for update in &updates {
                for entity_update in &update.updates {
                    match entity_update.action {
                        NetEntityAction::Spawn => {
                            println!("Spawn ◂ from cid={}  nid={:?}", update.client_id, entity_update.net_state.net_entity_id);
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
                        },
                        NetEntityAction::Despawn => {
                            println!("Despawn action not yet implemented (nid={:?})", entity_update.net_state.net_entity_id);
                        },
                        NetEntityAction::Update => {
                            // Handle updating the entity's transform
                            if let Some(tr) = &entity_update.transform {
                                for (_, transform, net_state)
                                    in engine.iterate_two_components_mut::<TransformComponent,
                                                                           NetworkStateComponent>()? {
                                    if entity_update.net_state.net_entity_id == net_state.net_entity_id {
                                        net_state.transform = Some(tr.clone());
                                        net_state.transform.as_mut().unwrap().net_dirty = false;
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

                let state = engine.get_global_component_mut::<GlobalNetState>()?;
                if let NetSide::Server(net) = &mut state.side {
                    // Broadcast the update to everyone except the sender
                    srv_broadcast(net, update.client_id, &WireMsg {
                        tag: WireTag::Update,
                        data: bincode::serialize(&update)?,
                    })?;
                    srv_flush(net)?;
                };
            }
            //println!("Received and broadcasted {} network updates", updates.len());
        },
        Err(e) => {
            println!("Failed to receive or broadcast network updates: {}", e);
            return Err(e);
        }
    }

    Ok(())
}

#[inline(always)]
fn exp_blend(dt: f32) -> f32 {
        1.0 - (-LERP_RATE * dt).exp()           // ≈ LERP_RATE * dt for small dt
}

fn lerp_vec3(from: Vector3f, to: Vector3f, t: f32) -> Vector3f {
    from + (to - from) * t.clamp(0.0, 1.0)
}
