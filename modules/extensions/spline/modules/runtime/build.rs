//! This crate's build script: writes the list of functions the host can
//! hot-patch in this crate.
//!
//! # Responsibilities
//!
//! - Write the crate's function list into `$OUT_DIR/function_inventory.rs`.
//   For each function a patch can reach it records the qualified path, the
//   address and the declaration, so the host can send calls to a patched copy.
//   Nothing in the crate's source marks a function as patchable.
// - Leave the scan to `pill_hot_scan`. The host runs the same scan to decide
//   what it may patch, so the two lists must match exactly and the scan exists
//   only once.
//
// REQUIREMENTS
//   Rust (stable). Cargo runs it as this crate's build script.
//
// DESCRIPTION
//   Writes the crate's function list into `$OUT_DIR/function_inventory.rs`, a
//   file the crate root includes. Above `opt-level = 0` the file is written
//   empty on purpose: the compiler merges functions with identical bodies into
//   one, and patching such a function would make it jump to itself.
//
// USAGE
//   cargo build -p pill_spline
//
// --- SCRIPT ---

fn main() {
    pill_hot_scan::generate_function_inventory();
}
