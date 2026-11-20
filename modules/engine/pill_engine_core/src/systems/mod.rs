//! How a plain function becomes a system, and how the frame runs it.
//!
//! # Responsibilities
//!
//! - Declare how a function's parameters are pulled from the world, so the
//!   function can run as a system ([`system`]).
//! - Order the frame's systems and decide which of them may run at the same
//!   time ([`scheduler`]).
//! - Declare the script lifecycle methods and the restricted context they run
//!   with ([`scripting`]).
//!
//! # Design
//!
//! Each module keeps its old crate-root path: the crate re-exports every one of
//! them (`pub use systems::{scheduler, …}` in `lib.rs`), so moving a file into
//! this group changes no caller - not the facade's glob re-export and not a
//! `crate::scripting::ScriptContext` path inside the crate.
//!
//! [`scripting`] is the trait that declares the lifecycle methods, and the
//! context they run with. The dispatch that decides when a lifecycle method
//! runs walks a world, so it lives beside the world (the `scripting` module
//! under [`crate::ecs::world`]).

/// Dependency analysis and parallel batch scheduling for system execution.
pub mod scheduler;

/// Script components: a component trait with an update step, and a context
/// that defers structural changes until it is safe to apply them.
pub mod scripting;

/// How a plain function's parameters are pulled from the world so the function
/// can run as a system.
pub mod system;
