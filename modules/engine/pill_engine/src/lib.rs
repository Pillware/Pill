//! The Pill engine, as projects and extensions see it.
//!
//! # Responsibilities
//!
//! - Re-export the engine core (`pill_engine_core`) under the same paths, so
//!   `use pill_engine::*` and every `::pill_engine::...` path the macros emit
//!   resolve.
//! - Own the per-DLL `inventory` registries ([`component_registry`],
//!   [`declarations`], [`hot_patch`]) and re-export the hot-patch descriptor
//!   types at the crate root.
//! - Re-export the `submit!` macro that the derive and attribute macros
//!   expand to.
//!
//! # Design
//!
//! This crate is an `rlib`, so every DLL gets its own copy. The engine core
//! holds everything that is the same for every DLL. The registries live here
//! because each question they answer is about one DLL: which components this
//! artifact declares, and where a function's address is in this artifact.
//! Each DLL's copy of this crate therefore reads only that DLL's own
//! declarations. The core never reads a registry and never depends on this
//! crate; code here passes the core what it needs from one.

// Makes `::pill_engine::...` resolve inside this crate. The derive and
// attribute macros expand to that path form.
extern crate self as pill_engine;

/// Everything in the engine core, re-exported under this crate's paths.
pub use pill_engine_core::*;

/// Per-DLL component, value-type, method, accessor and export registries.
pub mod component_registry;

/// The systems, resources and asset types an artifact declares, registered
/// from its entry point.
pub mod declarations;

/// The per-DLL hot-patch registries, built on the core's hot-patch machinery.
pub mod hot_patch;

// The hot-patch descriptor types, also reachable at the crate root so a
// caller need not name the `hot_patch` module.
pub use hot_patch::{PillHotFunctionDescriptor, PillHotSlotDescriptor};

// Re-exports the `inventory` `submit!` macro that the derive and attribute
// macros expand to. Module and project crates therefore need no dependency
// beyond `pill_engine` itself.
pub use inventory::submit;
