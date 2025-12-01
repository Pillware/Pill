//! # Networking Core (`pill_core::networking`)
//!
//! A small, façade around `renet`/`renet_netcode` providing a stable
//! API for PillEngine’s networking code. It exposes:
//! - **Server/Client wrappers** (`NetworkServer`, `NetworkClient`) with Netcode transport,
//! - a **packet format** (`NetworkPacket`) with a 1-byte [`NetworkAction`] tag,
//! - **tick/update** functions to drive the transports,
//! - **send/receive** helpers and event collection,
//! - a few ergonomic utilities (e.g., [`is_not_ready`]) for transient errors.
//!
//! ## Basic flow
//! 1. `server_start("0.0.0.0:9000", 64)`
//! 2. `client_connect("127.0.0.1:9000", client_id)`
//! 3. Each frame: `*_update(delta_time)` → pull `*_get_events()` → send with `*_send()` → `*_flush()`
//!
//! ## Channels
//! This module uses the **reliable, ordered** channel by default. The constants
//! [`RELIABLE_CHANNEL_ID`] and [`UNRELIABLE_CHANNEL_ID`] mirror `renet::DefaultChannel`.

use anyhow::{Context, Result};
use renet::{ ConnectionConfig, DefaultChannel, RenetClient, RenetServer, ServerEvent };
use renet_netcode::{
    ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, ServerAuthentication,
    ServerConfig, NetcodeError, NetcodeTransportError
};
use std::{
    net::{UdpSocket, SocketAddr, IpAddr, Ipv4Addr, Ipv6Addr},
    time::{Duration, SystemTime},
    io::ErrorKind,
};
use serde::{Serialize, Deserialize};
use crate::EngineError;

/// Logical tag prefixed to every reliable message on the wire (1 byte).
///
/// Encoded as `u8` in [`NetworkPacket`]. Extend carefully; unknown tags map
/// to [`EngineError::InvalidNetworkAction`].
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkAction {
    /// Gameplay/state update payload.
    Update = 0,
    /// A client has joined (emitted by server events).
    Join   = 1,
    /// A client/server is exiting; may include an [`ExitNotice`] payload.
    Exit   = 2,
}

/// Graceful-shutdown/exit metadata broadcast by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitNotice {
    /// Free-form human-readable reason.
    pub reason: String,
    /// UNIX epoch in milliseconds when the notice was generated (server time).
    pub when_ms: u64,
}

impl TryFrom<u8> for NetworkAction {
    type Error = anyhow::Error;

    /// Convert a wire `u8` into a [`NetworkAction`].
    ///
    /// Returns an error if the value is not a known action.
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(NetworkAction::Update),
            1 => Ok(NetworkAction::Join),
            2 => Ok(NetworkAction::Exit),
            _ => Err(EngineError::InvalidNetworkAction(value).into()),
        }
    }
}

/// Minimal wire message: a tag + opaque payload bytes.
///
/// The payload format is decided by the caller (e.g. `bincode`, `rkyv`, etc.).
#[derive(Clone)]
pub struct NetworkPacket {
    /// First byte on the wire.
    pub tag: NetworkAction,
    /// Opaque user payload.
    pub data: Vec<u8>,
}

/// Reliable/ordered channel used for gameplay messages.
pub const RELIABLE_CHANNEL_ID: u8 = DefaultChannel::ReliableOrdered as u8;
/// Unreliable channel (currently unused by helpers in this module).
pub const UNRELIABLE_CHANNEL_ID: u8 = DefaultChannel::Unreliable as u8;

/// Server wrapper (renet + Netcode) used by PillEngine subsystems.
#[derive(Debug)]
pub struct NetworkServer {
    pub server: RenetServer,
    pub transport: NetcodeServerTransport
}

/// Client wrapper (renet + Netcode) used by PillEngine subsystems.
#[derive(Debug)]
pub struct NetworkClient {
    pub client: RenetClient,
    pub transport: NetcodeClientTransport
}

/// Monotonic-ish time source for Netcode (UNIX epoch fallback to zero).
fn now() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
}

/// Returns `true` when `err` represents a **transient/not-ready** network state.
///
/// Useful to gate logs or transform errors that are expected during handshake,
/// reconnects, or nonblocking IO (e.g. `WouldBlock`, `Interrupted`,
/// `ClientNotConnected`, `Disconnected`).
#[inline]
pub fn is_not_ready(err: &anyhow::Error) -> bool {
    if let Some(e) = err.downcast_ref::<NetcodeTransportError>() {
        return match e {
            NetcodeTransportError::Netcode(ne) => {
                matches!(ne,
                    NetcodeError::ClientNotConnected |
                    NetcodeError::Disconnected(_)
                )
            }
            NetcodeTransportError::IO(ioe) => {
                matches!(ioe.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted)
            }
            _ => false,
        };
    }

    if let Some(ne) = err.downcast_ref::<NetcodeError>() {
        return matches!(ne,
            NetcodeError::ClientNotConnected |
            NetcodeError::Disconnected(_)
        );
    }
    false
}

