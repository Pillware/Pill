//! Services: what the engine reads from the outside world and offers to every
//! system.
//!
//! # Responsibilities
//!
//! - Hold the frame's inputs: the clock a frame measures itself with
//!   ([`time`]), and the keys, buttons and gamepads the frame was given
//!   ([`input`]).
//! - Hold the periodic ECS report a frame prints ([`diagnostics`]).
//! - Re-export the profiling API ([`profiling`]).
//!
//! # Design
//!
//! Each module keeps its old crate-root path: the crate re-exports every one of
//! them (`pub use services::{time, …}` in `lib.rs`), so moving a file into this
//! group changes no caller - not the facade's glob re-export and not a
//! `crate::input::Input` path inside the crate.
//!
//! A service is a resource or a free function, never state the engine owns
//! privately: a system reads the frame's time and input from the world. Drawing
//! is not a service: debug drawing and component gizmos live in the renderer
//! contract (`pill_renderer_api`), which is what a renderer draws and what the
//! engine has no vocabulary for.

/// A periodic report of ECS state, switched on with
/// `Engine::set_ecs_diagnostics_interval` and printed at that interval.
pub mod diagnostics;

/// Keyboard, mouse and gamepad state maintained by the engine and read through
/// `Res<Input>`.
pub mod input;

/// Re-exports the profiling API from `pill_core`.
pub mod profiling;

/// Frame timing maintained by the engine and read through `Res<Time>`.
pub mod time;
