//! PillLauncher - CLI build orchestrator for the Pill pill project engine.
//!
//! Manages project scaffolding, asset pipelines, native/WASM builds,
//! and documentation generation for Pill-based pill projects.

// This file is the crate root for PillLauncher.
//
// Responsibilities:
// - Declares all top-level modules.
// - Instantiates all CLI action objects.
// - Passes them to the generic dispatcher in utils::cli.

#![deny(clippy::all)]
#![warn(missing_docs)]
#![warn(clippy::clone_on_copy)]

mod actions;
mod types;
mod utils;

use actions::assets::Assets;
use actions::build::Build;
use actions::cargo_passthrough::Cargo;
use actions::create::Create;
use actions::docs::Docs;
use actions::link::{Link, Unlink};
use actions::run::Run;
use actions::Action;

fn main() {
    let actions: [&dyn Action; 8] = [
        &Create, &Run, &Build, &Docs, &Cargo, &Assets, &Link, &Unlink,
    ];

    if let Err(e) = utils::cli::run_app(&actions) {
        eprintln!("{:#}", e);
        std::process::exit(1);
    }
}
