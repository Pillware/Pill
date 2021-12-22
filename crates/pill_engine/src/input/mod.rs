#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

mod input_event;
mod input_component;
mod input_system;

pub use input_event::InputEvent;
pub use input_component::*;
pub use input_system::*;