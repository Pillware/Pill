#![cfg_attr(debug_assertions, allow(dead_code, unused_imports))]

pub(crate) mod input_event;
pub(crate) mod input_component;
pub(crate) mod input_system;

pub use input_event::InputEvent;
pub use input_component::InputComponent;
pub use input_system::*;