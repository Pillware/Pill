use anyhow::Result;
use egui::util::id_type_map::TypeId;
use std::{collections::HashMap};
use pill_core::{PillTypeMapKey, srv_start, cli_connect, NetClient, NetServer};
use crate::{ecs::{EntityHandle, Component, GlobalComponent, GlobalComponentStorage}, engine::Engine};

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
            side: NetSide::Server(srv_start(addr, max_clients)?),
            my_id: 0, // Server does not have a client ID
            tick: 0,
            accumulator: 0.0,
            timeout: UPDATE_FREQ_SEC,
            seq: 0,
        })
    }

    pub fn new_client(addr: &str, my_id: u64) -> Result<Self> {
        Ok(Self {
            side: NetSide::Client(cli_connect(addr, my_id)?),
            my_id,
            tick: 0,
            accumulator: 0.0,
            timeout: UPDATE_FREQ_SEC,
            seq: 0,
        })
    }
}
