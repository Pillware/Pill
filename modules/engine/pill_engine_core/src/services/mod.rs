//! Services: what the engine reads from the outside world and offers to every
//! system.
//!
//! # Responsibilities
//!
//! - Hold the frame's inputs: the clock a frame measures itself with
//!   ([`time`]), and the keys, buttons and gamepads the frame was given
//!   ([`input`]).
//! - Hold the outputs a system draws with: immediate debug drawing
//!   ([`debug_draw`]), the gizmo pass that draws on a component's behalf
//!   ([`gizmos`]) and the periodic ECS report ([`diagnostics`]).
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
//! privately: a system reads the frame's time and input from the world, and
//! hands debug drawing to the world so whichever renderer runs can draw it.

/// Immediate debug drawing: lines, shapes, labels and gizmo handles. Any
/// system can draw them, and every renderer draws them.
pub mod debug_draw;

/// A periodic report of ECS state, switched on with
/// `Engine::set_ecs_diagnostics_interval` and printed at that interval.
pub mod diagnostics;

/// The pass that draws every component's own gizmo, together with the registry
/// it walks and the selection it draws for.
pub mod gizmos;

/// Keyboard, mouse and gamepad state maintained by the engine and read through
/// `Res<Input>`.
pub mod input;

/// Re-exports the profiling API from `pill_core`.
pub mod profiling;

/// Frame timing maintained by the engine and read through `Res<Time>`.
pub mod time;
