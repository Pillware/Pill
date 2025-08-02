#![cfg(feature = "net")]
use anyhow::Result;
use egui::util::id_type_map::TypeId;
use std::{collections::HashMap};
use pill_core::PillTypeMapKey;
use crate::{ecs::{EntityHandle, Component, GlobalComponent, GlobalComponentStorage}, engine::Engine};

use pill_net::{start_server, connect_client, NetClient, NetServer};

const UPDATE_FREQ_HZ: f32 = 3.0; // Update frequency in Hz
const UPDATE_FREQ_SEC: f32 = 1.0 / UPDATE_FREQ_HZ; // Update frequency in seconds

pub enum NetSide {
    Server(NetServer),
    Client(NetClient),
}

// Global state of networking in this instance
pub struct NetState {
    pub side: NetSide,
    pub my_id: u64, // Client ID
    pub tick: u64,
    pub accumulator: f32, // running counter to reduce the tick rate
    pub timeout: f32,
    pub seq: u64, // Sequence number for packets
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
            tick: 0,
            accumulator: 0.0,
            timeout: UPDATE_FREQ_SEC,
            seq: 0,
        })
    }

    pub fn new_client(addr: &str, my_id: u64) -> Result<Self> {
        Ok(Self {
            side: NetSide::Client(connect_client(addr, my_id)?),
            my_id,
            tick: 0,
            accumulator: 0.0,
            timeout: UPDATE_FREQ_SEC,
            seq: 0,
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

