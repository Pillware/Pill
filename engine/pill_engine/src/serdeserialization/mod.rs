#![cfg_attr(debug_assertions, allow(dead_code, unused_imports, unused_variables))]

mod serdeser;
mod serdeser_backend;

pub use serdeser::Serdeser;
pub use serdeser_backend::{JsonBackend, SerdeserBackend};
