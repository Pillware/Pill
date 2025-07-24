use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum Msg {
    Ping,
    // Add more later (Transform, Spawn, Input etc.)
}

pub trait NetworkTransport {
    fn poll(&mut self) -> Result<Vec<Msg>>;
    fn send(&mut self, msg: Msg) -> Result<()>;
}

// TODO dummy implementation before we implement Renet

pub struct DummyTransport;

impl NetworkTransport for DummyTransport {
    fn poll(&mut self) -> Result<Vec<Msg>> {
        // Simulate receiving a Ping message
        Ok(vec![Msg::Ping])
    }

    fn send(&mut self, msg: Msg) -> Result<()> {
        // Simulate sending a message
        println!("Sending message: {:?}", msg);
        Ok(())
    }
}

// TODO: helpers for serialization and deserialization of messages
pub fn serialize_msg(msg: &Msg) -> Result<Vec<u8>> {
    bincode::serialize(msg).map_err(Into::into)
}

pub fn deserialize_msg(data: &[u8]) -> Result<Msg> {
    bincode::deserialize(data).map_err(Into::into)
}
