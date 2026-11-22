//! This crate's build script: writes the crate's function-address inventory.
//!
//! # Responsibilities
//!
//! - Write the inventory the host reads to patch any function of this crate
//!   in place. The scanning itself is `pill_hot_scan`'s, which the host also
//!   runs, so the two lists always match.
//
// REQUIREMENTS
//   Rust (stable). Cargo runs it as this crate's build script.
//
// DESCRIPTION
//   Writes the inventory to `$OUT_DIR/function_inventory.rs`, which the crate
//   root pulls in with `include!`.
//
// USAGE
//   cargo build -p pill_blender_link_editor
//
// EXAMPLE USAGE
//   cargo check -p pill_blender_link_editor
//
// --- SCRIPT ---

fn main() {
    pill_hot_scan::generate_function_inventory();
}
