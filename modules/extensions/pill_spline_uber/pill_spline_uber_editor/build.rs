//! This crate's build script: writes the list of functions the host can
//! hot-patch in this crate.
//!
//! # Responsibilities
//!
//! - Write the crate's function list into `$OUT_DIR/function_inventory.rs`, so
//!   an edit to the extension's hooks patches in place.
//! - Leave the scan to `pill_hot_scan`, the one implementation the host runs
//!   too.
//
// REQUIREMENTS
//   Rust (stable). Cargo runs it as this crate's build script.
//
// DESCRIPTION
//   Writes the crate's function list into `$OUT_DIR/function_inventory.rs`, a
//   file the crate root includes.
//
// USAGE
//   cargo build -p pill_spline_uber_editor
//
// --- SCRIPT ---

fn main() {
    pill_hot_scan::generate_function_inventory();
}
