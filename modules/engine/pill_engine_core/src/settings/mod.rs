//! Settings: the project's declared entries and the values a run resolved.
//!
//! # Responsibilities
//!
//! - Hold the project model: a declared schema per entry, the sparse documents
//!   a project writes, the environment over them, and the one resolution that
//!   composes them ([`project`]).
//!
//! # Design
//!
//! A module declares its settings in its own crate and registers the surface
//! with the engine; the host resolves the project's documents against every
//! declared schema and hands each manager its values. Nothing here reads a
//! file: the documents arrive from the host, so a test can resolve one without
//! a process environment or a disk.

/// The project's settings: a declared schema per entry, the sparse documents a
/// project writes, and the one resolution that composes them.
pub mod project;

pub use self::project::*;
