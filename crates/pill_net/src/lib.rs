use anyhow::Result;
use renet::{ ConnectionConfig, DefaultChannel, RenetClient, RenetServer, ServerEvent};
use serde::{Deserialize, Serialize};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, ServerAuthentication, ServerConfig};
use std::{net::{UdpSocket, SocketAddr}, time::{Duration, SystemTime}};

#[derive(Debug, Serialize, Deserialize)]
pub enum Msg {
    Ping(u64),
    Pong(u64),
    Counter(u64),
    // Add more later (Transform, Spawn, Input etc.)
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

pub fn start_server(bind: &str, max_clients: usize) -> Result<NetServer> {
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

pub fn server_update(net: &mut NetServer, dt: Duration) -> Result<Vec<(u64, Msg)>> {
    net.server.update(dt);
    net.transport.update(dt, &mut net.server)?;

    // handle connect/disconnect
    while let Some(e) = net.server.get_event() {
        match e {
            ServerEvent::ClientConnected { client_id }=> {
                log::info!("Client {client_id} connected");
            },
            ServerEvent::ClientDisconnected{ client_id, reason} => {
                log::info!("Client {client_id} disconnected: {reason:?}");
            }
        }
    }

    let mut inbox = Vec::new();
    for cid in net.server.clients_id() {
        while let Some(bytes) = net.server.receive_message(cid, RELIABLE_CHANNEL_ID) {
            let msg = bincode::deserialize(&bytes)?;
            inbox.push((cid, msg));
        }
    }

    Ok(inbox)
}

pub fn connect_client(bind: &str, client_id: u64) -> Result<NetClient> {
    let server_addr: SocketAddr = bind.parse()?;
    let socket = UdpSocket::bind(server_addr)?;

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

pub fn client_update(net: &mut NetClient, dt: Duration) -> Result<Vec<Msg>> {
    net.client.update(dt);
    net.transport.update(dt, &mut net.client)?;

    if net.client.is_connected() {
        log::info!("Connected!");
    }

    let mut inbox = Vec::new();
    while let Some(bytes) = net.client.receive_message(RELIABLE_CHANNEL_ID) {
        let msg = bincode::deserialize(&bytes)?;
        inbox.push(msg);
    }
    Ok(inbox)
}

pub fn srv_send_one(net: &mut NetServer, client_id: u64, msg: &Msg) -> Result<()> {
    let bytes = bincode::serialize(&msg)?;
    net.server.send_message(client_id, RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

pub fn srv_broadcast(net: &mut NetServer, msg: &Msg) -> Result<()> {
    let bytes = bincode::serialize(&msg)?;
    net.server.broadcast_message(RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}

pub fn cli_send(net: &mut NetClient, msg: &Msg) -> Result<()> {
    let bytes = bincode::serialize(&msg)?;
    net.client.send_message(RELIABLE_CHANNEL_ID, bytes);
    Ok(())
}
