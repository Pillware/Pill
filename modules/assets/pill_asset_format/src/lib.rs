//! The asset pack format's magic and version, shared by its writer and reader.
//!
//! # Responsibilities
//!
//! - Define the `PILLPACK` magic and the format version exactly once, for
//!   `pill_assets` (the writer, run from build scripts) and the engine's
//!   asset store (the reader, `pill_engine_core::asset_store`).
//!
//! # Design
//!
//! Deliberately dependency-free: `pill_assets` compiles into build scripts,
//! where every dependency is paid on each clean build, and the asset store
//! must not gain runtime dependencies through this crate. The pack's layout
//! is documented where it is read and written, so only the two values that
//! must agree byte-for-byte live here.

// =============================================================================
// Constants
// =============================================================================

/// The first bytes of every asset pack.
pub const ASSET_PACK_MAGIC: &[u8; 8] = b"PILLPACK";

/// The pack format version `pill_assets` writes and the asset store reads.
pub const ASSET_PACK_VERSION: u32 = 1;
