use anyhow::Result;
use egui::util::id_type_map::TypeId;
use std::{collections::{HashMap, HashSet}, time::Instant};
use pill_core::{PillTypeMapKey, server_start, client_connect, NetworkClient, NetworkServer};
use crate::{ecs::{EntityHandle, Component, GlobalComponent, GlobalComponentStorage, NetworkStateComponent, TransformComponent}, engine::Engine};

pub type ClientId = u64;
pub type NetEntityId = u64;

// Client-side state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug)]
pub struct ClientState {
    pub net: NetworkClient,
    pub server_address: String,
    pub connection_state: ConnectionState,
    pub want_reconnect: bool,
    pub backoff_ms: u64,
    pub next_try_s: f32,
    pub world_epoch: u64,
    pub connection_seq: u64,
}

// Server-side state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Offline,
    Online
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflinePolicy {
    Despawn,
    Freeze,
    AI
}

#[derive(Debug)]
pub struct Session {
    pub status: SessionStatus,
    pub offline_policy: OfflinePolicy,
    pub owned: HashSet<NetEntityId>,
    pub reconnect_deadline: Instant
}

#[derive(Debug)]
pub struct ServerState {
    pub net: NetworkServer,
    pub world_epoch: u64,
    pub offline_policy: OfflinePolicy,
    pub sessions: HashMap<ClientId, Session>,
}

pub enum NetworkSide {
    Server(ServerState),
    Client(ClientState),
}

type SpawnFn = fn(&mut Engine, &NetworkStateComponent, &TransformComponent) -> Result<()>;
type DespawnFn = fn(&mut Engine, &NetworkStateComponent) -> Result<()>;
type InterpolationHookFn = fn(&mut Engine) -> Result<()>;

// Global state of networking in this instance of the engine
pub struct NetworkManagerComponent {
    pub side: NetworkSide,
    pub my_id: u64, // Client ID for this instance (0 for server)
    pub tick: u64,
    pub accumulator: f32, // running counter to reduce the tick rate
    pub timeout: f32,
    pub spawn_handlers: HashMap<String, SpawnFn>, // Handlers for spawning entities based on type
    pub despawn_handlers: HashMap<String, DespawnFn>,
    pub client_interpolation_hook: Option<InterpolationHookFn>, // Optional hook for client-side interpolation
}

impl PillTypeMapKey for NetworkManagerComponent {
    type Storage = GlobalComponentStorage<NetworkManagerComponent>;
}
impl GlobalComponent for NetworkManagerComponent {}

const UPDATE_FREQUENCY_HZ: f32 = 3.0; // Update frequency in Hz
const UPDATE_FREQUENCY_SEC: f32 = 1.0 / UPDATE_FREQUENCY_HZ; // Update frequency in seconds


impl NetworkManagerComponent {
    pub fn new_server(address: &str, max_clients: usize) -> Result<Self> {
        let server = server_start(address, max_clients)?;
        Ok(Self {
            side: NetworkSide::Server(ServerState {
                net: server,
                world_epoch: 1,
                offline_policy: OfflinePolicy::Despawn,
                sessions: HashMap::new(),
            }),
            my_id: 0, // Server does not have a client ID
            tick: 0,
            accumulator: 0.0,
            timeout: UPDATE_FREQUENCY_SEC,
            spawn_handlers: HashMap::new(),
            despawn_handlers: HashMap::new(),
            client_interpolation_hook: None,
        })
    }

    pub fn new_client(address: &str, my_id: u64) -> Result<Self> {
        let client = client_connect(address, my_id)?;
        Ok(Self {
            side: NetworkSide::Client(ClientState {
                net: client,
                server_address: address.to_string(),
                connection_state: ConnectionState::Connecting,
                want_reconnect: false,
                backoff_ms: 500,
                next_try_s: 0.0,
                world_epoch: 1,
                connection_seq: 0,
            }),
            my_id,
            tick: 0,
            accumulator: 0.0,
            timeout: UPDATE_FREQUENCY_SEC,
            spawn_handlers: HashMap::new(),
            despawn_handlers: HashMap::new(),
            client_interpolation_hook: None,
        })
    }

    #[inline]
    pub fn server_mut(&mut self) -> Option<&mut ServerState> {
        match &mut self.side { NetworkSide::Server(s) => Some(s), _ => None }
    }
    #[inline]
    pub fn client_mut(&mut self) -> Option<&mut ClientState> {
        match &mut self.side { NetworkSide::Client(c) => Some(c), _ => None }
    }
    #[inline]
    pub fn is_server(&self) -> bool {
        matches!(&self.side, NetworkSide::Server(_))
    }
    #[inline]
    pub fn is_client(&self) -> bool {
        matches!(&self.side, NetworkSide::Client(_))
    }
}
