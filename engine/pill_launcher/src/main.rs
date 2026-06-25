// This file is the crate root for PillLauncher.
//
// Responsibilities:
// - Declares all top-level modules.
// - Instantiates all CLI action objects.
// - Passes them to the generic dispatcher in utils::cli.

mod actions;
mod types;
mod utils;

use actions::assets::Assets;
use actions::benchmark::{Benchmark, SizeBenchmark};
use actions::build::{Build, Run};
use actions::cargo_passthrough::Cargo;
use actions::check::{CheckCode, CheckWasm};
use actions::ci::Ci;
use actions::create::Create;
use actions::docs::Docs;
use actions::Action;

fn main() {
    let actions: Vec<Box<dyn Action>> = vec![
        Box::new(Create),
        Box::new(Run),
        Box::new(Build),
        Box::new(Docs),
        Box::new(Cargo),
        Box::new(Assets),
        Box::new(CheckCode),
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
