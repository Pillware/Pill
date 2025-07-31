#![cfg(feature = "net")]

use anyhow::Result;
use std::time::Duration;

use crate::ecs::TransformComponent;
use crate::engine::Engine;
use crate::ecs::components::net_components::{NetState, NetSide, NetStats};
use crate::ecs::components::spawn_queue_component::SpawnQueueComponent;

use pill_net::{
    Msg, TrPacket,
    server_update, srv_send_one, srv_broadcast, srv_flush,
    client_update, cli_send, cli_flush,
};

const DT: Duration = Duration::from_millis(16); // TODO: we should specify it in some other way

pub fn net_recv_system(engine: &mut Engine) -> Result<()> {
    // temporary vector for join colleciton
    let mut joins_to_enqueue: Vec<(u64, TrPacket)> = Vec::new();

    {
        let state = engine.get_global_component_mut::<NetState>()?;

        match &mut state.side {
            NetSide::Server(net) => {
                let inbox = server_update(net, DT)?;
                for (cid, msg) in inbox {
                    match msg {
                        Msg::Join {client_id, tr } => {
                            log::info!("Srv ▸ JOIN received  cid={client_id}  raw={tr:?}");
                            let pkt = tr.unwrap_or_else( ||{
                                log::warn!("Srv: Join without transform, using default");
                                TrPacket::default()
                            });
                            // push onto queue; Game logics spawns it later
                            joins_to_enqueue.push((client_id, pkt));
                            log::info!("Srv ▹ queued  cid={client_id}  joins_to_enqueue.len()={}", joins_to_enqueue.len());
                            // acknowledge the spawn to all clients (// TODO: is this not too complex?)
                            srv_broadcast(net, &Msg::Tr{
                                client_id,
                                tr: pkt,
                            })?;
                            log::info!("Srv: TR broadcast, client_id: {}", client_id);
                        },
                        Msg::Tr {client_id, tr} => {
                            // forward authoritative transform to all clients
                            srv_broadcast(net, &Msg::Tr {
                                client_id,
                                tr,
                            })?;
                            log::warn!("Srv: Got TR from {client_id}, forwarding to all clients");
                        },
                        Msg::Ping(t) => { srv_send_one(net, cid, &Msg::Pong(t))?; }
                        _ => {}
                    }
                }
            }
            NetSide::Client(net) => {
                // Send join exactly once
                if !state.join_sent {
                    cli_send(net, &Msg::Join {
                        client_id: state.my_id,
                        tr: None, // TODO: send initial transform
                    })?;
                    state.join_sent = true;
                    log::info!("Cli: JOIN sent, client_id: {}", state.my_id);
                }
                let inbox = client_update(net, DT)?;
                for msg in inbox {
                    match msg {
                        Msg::Tr { client_id, tr } => {
                            log::info!("Cli ◂ TR arrived  cid={client_id}  pkt={tr:?}");
                            handle_remote_transform(engine, client_id, tr)?;
                        }
                        Msg::Pong(_)| Msg::Ping(_) => {}
                        Msg::Join {..} => {}
                    }
                }
            }
        }
    }

    if !joins_to_enqueue.is_empty() {
        // Enqueue all joins to the spawn queue
        if let Ok(q) = engine.get_global_component_mut::<SpawnQueueComponent>() {
            q.requests.extend(joins_to_enqueue);
        }
    }
    Ok(())
}

pub fn net_send_system(engine: &mut Engine) -> Result<()> {
    let state = engine.get_global_component_mut::<NetState>()?;
    state.tick = state.tick.wrapping_add(1);

    if let NetSide::Client(net) = &mut state.side {
        if state.tick % 60 == 0 {
            println!("Cli: sending Ping, tick: {}", state.tick);
            cli_send(net, &Msg::Ping(state.tick as u64))?;
        }
    }
    Ok(())
}

pub fn net_flush_system(engine: &mut Engine) -> Result<()> {
    let state = engine.get_global_component_mut::<NetState>()?;
    match &mut state.side {
        NetSide::Server(net) => srv_flush(net)?,
        NetSide::Client(net) => {
            if let Err(e) = cli_flush(net) {
                // Ignore "disconnected or connecting" until we are actually connected
                if e.to_string().contains("disconnected or connecting") {
                    return Ok(());
                }
                return Err(e.into());
            }
        }
    }
    Ok(())
}

// updates in all clients
fn handle_remote_transform(
    engine: &mut Engine,
    client_id: u64,
    tr: TrPacket,
) -> Result<()> {
    let scene = engine.scene_manager.get_active_scene_mut()?;

    {
        let state = engine.get_global_component_mut::<NetState>()?;
        // If this is our own client, ensure we registered it
        if let Some(&ent) = state.entity_by_client.get(&client_id) {
            log::info!("Cli ◂ UPDATE local ent={:?} with pkt={:?}", ent, tr);
            // If we have an entity for this client, update its transform
            for (eh, trc) in engine.iterate_one_component_mut::<TransformComponent>()? {
                if eh == ent { *trc = TransformComponent::from(&tr); }
            }
            return Ok(());
        }
    }

    // push into SpawnQueueComponent - will be spawned by the system
    if let Ok(q) = engine.get_global_component_mut::<SpawnQueueComponent>() {
        log::info!("Cli ◂ QUEUE spawn for cid={client_id}");
        q.requests.push((client_id, tr));
    }
    Ok(())
}
