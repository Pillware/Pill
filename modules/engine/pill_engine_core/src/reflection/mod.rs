//! What the engine knows about a value's shape without naming its type.
//!
//! # Responsibilities
//!
//! - Declare the field layouts the derive macros emit, and the resolvers that
//!   find the layout of a nested value type ([`component_registry`]).
//! - Read and write one field of a component row through that layout
//!   ([`component_field`]).
//! - Spell a type's public name, `pill_engine::...`, for anything the engine
//!   stores as data ([`type_names`]).
//!
//! # Design
//!
//! Each module keeps its old crate-root path: the crate re-exports every one of
//! them (`pub use reflection::{component_field, …}` in `lib.rs`), so moving a
//! file into this group changes no caller - not the facade's glob re-export and
//! not a `crate::type_names::stable_type_name` path inside the crate.
//!
//! Nothing here registers anything. The derives write their layouts into the
//! facade's per-DLL registries; this group holds the types those registries and
//! the world share, plus the code that walks a layout.

/// Generic type-erased component field access for editor-style tools.
pub mod component_field;

/// Compile-time field layouts of components and value types, emitted by the
/// derive macros.
pub mod component_registry;

/// Type names as the rest of the engine sees them: `pill_engine::...` for the
/// engine's own types.
pub mod type_names;
