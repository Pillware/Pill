//! # Network Manager
//!
//! `NetworkManagerComponent` is the global ECS component that owns all runtime
//! **networking state** for the current engine instance (either server or client).
//!
//! It wraps the low-level networking primitives from `pill_core::networking`
//! (server/client sockets, transports) and provides a place to keep engine-side
//! policy and glue (spawn/despawn handlers, interpolation hook, tick cadence,
//! reconnect backoff, etc.).
//!
//! ## Responsibilities
//! - Initialize and hold either **server** or **client** state ([`NetworkSide`]),
//! - Track connection lifecycle ([`ConnectionState`], reconnect intent/backoff),
//! - Maintain world epoch and per-connection sequence,
//! - Provide **typed hooks** for engine-level spawn/despawn/interpolation,
//! - Drive update cadence via [`NetworkManagerComponent::tick`], [`NetworkManagerComponent::accumulator`], and [`NetworkManagerComponent::timeout`].
//!
//! ## Typical usage
//! - **Server:** [`NetworkManagerComponent::new_server`] →  create the server object with given
//! address and max clients, then per-frame drive the server side.
//! - **Client:** [`NetworkManagerComponent::new_client`] → create the client object with given
//! client ID and server address to connect to, then per-frame drive the client side.
//!
//! See also: `pill_core::networking` (packet format, transports, send/receive) and
//! `pill_engine::systems::networking` for details on networking implementation.

use anyhow::Result;
use egui::util::id_type_map::TypeId;
use std::{collections::{HashMap, HashSet}, time::Instant};
use pill_core::{PillTypeMapKey, server_start, client_connect, NetworkClient, NetworkServer};
use crate::{ecs::{EntityHandle, Component, GlobalComponent, GlobalComponentStorage, NetworkStateComponent, TransformComponent}, engine::Engine};

/// Client-side connection lifecycle.
///
/// This is **engine-facing** UI/state; low-level connectivity lives in
/// `pill_core::networking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// Not connected; client may be idle or preparing to reconnect.
    Disconnected,
    /// Handshake/backoff in progress.
    Connecting,
    /// Fully connected and exchanging packets.
    Connected,
}

/// Per-client runtime state kept by the engine.
#[derive(Debug)]
pub struct ClientState {
    /// Thin wrapper over renet+netcode client (from `pill_core`).
    pub net: NetworkClient,
    /// Server address string used for (re)connects.
    pub server_address: String,
    /// High-level connection state (see [`ConnectionState`]).
    pub connection_state: ConnectionState,
    /// If `true`, engine should attempt reconnects when disconnected.
    pub want_reconnect: bool,
    /// Exponential backoff baseline, in milliseconds.
    pub backoff_ms: u64,
    /// Time (seconds, engine clock) when next reconnect attempt is allowed.
    pub next_try_s: f32,
    /// World-version marker synced from the server (engine-defined semantics).
    pub world_epoch: u64,
    /// Monotonic sequence for this connection (engine-defined semantics).
    pub connection_seq: u64,
}

/// Policy for entities owned by a client that goes offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflinePolicy {
    /// Despawn entities immediately.
    Despawn,
    /// Keep entities in place and stop simulating.
    Freeze,
    /// Hand control over to AI systems.
    AI
}

/// Per-server runtime state kept by the engine.
#[derive(Debug)]
pub struct ServerState {
    /// Thin wrapper over renet+netcode server (from `pill_core`).
    pub net: NetworkServer,
    /// World-version marker used by the server.
    pub world_epoch: u64,
    /// What to do with offline client entities.
    pub offline_policy: OfflinePolicy,
}

/// Which side this engine instance is currently running on.
pub enum NetworkSide {
    /// Dedicated or authoritative server.
    Server(ServerState),
    /// Client instance.
    Client(ClientState),
}

/// Function pointer type for spawning an entity of a given type.
///
/// The string key in [`NetworkManagerComponent::spawn_handlers`] determines
/// which handler to call (e.g. `"Player"`, `"Crate"`, …). The engine chooses
/// the mapping and payload structure; this hook just performs the actual spawn.
type SpawnFn = fn(&mut Engine, &NetworkStateComponent, &TransformComponent) -> Result<()>;

/// Function pointer type for despawning.
type DespawnFn = fn(&mut Engine, &NetworkStateComponent) -> Result<()>;

/// Optional per-tick interpolation callback for the client.
///
/// Called after network updates to smooth visuals (e.g. blend transforms).
type InterpolationHookFn = fn(&mut Engine) -> Result<()>;

