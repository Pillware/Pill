use anyhow::{Context, Result};
use renet::{ ConnectionConfig, DefaultChannel, RenetClient, RenetServer, ServerEvent};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, ServerAuthentication, ServerConfig};
use std::{net::{UdpSocket, SocketAddr, IpAddr, Ipv4Addr, Ipv6Addr}, time::{Duration, SystemTime}};
use serde::{Serialize, Deserialize};

#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireTag {
    Update = 0,
    Join = 1,
    Exit = 2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExitNotice {
    pub reason: String,
    pub when_ms: u64,
}

impl TryFrom<u8> for WireTag {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(WireTag::Update),
            1 => Ok(WireTag::Join),
            2 => Ok(WireTag::Exit),
            _ => Err(anyhow::anyhow!("Invalid WireTag byte: {}", value)),
        }
    }
}

#[derive(Clone)]
pub struct WireMsg {
    pub tag: WireTag,
    pub data: Vec<u8>,
}

pub const RELIABLE_CHANNEL_ID: u8 = DefaultChannel::ReliableOrdered as u8;
pub const UNRELIABLE_CHANNEL_ID: u8 = DefaultChannel::Unreliable as u8;

pub struct NetServer {
    pub server: RenetServer,
    pub transport: NetcodeServerTransport
}

pub struct NetClient {
    pub client: RenetClient,
    pub transport: NetcodeClientTransport
}

fn now() -> Duration {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
}

pub fn srv_start(bind: &str, max_clients: usize) -> Result<NetServer> {
    let addr: SocketAddr = bind.parse()?;
    let socket = UdpSocket::bind(addr)?;
    socket.set_nonblocking(true)?;

    let server = RenetServer::new(ConnectionConfig::default());
    let server_config = ServerConfig {
        current_time: now(),
        max_clients,
        protocol_id: 0,
        public_addresses: vec![addr],
        authentication: ServerAuthentication::Unsecure,
    };
    let transport = NetcodeServerTransport::new(server_config, socket)?;

    log::info!("Server started at {addr}, max clients: {max_clients}");

    Ok(NetServer {
        server,
        transport,
    })
}

pub fn cli_connect(bind: &str, client_id: u64) -> Result<NetClient> {
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

    Ok(NetClient {
        client,
        transport,
    })
}

pub fn srv_update(net: &mut NetServer, dt: Duration) -> Result<()> {
    net.server.update(dt);
    net.transport.update(dt, &mut net.server)?;
    Ok(())
}

pub fn srv_get_events(net: &mut NetServer) -> Result<Vec<(u64, WireMsg)>> {
    let mut inbox = Vec::new();
    // handle connect/disconnect
    while let Some(e) = net.server.get_event() {
        match e {
            ServerEvent::ClientConnected { client_id }=> {
                log::info!("Client {client_id} connected");
            },
            ServerEvent::ClientDisconnected{ client_id, reason} => {
                log::info!("Client {client_id} disconnected: {reason:?}");
                inbox.push((client_id, WireMsg { tag: WireTag::Exit, data: Vec::new() }));
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

pub fn cli_update(net: &mut NetClient, dt: Duration) -> Result<()> {
    net.client.update(dt);
    net.transport.update(dt, &mut net.client)?;
    Ok(())
}

pub fn cli_get_events(net: &mut NetClient) -> Result<Vec<WireMsg>> {
    let mut inbox = Vec::new();
    while let Some(bytes) = net.client.receive_message(RELIABLE_CHANNEL_ID) {
        if bytes.is_empty() {
            continue; // Skip empty messages
        }
        inbox.push(decode_wire(&bytes)?);
    }
    Ok(inbox)
}

pub fn srv_send_one(net: &mut NetServer, client_id: u64, msg: &WireMsg) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.server.send_message(client_id, RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

pub fn srv_broadcast(net: &mut NetServer, msg: &WireMsg) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.server.broadcast_message(RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

pub fn srv_broadcast_except(net: &mut NetServer, client_id: u64, msg: &WireMsg) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.server.broadcast_message_except(client_id, RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

pub fn srv_broacast_exit(net: &mut NetServer, reason: &str) -> Result<()> {
    let notice = ExitNotice {
        reason: reason.to_string(),
        when_ms: now().as_millis() as u64,
    };
    let data = bincode::serialize(&notice)?;
    let msg = WireMsg {
        tag: WireTag::Exit,
        data,
    };
    srv_broadcast(net, &msg);
    srv_flush(net)?;
    Ok(())
}

pub fn srv_dying_grasp(net: &mut NetServer, wait: Duration) -> Result<()> {
    let start = now();
    let step = Duration::from_millis(16);

    while now() - start < wait {
        srv_update(net, step)?;
        srv_flush(net)?;
        std::thread::sleep(step);
    }

    Ok(())
}

pub fn cli_send(net: &mut NetClient, msg: &WireMsg) -> Result<()> {
    let mut bytes = Vec::with_capacity(1 + msg.data.len());
    bytes.push(msg.tag as u8);
    bytes.extend_from_slice(&msg.data);
    net.client.send_message(RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

pub fn srv_flush(net: &mut NetServer) -> Result<()> {
    net.transport.send_packets(&mut net.server);
    Ok(())
}

pub fn cli_flush(net: &mut NetClient) -> Result<()> {
    net.transport.send_packets(&mut net.client)?;
    Ok(())
}

fn decode_wire(buf: &[u8]) -> Result<WireMsg> {
    let Some((tag_byte, data)) = buf.split_first() else {
        anyhow::bail!("Received empty message")
    };
    let tag = WireTag::try_from(*tag_byte)?;
    Ok(WireMsg {
        tag,
        data: data.to_vec(),
    })
}

