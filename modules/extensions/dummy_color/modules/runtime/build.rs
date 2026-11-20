//! This crate's build script: generates the function-address inventory the host
//! needs to patch this crate's functions.
//!
//! # Responsibilities
//!
//! - Generate the function inventory: a Rust file that records, for every
//!   function a patch can reach, its qualified path and its address. The host
//!   uses them to point calls at a patched function, although nothing in the
//!   source marks a function as patchable.
//! - Leave the scanning to `pill_hot_scan`, which the host also runs to decide
//!   what it may patch, so both sides work from the same list of functions.
//
// REQUIREMENTS
//   Rust (stable). Cargo runs it as this crate's build script.
//
// DESCRIPTION
//   Generates the crate's function-address inventory into
//   `$OUT_DIR/function_inventory.rs`.
//
// USAGE
//   cargo build -p pill_dummy_color
//
// --- SCRIPT ---

fn main() {
    pill_hot_scan::generate_function_inventory();
}
