#![cfg(feature = "net")]

use anyhow::Result;
use std::time::Duration;

use crate::engine::Engine;
use crate::ecs::components::net_components::{NetState, NetSide, NetStats};

use pill_net::{
    Msg,
    server_update, srv_send_one, srv_broadcast, srv_flush,
    client_update, cli_send, cli_flush,
};

const DT: Duration = Duration::from_millis(16); // TODO: we should specify it in some other way

pub fn net_recv_system(engine: &mut Engine) -> Result<()> {
    let state = engine.get_global_component_mut::<NetState>()?;

    match &mut state.side {
        NetSide::Server(net) => {
            let inbox = server_update(net, DT)?;
            for (client_id, msg) in inbox {
                match msg {
                    Msg::Ping(t) => {
                        println!("Srv: getting ping, tick: {}", state.tick);
                        log::info!("Srv: Ping from {client_id}, t: {t:?}");
                        // reply with Pong just to one client
                        srv_send_one(net, client_id, &Msg::Pong(t))?;
                        // echo current counter back to all
                        srv_broadcast(net, &Msg::Counter(state.tick))?;
                    }
                    Msg::Pong(t) => {
                        println!("Srv: getting pong, tick: {}", state.tick);
                        log::debug!("Srv: unexpected Pong from {client_id}, t: {t:?}");
                    }
                    Msg::Counter(_) => {}
                }
            }
        }
        NetSide::Client(net) => {
            let inbox = client_update(net, DT)?;
            for msg in inbox {
                match msg {
                    Msg::Counter(v) => {
                        println!("Cli: getting counter = {v}");
                        log::info!("Cli: Counter = {v}");
                        if let Ok(stats) = engine.get_global_component_mut::<NetStats>() {
                            stats.last_counter = v;
                        }
                    }
                    Msg::Pong(t) => {
                        println!("Cli: pong");
                        log::info!("Cli: Pong, t: {t:?}");
                    }
                    Msg::Ping(_) => {}
                }
            }
        }
    }
    Ok(())
}

pub fn net_send_system(engine: &mut Engine) -> Result<()> {
    let state = engine.get_global_component_mut::<NetState>()?;

    match &mut state.side {
        NetSide::Server(net) => {
            println!("Srv: broadcasting counter, tick: {}", state.tick);
            state.tick = state.tick.wrapping_add(1);
            srv_broadcast(net, &Msg::Counter(state.tick))?;
        }
        NetSide::Client(net) => {
            state.tick = state.tick.wrapping_add(1);
            if state.tick % 60 == 0 {
                println!("Cli: sending Ping, tick: {}", state.tick);
                cli_send(net, &Msg::Ping(state.tick as u64))?;
            }
        }
    }
    Ok(())
}

pub fn net_flush_system(engine: &mut Engine) -> Result<()> {
    let state = engine.get_global_component_mut::<NetState>()?;
    match &mut state.side {
        NetSide::Server(net) => srv_flush(net)?,
        NetSide::Client(net) => cli_flush(net)?,
    }
    Ok(())
}
