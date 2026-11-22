//! The master renderer's draw paths.
//!
//! # Responsibilities
//!
//! - Hold one module per draw path. [`mesh_drawer`] is the mesh path: it
//!   batches the instances a frame queues and records their instanced draws
//!   in the render pass.

/// The mesh drawer: batches the frame's queued instances and records their
/// instanced draws.
pub(crate) mod mesh_drawer;
