#![cfg(feature = "net")]
use anyhow::Result;
use std::collections::HashMap;
use pill_core::PillTypeMapKey;
use crate::{ecs::{EntityHandle, GlobalComponent, GlobalComponentStorage}, engine::Engine};

use pill_net::{start_server, connect_client, NetClient, NetServer};

pub enum NetSide {
    Server(NetServer),
    Client(NetClient),
}

// Global state of networking in this instance
pub struct NetState {
    pub side: NetSide,
    pub my_id: u64, // Cliend ID
    pub join_sent: bool, // true if Join message was sent
    pub entity_by_client: HashMap<u64, EntityHandle>,
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
            my_id: 0, // Server does not have a client ID
            join_sent: true, // server never needs to join
            entity_by_client: HashMap::new(),
            tick: 0,
        })
    }

    pub fn new_client(addr: &str, my_id: u64) -> Result<Self> {
        Ok(Self {
            side: NetSide::Client(connect_client(addr, my_id)?),
            my_id,
            join_sent: false, // Client needs to send Join message
            entity_by_client: HashMap::new(),
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

