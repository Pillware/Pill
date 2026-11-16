//! The asset pipeline: what an asset is, where its bytes come from, and how a
//! later run finds it again.
//!
//! # Responsibilities
//!
//! - Hold the asset trait, the generational handle that names one, and the
//!   manager that stores live assets ([`storage`]).
//! - Hold every asset of a project by guid and by path ([`database`]).
//! - Read and write the files an asset is saved in: the YAML document format
//!   ([`document`]), the `.meta` sidecar beside a source file ([`metadata`])
//!   and the standalone asset file with no source ([`standalone`]).
//! - Resolve a path to bytes through mounted packs and the filesystem
//!   ([`store`]), and load by path from game code ([`loading`]).
//! - Import by file extension when the type is unknown ([`import_registry`]),
//!   and save a reference to an asset that outlives the run ([`reference`]).
//!
//! # Design
//!
//! Each module keeps its old crate-root path: the crate re-exports every one of
//! them (`pub use assets::{storage as asset, …}` in `lib.rs`), so moving a file
//! into this group changes no caller - not the facade's glob re-export and not
//! a `crate::asset_store::read` path inside the crate.
//!
//! The files are named after the part of the pipeline they own, not after the
//! `asset_` prefix every one of them carried at the crate root: `storage` is
//! the store of live assets, `store` is where bytes come from.

/// Every asset of a project by guid and by path, loaded or not.
pub mod database;

/// The YAML document format every asset file shares.
pub mod document;

/// Importing assets by file extension, for code that does not know their type.
pub mod import_registry;

/// Loading assets and scenes from game code by path.
pub mod loading;

/// Metadata files beside an asset's source (`<source_file>.meta`).
pub mod metadata;

/// Saved references to assets, by guid, that point at the asset in a later
/// run.
pub mod reference;

/// Assets stored as their own file in `res`, with no source file.
pub mod standalone;

/// Many-per-type asset storage addressed by generational handle.
///
/// The counterpart to resource storage for data a world holds several of -
/// meshes, textures, materials - where the type alone does not name one value.
/// The store is itself a resource, so it reaches systems through the `Res` /
/// `ResMut` parameters.
pub mod storage;

/// Where an asset's bytes come from: mounted packs and the filesystem.
pub mod store;
