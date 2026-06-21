// This file is the crate root for PillLauncher.
//
// Responsibilities:
// - Declares all top-level modules.
// - Instantiates all CLI action objects.
// - Passes them to the generic dispatcher in utils::cli.

// PillLauncher uses PascalCase for the binary name — allow it without global suppression.
#![allow(non_snake_case)]

mod actions;
mod types;
mod utils;

use actions::assets::Assets;
use actions::build::{Build, Run};
use actions::cargo_passthrough::Cargo;
use actions::check::Check;
use actions::check_wasm::CheckWasm;
use actions::ci::Ci;
use actions::create::Create;
use actions::docs::Docs;
use actions::performance_benchmark::Benchmark;
use actions::size_benchmark::SizeBenchmark;
use actions::Action;

fn main() {
    let actions: Vec<Box<dyn Action>> = vec![
        Box::new(Create),
        Box::new(Run),
        Box::new(Build),
        Box::new(Docs),
        Box::new(Cargo),
        Box::new(Assets),
        Box::new(Check),
        Box::new(Benchmark),
        Box::new(SizeBenchmark),
        Box::new(CheckWasm),
        Box::new(Ci),
    ];

    if let Err(e) = utils::cli::run_app(&actions) {
        eprintln!("{:#}", e);
        std::process::exit(1);
    }
}
