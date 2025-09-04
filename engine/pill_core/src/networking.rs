use anyhow::{Context, Result};
use renet::{ ConnectionConfig, DefaultChannel, RenetClient, RenetServer, ServerEvent};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, ServerAuthentication, ServerConfig, NetcodeError, NetcodeTransportError};
use std::{net::{UdpSocket, SocketAddr, IpAddr, Ipv4Addr, Ipv6Addr}, time::{Duration, SystemTime}};
use std::io::ErrorKind;
use serde::{Serialize, Deserialize};
use crate::EngineError;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkAction {
    Update = 0,
    Join = 1,
    Exit = 2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitNotice {
    pub reason: String,
    pub when_ms: u64,
}

impl TryFrom<u8> for NetworkAction {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(NetworkAction::Update),
            1 => Ok(NetworkAction::Join),
            2 => Ok(NetworkAction::Exit),
            _ => Err(EngineError::InvalidNetworkAction(value).into()),
        }
    }
}

#[derive(Clone)]
pub struct NetworkPacket {
    pub tag: NetworkAction,
    pub data: Vec<u8>,
}

pub const RELIABLE_CHANNEL_ID: u8 = DefaultChannel::ReliableOrdered as u8;
pub const UNRELIABLE_CHANNEL_ID: u8 = DefaultChannel::Unreliable as u8;

#[derive(Debug)]
pub struct NetworkServer {
    pub server: RenetServer,
    pub transport: NetcodeServerTransport
}

#[derive(Debug)]
pub struct NetworkClient {
    pub client: RenetClient,
    pub transport: NetcodeClientTransport
}

fn now() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
}

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

    Ok(NetworkServer {
        server,
        transport,
    })
}

pub fn client_connect(bind: &str, client_id: u64) -> Result<NetworkClient> {
    let server_addr: SocketAddr = bind.parse()?;

	let local_bind = match server_addr {
        SocketAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0), // 0.0.0.0:0
        SocketAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0), // [::]:0
    };

    let socket = UdpSocket::bind(local_bind).with_context(|| format!("Client failed to bind local UDP socket at {}", local_bind))?;
    socket.set_nonblocking(true)?;

    let client = RenetClient::new(
        ConnectionConfig::default(),
    );

    let authentication = ClientAuthentication::Unsecure {
        server_addr,
        client_id,
        user_data: None,
        protocol_id: 0,
    };

    let transport = NetcodeClientTransport::new(now(), authentication, socket)?;

    Ok(NetworkClient {
        client,
        transport,
    })
}

pub fn client_disconnect(net: &mut NetworkClient) -> Result<()> {
    net.transport.disconnect();
    Ok(())
}

pub fn server_update(net: &mut NetworkServer, dt: Duration) -> Result<()> {
    net.server.update(dt);
    net.transport.update(dt, &mut net.server)?;
    Ok(())
}

pub fn server_get_events(net: &mut NetworkServer) -> Result<Vec<(u64, NetworkPacket)>> {
    let mut inbox = Vec::new();
    // handle connect/disconnect
    while let Some(e) = net.server.get_event() {
        match e {
            ServerEvent::ClientConnected { client_id }=> {
                log::info!("Client {client_id} connected");
                inbox.push((client_id, NetworkPacket { tag: NetworkAction::Join, data: Vec::new() }));
            },
            ServerEvent::ClientDisconnected{ client_id, reason} => {
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

pub fn client_update(net: &mut NetworkClient, dt: Duration) -> Result<()> {
    net.client.update(dt);
    net.transport.update(dt, &mut net.client)?;
    Ok(())
}

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

pub fn server_send_one(net: &mut NetworkServer, client_id: u64, msg: &NetworkPacket) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.server.send_message(client_id, RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

pub fn server_broadcast(net: &mut NetworkServer, msg: &NetworkPacket) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.server.broadcast_message(RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

pub fn server_broadcast_except(net: &mut NetworkServer, client_id: u64, msg: &NetworkPacket) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.server.broadcast_message_except(client_id, RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

pub fn server_broacast_exit(net: &mut NetworkServer, reason: &str) -> Result<()> {
    let notice = ExitNotice {
        reason: reason.to_string(),
        when_ms: now().as_millis() as u64,
    };
    let data = bincode::serialize(&notice)?;
    let msg = NetworkPacket {
        tag: NetworkAction::Exit,
        data,
    };
    server_broadcast(net, &msg)?;
    server_flush(net)?;
    Ok(())
}

pub fn server_dying_grasp(net: &mut NetworkServer, wait: Duration) -> Result<()> {
    let start = now();
    let step = Duration::from_millis(16);

    while now() - start < wait {
        //server_update(net, step)?;
        server_flush(net)?;
        std::thread::sleep(step);
    }

    Ok(())
}

pub fn client_send(net: &mut NetworkClient, msg: &NetworkPacket) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.client.send_message(RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

pub fn server_flush(net: &mut NetworkServer) -> Result<()> {
    net.transport.send_packets(&mut net.server);
    Ok(())
}

pub fn client_flush(net: &mut NetworkClient) -> Result<()> {
    net.transport.send_packets(&mut net.client)?;
    Ok(())
}

fn decode_wire(buf: &[u8]) -> Result<NetworkPacket> {
    let Some((tag_byte, data)) = buf.split_first() else {
        anyhow::bail!("Received empty message")
    };
    let tag = NetworkAction::try_from(*tag_byte)?;
    Ok(NetworkPacket {
        tag,
        data: data.to_vec(),
    })
}