/// Global ECS component holding all network management state.
///
/// Insert one into the **global** component storage. It exposes helpers to
/// determine side ([`NetworkManagerComponent::is_server`]/[`NetworkManagerComponent::is_client`]) and
/// provides mutable accessors to the underlying server/client states.
///
/// The component also tracks **update cadence**:
/// - [`Self::tick`]: monotonically increasing tick count,
/// - [`Self::accumulator`]: time accumulator used to throttle below frame rate,
/// - [`Self::timeout`]: desired seconds between network updates (derived from
///   (`UPDATE_FREQUENCY_HZ`).
///
/// Handler maps let you register type-based spawn/despawn functions without
/// coupling your networking systems to specific game types.
pub struct NetworkManagerComponent {
    /// Current side (server or client), with its state.
    pub side: NetworkSide,
    /// The local client ID; `0` is reserved for server.
    pub my_id: u64,
    /// Monotonic tick counter for network updates.
    pub tick: u64,
    /// Running time accumulator (seconds); use to step network at fixed rate.
    pub accumulator: f32,
    /// Step interval (seconds) for network updates (e.g. 1/3s).
    pub timeout: f32,
    /// Handlers for spawning entities by type name/key.
    pub spawn_handlers: HashMap<String, SpawnFn>,
    /// Handlers for despawning entities by type name/key.
    pub despawn_handlers: HashMap<String, DespawnFn>,
    /// Optional client-side interpolation hook (executed each network step).
    pub client_interpolation_hook: Option<InterpolationHookFn>,
}

impl PillTypeMapKey for NetworkManagerComponent {
    type Storage = GlobalComponentStorage<NetworkManagerComponent>;
}
impl GlobalComponent for NetworkManagerComponent {}

/// Target network **step frequency** (Hz) used by the engine systems.
///
/// This is separate from the render/frame rate.
const UPDATE_FREQUENCY_HZ: f32 = 3.0;
/// Derived fixed-step duration (seconds) for network updates.
const UPDATE_FREQUENCY_SEC: f32 = 1.0 / UPDATE_FREQUENCY_HZ;

impl NetworkManagerComponent {
    /// Create a new **server-side** network manager and bind the server.
    ///
    /// Calls `pill_core::networking::server_start(address, max_clients)` and
    /// initializes server state with default policy (`OfflinePolicy::Despawn`).
    ///
    /// # Errors
    /// Propagates address parsing, socket bind, and Netcode transport errors.
    ///
    /// # Example
    /// ```no_run
    /// # use pill_engine::ecs::components::network_manager_component::*;
    /// let network_manager = NetworkManagerComponent::new_server("0.0.0.0:9000", 64)?;
    /// # Ok::<_, anyhow::Error>(())
    /// ```
    pub fn new_server(address: &str, max_clients: usize) -> Result<Self> {
        let server = server_start(address, max_clients)?;
        Ok(Self {
            side: NetworkSide::Server(ServerState {
                net: server,
                world_epoch: 1,
                offline_policy: OfflinePolicy::Despawn,
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

    /// Create a new **client-side** network manager and connect to `address`.
    ///
    /// Calls `pill_core::networking::client_connect`, seeds connection state,
    /// and sets initial reconnect policy (disabled by default, zero next-try).
    ///
    /// # Errors
    /// Propagates address parsing, socket bind, and Netcode transport errors.
    ///
    /// # Example
    /// ```no_run
    /// # use pill_engine::ecs::components::network_manager_component::*;
    /// let network_manager = NetworkManagerComponent::new_client("127.0.0.1:9000", 42)?;
    /// # Ok::<_, anyhow::Error>(())
    /// ```
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

    /// Get mutable access to the **server** state if this instance is a server.
    #[inline]
    pub fn server_mut(&mut self) -> Option<&mut ServerState> {
        match &mut self.side { NetworkSide::Server(s) => Some(s), _ => None }
    }

    /// Get mutable access to the **client** state if this instance is a client.
    #[inline]
    pub fn client_mut(&mut self) -> Option<&mut ClientState> {
        match &mut self.side { NetworkSide::Client(c) => Some(c), _ => None }
    }

    /// Returns `true` if this engine instance is running as a **server**.
    #[inline]
    pub fn is_server(&self) -> bool {
        matches!(&self.side, NetworkSide::Server(_))
    }

    /// Returns `true` if this engine instance is running as a **client**.
    #[inline]
    pub fn is_client(&self) -> bool {
        matches!(&self.side, NetworkSide::Client(_))
    }
}