/// Start a UDP/Netcode server and begin listening.
///
/// Binds `bind` (e.g. `"0.0.0.0:9000"`), sets nonblocking, configures Netcode,
/// and returns a ready [`NetworkServer`]. Call [`server_update`] every tick.
///
/// # Errors
/// - Address parse/bind errors,
/// - Netcode transport initialization errors.
///
/// # Example
/// ```no_run
/// # use pill_core::networking::*;
/// let mut server = server_start("0.0.0.0:9000", 64)?;
/// loop {
///     server_update(&mut server, std::time::Duration::from_millis(16))?;
///     for (cid, pkt) in server_get_events(&mut server)? {
///         // handle client joins/exits/updates...
///     }
///     server_flush(&mut server)?;
/// }
/// # Ok::<_, anyhow::Error>(())
/// ```
pub fn server_start(bind: &str, max_clients: usize) -> Result<NetworkServer> {
    let address: SocketAddr = bind.parse()?;

    let socket = UdpSocket::bind(address)?;
    socket.set_nonblocking(true)?;

    let server = RenetServer::new(ConnectionConfig::default());
    let server_config = ServerConfig {
        current_time: now(),
        max_clients,
        protocol_id: 0,
        public_addresses: vec![address],
        authentication: ServerAuthentication::Unsecure,
    };
    let transport = NetcodeServerTransport::new(server_config, socket)?;

    log::info!("Server started at {address}, max clients: {max_clients}");

    Ok(NetworkServer { server, transport })
}

/// Create a UDP/Netcode client and connect to `bind` (server address).
///
/// Binds an ephemeral local UDP port, enables nonblocking IO, creates a
/// `RenetClient` with default channels, and starts an **Unsecure** Netcode
/// handshake towards `server_addr`.
///
/// Call [`client_update`] every tick; use [`client_get_events`] to pull messages.
///
/// # Errors
/// - Invalid server address,
/// - Local socket bind failure,
/// - Netcode transport initialization errors.
/// # Example
/// ```no_run
/// # use pill_core::networking::*;
/// let client_id = 12345; // Unique per client_id
/// let mut client = client_connect("192.168.1.0:9000", client_id)?;
/// loop {
///    client_update(&mut client, std::time::Duration::from_millis(16))?;
///    // send own updates to the server
///    // client_send(&mut client, &my_packet)?;
///    for pkt in client_get_events(&mut client)? {
    ///    // handle server updates...
///    }
///    client_flush(&mut client)?;
/// }
/// # Ok::<_, anyhow::Error>(())
/// ```
pub fn client_connect(bind: &str, client_id: u64) -> Result<NetworkClient> {
    let server_addr: SocketAddr = bind.parse()?;

    let local_bind = match server_addr {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0), // 0.0.0.0:0
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0), // [::]:0
    };

    let socket = UdpSocket::bind(local_bind)
        .with_context(|| format!("Client failed to bind local UDP socket at {}", local_bind))?;
    socket.set_nonblocking(true)?;

    let client = RenetClient::new(ConnectionConfig::default());

    let authentication = ClientAuthentication::Unsecure {
        server_addr,
        client_id,
        user_data: None,
        protocol_id: 0,
    };

    let transport = NetcodeClientTransport::new(now(), authentication, socket)?;

    Ok(NetworkClient { client, transport })
}

/// Request a client disconnect (immediate).
pub fn client_disconnect(net: &mut NetworkClient) -> Result<()> {
    net.transport.disconnect();
    Ok(())
}

/// Drive the server state machine by `delta_time`.
///
/// Must be called regularly (e.g. once per frame). This updates renet
/// and then pumps the Netcode transport to process IO.
pub fn server_update(net: &mut NetworkServer, delta_time: Duration) -> Result<()> {
    net.server.update(delta_time);
    net.transport.update(delta_time, &mut net.server)?;
    Ok(())
}

/// Disconnect a single client by ID (server-side).
pub fn server_disconnect_client(net: &mut NetworkServer, client_id: u64) -> Result<()> {
    net.server.disconnect(client_id);
    Ok(())
}

