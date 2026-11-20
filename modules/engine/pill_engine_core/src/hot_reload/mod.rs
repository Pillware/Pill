//! The hot-reload machinery: what survives a swapped DLL and what carries a
//! patch into it.
//!
//! # Responsibilities
//!
//! - Give a registered system a stable dispatch slot a patch can redirect
//!   ([`hot_patch`]).
//! - Keep component data readable across a reload: the persistence registry,
//!   the schema hashes and the migration that moves a row from an older layout
//!   ([`persistence`]).
//!
//! # Design
//!
//! Each module keeps its old crate-root path: the crate re-exports both
//! (`pub use hot_reload::{hot_patch, persistence}` in `lib.rs`), so moving a
//! file into this group changes no caller.
//!
//! The two belong together because they answer one question from two sides: a
//! patched function keeps running while the data it reads was built by an older
//! image, and the persisted type names are what a later generation registers
//! against. Neither may hold a pointer into an image that a reload can unmap,
//! so both store owned text and plain numbers.

/// Per-function hot patching: stable dispatch slots for registered systems.
pub mod hot_patch;

/// Component persistence and schema migration for hot-reload.
pub mod persistence;
