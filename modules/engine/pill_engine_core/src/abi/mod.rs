//! The ABI surface: what a loaded DLL may name across the boundary.
//!
//! # Responsibilities
//!
//! - Declare the API table a loaded project or module receives at its entry
//!   point, and the calls it carries ([`api`]).
//! - Declare the ABI revision the host and every module compare before either
//!   trusts the other ([`module_abi`]).
//! - Declare the C ABI a mirrored Rust function crosses to reach C#
//!   ([`mirror`]).
//!
//! # Design
//!
//! Each module keeps its old crate-root path: the crate re-exports every one of
//! them (`pub use abi::{api, …}` in `lib.rs`), so moving a file into this group
//! changes no caller - not the facade's glob re-export and not a
//! `crate::module_abi::MODULE_ABI_VERSION` path inside the crate.
//!
//! Everything here is plain data and `extern "C"` signatures: no ring buffer,
//! no registry and no pointer into the engine's own state, because a value that
//! crosses this boundary outlives the image that built it.

/// The engine API table a loaded project or module receives at its entry
/// point.
pub mod api;

/// The C ABI a mirrored Rust function crosses to reach C#: argument slots,
/// result encodings, and the channels that carry an error message or a
/// returned string.
pub mod mirror;

/// The extension ABI revision the host and every module compare at load time.
pub mod module_abi;