/// Poll server events and read incoming reliable messages from all clients.
///
/// Returns a batch of `(client_id, NetworkPacket)` where packets are:
/// - **Join/Exit** synthesized from `ServerEvent`s,
/// - **Update** decoded from the **reliable** channel.
///
/// Empty messages are ignored.
///
/// # Errors
/// - Decoding errors (unknown tag),
/// - Any `anyhow` errors surfaced by helpers.
pub fn server_get_events(net: &mut NetworkServer) -> Result<Vec<(u64, NetworkPacket)>> {
    let mut inbox = Vec::new();
    // handle connect/disconnect
    while let Some(e) = net.server.get_event() {
        match e {
            ServerEvent::ClientConnected { client_id } => {
                log::info!("Client {client_id} connected");
                inbox.push((client_id, NetworkPacket { tag: NetworkAction::Join, data: Vec::new() }));
            },
            ServerEvent::ClientDisconnected { client_id, reason } => {
                log::info!("Client {client_id} disconnected: {reason:?}");
                inbox.push((client_id, NetworkPacket { tag: NetworkAction::Exit, data: Vec::new() }));
            }
        }
    }

    for cid in net.server.clients_id() {
        while let Some(bytes) = net.server.receive_message(cid, RELIABLE_CHANNEL_ID) {
            if bytes.is_empty() {
                continue; // Skip empty messages
            }
            inbox.push((cid, decode_wire(&bytes)?));
        }
    }

    Ok(inbox)
}

/// Drive the client state machine by `delta_time`.
///
/// Must be called regularly (e.g. once per frame) to advance renet and
/// Netcode, handle timeouts, etc.
pub fn client_update(net: &mut NetworkClient, delta_time: Duration) -> Result<()> {
    net.client.update(delta_time);
    net.transport.update(delta_time, &mut net.client)?;
    Ok(())
}

/// Drain all incoming reliable messages for this client.
///
/// # Returns
/// Vector of decoded [`NetworkPacket`] values.
///
/// # Errors
/// - Decoding errors (unknown tag) per message.
pub fn client_get_events(net: &mut NetworkClient) -> Result<Vec<NetworkPacket>> {
    let mut inbox = Vec::new();
    while let Some(bytes) = net.client.receive_message(RELIABLE_CHANNEL_ID) {
        if bytes.is_empty() {
            continue; // Skip empty messages
        }
        inbox.push(decode_wire(&bytes)?);
    }
    Ok(inbox)
}

/// Send one reliable message to a specific client.
pub fn server_send_one(net: &mut NetworkServer, client_id: u64, msg: &NetworkPacket) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.server.send_message(client_id, RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

/// Broadcast a reliable message to **all** connected clients.
pub fn server_broadcast(net: &mut NetworkServer, msg: &NetworkPacket) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.server.broadcast_message(RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

/// Broadcast a reliable message to **all except** `client_id`.
pub fn server_broadcast_except(net: &mut NetworkServer, client_id: u64, msg: &NetworkPacket) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.server.broadcast_message_except(client_id, RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

/// Broadcast a graceful-shutdown notice to all clients and flush immediately.
///
/// Sends a [`NetworkAction::Exit`] with a serialized [`ExitNotice`], then
/// calls [`server_flush`] to push packets to the socket.
pub fn server_broadcast_exit(net: &mut NetworkServer, reason: &str) -> Result<()> {
    let notice = ExitNotice {
        reason: reason.to_string(),
        when_ms: now().as_millis() as u64,
    };
    let data = bincode::serialize(&notice)?;
    let msg = NetworkPacket { tag: NetworkAction::Exit, data };
    server_broadcast(net, &msg)?;
    server_flush(net)?;
    Ok(())
}

/// Keep flushing for `wait` duration — a “dying grasp”.
///
/// Helpful during shutdown to increase the chance that final packets make it out.
/// Sleeps in ~16ms steps and calls [`server_flush`] repeatedly.
pub fn server_dying_grasp(net: &mut NetworkServer, wait: Duration) -> Result<()> {
    let start = now();
    let step = Duration::from_millis(16);

    while now() - start < wait {
        // server_update(net, step)?; // optional: drive timeouts during shutdown
        server_flush(net)?;
        std::thread::sleep(step);
    }

    Ok(())
}

/// Send one reliable message from the client to the server.
pub fn client_send(net: &mut NetworkClient, msg: &NetworkPacket) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.client.send_message(RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

/// Push all pending server packets down to the socket.
///
/// Transport `send_packets` consumes packets from renet and writes to UDP.
/// This variant does not surface IO errors (mirrors `NetcodeServerTransport`).
pub fn server_flush(net: &mut NetworkServer) -> Result<()> {
    net.transport.send_packets(&mut net.server);
    Ok(())
}

/// Push all pending client packets down to the socket.
///
/// Returns an error if the underlying transport fails.
pub fn client_flush(net: &mut NetworkClient) -> Result<()> {
    net.transport.send_packets(&mut net.client)?;
    Ok(())
}

/// Decode a single wire message (`tag | payload…`) into a [`NetworkPacket`].
///
/// # Errors
/// - Empty message,
/// - Unknown tag (propagates [`EngineError::InvalidNetworkAction`]).
fn decode_wire(buf: &[u8]) -> Result<NetworkPacket> {
    let Some((tag_byte, data)) = buf.split_first() else {
        anyhow::bail!("Received empty message")
    };
    let tag = NetworkAction::try_from(*tag_byte)?;
    Ok(NetworkPacket { tag, data: data.to_vec() })
}

