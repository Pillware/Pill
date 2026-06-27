//! PillLauncher — CLI build orchestrator for the Pill game engine.
//!
//! Manages project scaffolding, asset pipelines, native/WASM builds,
//! benchmarking, and CI workflows for Pill-based game projects.

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
use actions::benchmarks::performance_benchmark::Benchmark;
use actions::benchmarks::size_benchmark::SizeBenchmark;
use actions::build::Build;
use actions::cargo_passthrough::Cargo;
use actions::checks::check_code::CheckCode;
use actions::checks::check_wasm_target::CheckWasm;
use actions::ci::Ci;
use actions::create::Create;
use actions::docs::Docs;
use actions::run::Run;
use actions::Action;

fn main() {
    let actions: [&dyn Action; 11] = [
        &Create,
        &Run,
        &Build,
        &Docs,
        &Cargo,
        &Assets,
        &CheckCode,
        &Benchmark,
        &SizeBenchmark,
        &CheckWasm,
        &Ci,
    ];

    if let Err(e) = utils::cli::run_app(&actions) {
        eprintln!("{:#}", e);
        std::process::exit(1);
    }
}
