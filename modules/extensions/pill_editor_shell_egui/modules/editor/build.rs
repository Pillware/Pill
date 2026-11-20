//! This crate's build script: writes the crate's function-address inventory.
//!
//! # Responsibilities
//!
//! - Write the inventory: one entry for every function a patch can reach, with
//!   its qualified path and its address. The host reads it to redirect a call
//!   to the patched function, although nothing in the source marks a function
//!   as patchable.
//! - Leave the scanning to `pill_hot_scan`, which the host also runs to decide
//!   what it may patch. The two lists must match exactly, so the scan has one
//!   implementation.
//
// REQUIREMENTS
//   Rust (stable). Cargo runs it as this crate's build script.
//
// DESCRIPTION
//   Writes the inventory to `$OUT_DIR/function_inventory.rs`, which the crate
//   root pulls in with `include!`.
//
// USAGE
//   cargo build -p pill_editor_shell_egui_editor
//
// --- SCRIPT ---

fn main() {
    pill_hot_scan::generate_function_inventory();
}
