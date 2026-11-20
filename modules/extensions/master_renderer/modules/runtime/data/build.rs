//! Cooks the renderer's shaders before the crate itself compiles.
//!
//! # Responsibilities
//!
//! - Cook every HLSL source under `shader_sources/` to the WGSL the crate
//!   embeds. The cook is the same routine the development host's shader
//!   watcher runs, so a shader is cooked the same way at build time and at run
//!   time.
//! - Tell cargo which directories, headers and sources to watch, so that a
//!   shader edit rebuilds the crate.
//!
//! # Design
//!
//! Cargo runs build scripts before the crate's own compilation, so the cooked
//! files are on disk by the time `include_str!` looks for them.
//!
//! This file does not list which directories hold sources and which hold
//! headers. `pill_assets`' shader-tree convention decides that
//! ([`pill_assets::cook_shader_tree`]), so the build and a runtime cook always
//! look at the same files.
//!
//! Cargo cannot watch a glob, so a list of files written now would miss a
//! `.hlsl` that does not exist yet. Watching a directory catches a file added
//! inside it, and watching each file catches edits to that file.

// Standard library
use std::path::PathBuf;

fn main() {
    // Step 1: Build the path to the shader tree. It sits at the crate root
    // instead of under `src/`, because the development host rebuilds this
    // module when a file under `src/` changes. A shader edit reaches the
    // running game through the shader watcher instead.
    let shaders_directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shader_sources");

    println!("cargo:rerun-if-changed=build.rs");

    // Step 2: Cook every source that has no up-to-date output. A cook that
    // fails stops the build.
    let report = pill_assets::cook_shader_tree(&shaders_directory)
        .unwrap_or_else(|error| panic!("shader cooking failed: {error}"));

    // Step 3: Tell cargo what to watch. A watched directory reports a file
    // added inside it, and a watched file reports an edit to that file.
    // Headers go in too: every source can include them through the include
    // path, and cargo's directives take no glob.
    for directory in report.layout.watched_directories() {
        println!("cargo:rerun-if-changed={}", directory.display());
    }
    for header_path in &report.header_files {
        println!("cargo:rerun-if-changed={}", header_path.display());
    }
    for source_path in &report.discovered_sources {
        println!("cargo:rerun-if-changed={}", source_path.display());
    }
}
