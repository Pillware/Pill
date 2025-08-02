#![cfg(feature = "net")]

use anyhow::Result;
use pill_net::{client_update, server_update, cli_send, srv_broadcast, cli_flush, srv_flush, WireMsg, WireTag};

use crate::ecs::components::transform_component;
use crate::engine::Engine;
use crate::ecs::{EntityHandle, TransformComponent, TimeComponent, NetworkStateComponent, NetEntityState};
use crate::{NetSide, NetState, SpawnDespawnQueueComponent};

use serde::{Deserialize, Serialize};
use std::time::Duration;
use bincode;


#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetEntityAction {
    Spawn,
    Despawn,
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityUpdate {
    pub action: NetEntityAction,
    pub entity_handle: EntityHandle,
    pub transform: Option<TransformComponent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkUpdatePayload {
    pub client_id: u64,
    pub updates: Vec<EntityUpdate>,
}

const DT: Duration = Duration::from_millis(16); // TODO: we should specify it in some other way

    // TODO: optimization -> can have a global array with indices of entities that have changed
    // the ids are removed when the interpolation has finished etc
    // 4 operations:
    // // this is a first iterator
    // 1. Iterate over all entities that have changed (E.g. Transform + Health +
    //    NetworkStateComponent (only these that are networking)
    // creates network payloads with if dirty (flag that the component has changed)
    // 2. Send the payloads to the server
    //   Flags the components as not dirty
    // 3. Receives the payloads with EntityHandles
    //   Finds NetworkStateComponent by the entity handle
    //   updates the target fields (e.g. TransformComponent)
    //  // this is the second iterator
    // 4. Now it does the interpolation of the entity it received
    //  checks the current transform and the target transform and steps
fn is_not_ready(err: &anyhow::Error) -> bool {
        err.to_string().contains("disconnected or connecting")
}

fn prepare_network_update(engine: &mut Engine) -> Result<NetworkUpdatePayload> {
    // ----- STEP 1: Gather the updates to send -----
    let mut entity_updates: Vec<EntityUpdate> = Vec::new();

    for (eh, transform, network_state) in engine.iterate_two_components_mut::<TransformComponent, NetworkStateComponent>()? {
        match network_state.state {
            NetEntityState::Spawn => {
                log::debug!("Spawn  ▸ {:?}", eh);
                entity_updates.push(EntityUpdate {
                    action: NetEntityAction::Spawn,
                    entity_handle: eh.clone(),
                    transform: Some(transform.clone()),
                });
                network_state.state = NetEntityState::Alive; // Reset state after spawning
            },
            NetEntityState::Despawn => {
                log::debug!("Despawn ▸ {:?}", eh);
                entity_updates.push(EntityUpdate {
                    action: NetEntityAction::Despawn,
                    entity_handle: eh.clone(),
                    transform: None, // No transform needed for despawn
                });
            },
            NetEntityState::Alive => {
                // Iterate over the components to see if we need to update the transform
                log::info!("Update ▸ {:?}  t={:?}", eh, transform.position);
                if transform.net_dirty {
                    entity_updates.push(EntityUpdate {
                        action: NetEntityAction::Update,
                        entity_handle: eh.clone(),
                        transform: Some(transform.clone()),
                    });
                    transform.net_dirty = false; // Reset dirty flag after processing
                }
                // TODO: iterate over other components that might be dirty
            }
        }
    }

    let id = engine.get_global_component::<NetState>()?.my_id;
    //log::info!("Prepared {} entity updates (client_id={id})", entity_updates.len());

    // Create the network update payload
    let pkt = NetworkUpdatePayload {
        client_id: id,
        updates: entity_updates,
    };

    Ok(pkt)
}

fn send_network_update(engine: &mut Engine, pkt: &NetworkUpdatePayload) -> Result<()> {
    // ----- STEP 2: Send the updates -----
    if pkt.updates.is_empty() {
        //log::info!("Nothing to send this frame");
        return Ok(());
    }

    let bytes = bincode::serialize(pkt)?;
    let msg = pill_net::WireMsg {
        tag: WireTag::Update,
        data: bytes,
    };
    let state = engine.get_global_component_mut::<NetState>()?;
    if let NetSide::Client(net) = &mut state.side {
		if let Err(e) = (|| { cli_send(net, &msg)?; cli_flush(net) })() {
            if !is_not_ready(&e) { return Err(e); }          // real error
            log::info!("Cli ▸ not connected yet – update skipped");
        }
          log::debug!("Client ▸ sent {}B ({} updates)",
                        msg.data.len(), pkt.updates.len());
    } else if let NetSide::Server(net) = &mut state.side {
        srv_broadcast(net, &msg)?;
        srv_flush(net)?; // TODO: is it too early? should we wait?
		log::debug!("Server ▸ broadcast {}B ({} updates) to {} clients",
					msg.data.len(), pkt.updates.len(), net.server.clients_id().len());
    }
    Ok(())
}

fn receive_network_updates(engine: &mut Engine) -> Result<()> {
    // ----- STEP 3: Process received updates -----
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
    // TODO: dt -> to be input to the network component
    //
    //
    let mut incoming: Vec<NetworkUpdatePayload> = Vec::new();
    let state = engine.get_global_component_mut::<NetState>()?;

	match &mut state.side {
        NetSide::Client(net) => {
            match client_update(net, DT) {
                Ok(msgs) => {
                    for msg in &msgs {
                        if msg.tag == WireTag::Update {
                            let pkt: NetworkUpdatePayload = bincode::deserialize(&msg.data)?;
                            incoming.push(pkt);
                        }
                    }
                    log::info!("Client ◂ received {} updates from srv", msgs.len());
                },
                Err(e) if is_not_ready(&e) => {
                    log::info!("Cli ▸ not connected yet – update skipped");
                    return Ok(());
                },
                Err(e) => return Err(e),
            }
        }
        NetSide::Server(net) => {
            for (cid, msg) in server_update(net, DT)? {
                if msg.tag == WireTag::Update {
                    let pkt: NetworkUpdatePayload = bincode::deserialize(&msg.data)?;
                    log::info!("Server ◂ received {} updates from cid={cid}", pkt.updates.len());
                    incoming.push(pkt);
                }
            }
        }
    }

    for pkt in incoming {
    for update in pkt.updates {
            match update.action {
                NetEntityAction::Spawn => {
                    let tr = update.transform
                                  .clone()
                                  .unwrap_or_else(TransformComponent::default);
                    log::debug!("Spawn ◂ from cid={}  ent={:?}", pkt.client_id, update.entity_handle);
                    engine.get_global_component_mut::<SpawnDespawnQueueComponent>()?
                          .spawn
                          .push((pkt.client_id, update.entity_handle.clone(), tr));
                },
                NetEntityAction::Despawn => {
                    // TODO: implement
                    log::warn!("Despawn action not yet implemented (ent={:?})", update.entity_handle);
                },
                NetEntityAction::Update => {
                    // Handle updating the entity's transform
                    if let Some(tr) = &update.transform {
                        let mut found = false;
                        for (eh, _, net_state)
                            in engine.iterate_two_components_mut::<TransformComponent,
                                                                   NetworkStateComponent>()?
                        {
                            if eh == update.entity_handle {
                                net_state.transform = Some(tr.clone());
                                found = true;
                                break;
                            }
                        }

                        // TODO: it is possible that the entity was spawned before a client joined the
                        // game - in this case a new entity should be spawned
                        if !found {
                            log::warn!("Update for unknown ent={:?}; queuing spawn", update.entity_handle);
							engine.get_global_component_mut::<SpawnDespawnQueueComponent>()?
                                  .spawn
                                  .push((pkt.client_id, update.entity_handle.clone(), tr.clone()));
                        }
                    }
                },
            }
        }
    }
    Ok(())
}

// TODO: implement
fn interpolate_entities(engine: &mut Engine) -> Result<()> {
    // This function would handle the interpolation of entities based on the received updates.
    // It would typically involve checking the current state and the target state of each entity
    // and applying interpolation logic to smooth out movements.

    // For now we will set the transforms directly (add the intreapolation step later)
    Ok(())
}

pub fn networking_system(engine: &mut Engine) -> Result<()> {
    let dt = engine.get_global_component::<TimeComponent>()?.delta_time;
    let state = engine.get_global_component_mut::<NetState>()?;
    state.accumulator += dt;
    if state.accumulator < state.timeout {
        // Not enough time has passed to process the next network update
        return Ok(());
    }
    state.accumulator = 0.0; // Reset the accumulator

    let update = prepare_network_update(engine)?;

    send_network_update(engine, &update)?;

    receive_network_updates(engine)?;

    interpolate_entities(engine)?;

    Ok(())
}
