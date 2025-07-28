#![cfg(feature = "net")]
use anyhow::Result;
use pill_core::PillTypeMapKey;
use crate::{ecs::{GlobalComponent, GlobalComponentStorage}, engine::Engine};

use pill_net::{start_server, connect_client, NetClient, NetServer};

pub enum NetSide {
    Server(NetServer),
    Client(NetClient),
}

// Global state of networking in this instance
pub struct NetState {
    pub side: NetSide,
    pub tick: u64,
}

impl PillTypeMapKey for NetState {
    type Storage = GlobalComponentStorage<NetState>;
}
impl GlobalComponent for NetState {}

impl NetState {
    pub fn new_server(addr: &str, max_clients: usize) -> Result<Self> {
        Ok(Self {
            side: NetSide::Server(start_server(addr, max_clients)?),
            tick: 0,
        })
    }

    pub fn new_client(addr: &str, client_id: u64) -> Result<Self> {
        Ok(Self {
            side: NetSide::Client(connect_client(addr, client_id)?),
            tick: 0,
        })
    }
}

// TODO: do we want to split the components?
// Debug component
pub struct NetStats {
    pub last_counter: u64,
}

impl PillTypeMapKey for NetStats {
    type Storage = GlobalComponentStorage<NetStats>;
}
impl GlobalComponent for NetStats {}
impl NetStats {
    pub fn new() -> Self {
        Self {
            last_counter: 0,
        }
    }
}

