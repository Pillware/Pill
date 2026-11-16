//! Scenes: a world written as one file, and the references inside that file.
//!
//! # Responsibilities
//!
//! - Save a world's entities, components and references as one standalone
//!   `.scene` asset, and load it back ([`scene`]).
//! - Hold the saved form of a reference - a guid or a stable id, plus what a
//!   later run needs to find the target again ([`saved_references`]).
//! - Load the startup scene a project's settings name, before the project's
//!   own `init` fills the world ([`startup_scene`]).
//!
//! # Design
//!
//! Each module keeps its old crate-root path: the crate re-exports every one of
//! them (`pub use scene::{saved_references, startup_scene}` in `lib.rs`), so
//! moving a file into this group changes no caller.
//!
//! This group and the scene module inside it share a name, so the items of
//! [`scene`] are re-exported here as well: `crate::scene::Scene` is the path
//! callers have always used, and `crate::scene::scene::Scene` is the same type
//! by its new path.

/// The saved form of asset and entity references: a guid and a stable id, plus
/// what a later run needs to find the target again.
pub mod saved_references;

// The group is named after the module callers look for: the scene is what this
// folder is about, and `crate::scene::Scene` must resolve while the module sits
// in `scene/`. Clippy reads the repetition as an accident; it is deliberate.
#[allow(clippy::module_inception)]
/// A scene: a world's entities, components and references as one standalone
/// asset that loads back into a world.
pub mod scene;

/// The startup scene: the `.scene` a project's settings name, loaded before
/// the project's own `init` fills the world.
pub mod startup_scene;

// Current crate
pub use self::scene::*;
