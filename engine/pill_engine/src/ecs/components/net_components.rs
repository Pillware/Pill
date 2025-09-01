use anyhow::Result;
use egui::util::id_type_map::TypeId;
use std::{collections::HashMap};
use pill_core::{PillTypeMapKey, srv_start, cli_connect, NetClient, NetServer};
use crate::{ecs::{EntityHandle, Component, GlobalComponent, GlobalComponentStorage, NetworkStateComponent, TransformComponent}, engine::Engine};

const UPDATE_FREQ_HZ: f32 = 3.0; // Update frequency in Hz
const UPDATE_FREQ_SEC: f32 = 1.0 / UPDATE_FREQ_HZ; // Update frequency in seconds

pub enum NetSide {
    Server(NetServer),
    Client(NetClient),
}

type SpawnFn = fn(&mut Engine, &NetworkStateComponent, &TransformComponent) -> Result<()>;
type DespawnFn = fn(&mut Engine, &NetworkStateComponent) -> Result<()>;
type InterpolationHookFn = fn(&mut Engine) -> Result<()>;

// Global state of networking in this instance of the engine
pub struct GlobalNetState {
    pub side: NetSide,
    pub my_id: u64, // Client ID
    pub tick: u64,
    pub accumulator: f32, // running counter to reduce the tick rate
    pub timeout: f32,
    pub spawn_handlers: HashMap<String, SpawnFn>, // Handlers for spawning entities based on type
    pub despawn_handlers: HashMap<String, DespawnFn>,
    pub client_interpolation_hook: Option<InterpolationHookFn>, // Optional hook for client-side interpolation
}

impl PillTypeMapKey for GlobalNetState {
    type Storage = GlobalComponentStorage<GlobalNetState>;
}
impl GlobalComponent for GlobalNetState {}

impl GlobalNetState {
    pub fn new_server(addr: &str, max_clients: usize) -> Result<Self> {
        Ok(Self {
            side: NetSide::Server(srv_start(addr, max_clients)?),
            my_id: 0, // Server does not have a client ID
            tick: 0,
            accumulator: 0.0,
            timeout: UPDATE_FREQ_SEC,
            spawn_handlers: HashMap::new(),
            despawn_handlers: HashMap::new(),
            client_interpolation_hook: None,
        })
    }

    pub fn new_client(addr: &str, my_id: u64) -> Result<Self> {
        Ok(Self {
            side: NetSide::Client(cli_connect(addr, my_id)?),
            my_id,
            tick: 0,
            accumulator: 0.0,
            timeout: UPDATE_FREQ_SEC,
            spawn_handlers: HashMap::new(),
            despawn_handlers: HashMap::new(),
            client_interpolation_hook: None,
        })
    }
}
