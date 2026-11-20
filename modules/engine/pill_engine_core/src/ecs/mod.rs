//! The ECS half of the crate: the data model every other module is built on.
//!
//! # Responsibilities
//!
//! - Hold the entities ([`entity`]), components ([`component`]), resources
//!   ([`resource`]) and queued commands ([`commands`]) a world stores.
//! - Hold the storage they live in: the [`archetype`] columns, the [`world`]
//!   that owns them, and the [`query`] layer that reads them.
//! - Hold the components every world needs ([`common_components`]).
//!
//! # Design
//!
//! Each module keeps its old crate-root path: the crate re-exports every one of
//! them (`pub use ecs::{archetype, …}` in `lib.rs`), so moving a file into this
//! group changes no caller - not the facade's glob re-export and not a
//! `crate::entity::Entity` path inside the crate.

/// Archetype-based component storage with structure-of-arrays layout.
pub mod archetype;

/// Deferred command queue for structural ECS mutations.
pub mod commands;

/// Components the engine defines because more than one consumer needs them.
pub mod common_components;

/// Component trait, type identification, and change-detection primitives.
pub mod component;

/// Lightweight entity handles with generation-based invalidation.
pub mod entity;

/// Query system for efficient iteration over entities with specific components.
pub mod query;

/// Singleton resources stored in the [`world::World`], not attached to
/// entities.
pub mod resource;

/// Central ECS state container - entities, archetypes, components, and
/// resources.
pub mod world;
