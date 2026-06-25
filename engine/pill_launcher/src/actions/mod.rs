// This file declares all sub-modules under the actions/ directory and defines
// the common Action trait that every CLI action must implement.
//
// Modules:
// - assets: run the asset pipeline (HLSL→WGSL, etc.).
// - benchmark / benchmarks: performance and artifact-size analysis.
// - build: compile and run native game projects (Build + Run structs).
// - cargo_passthrough: forward arbitrary cargo commands to the workspace.
// - check / checks: code validation (check_code) and WASM smoke-testing (check_wasm).
// - ci: meta-action running check → fmt → clippy → build sequentially.
// - create: scaffold a new game project from template.
// - docs: generate cargo doc for engine crates.

use anyhow::Result;
use clap::{App, ArgMatches};

pub mod assets;
pub mod benchmark;
pub mod benchmarks;
pub mod build;
pub mod cargo_passthrough;
pub mod check;
pub mod checks;
pub mod ci;
pub mod create;
pub mod docs;

/// Common interface for every CLI action.
///
/// Each action:
/// - Has a unique name (the value passed to `-a` / `--action`).
/// - Registers its own CLI flags via `register()`.
/// - Executes its logic via `run()`, extracting any needed values from the
///   parsed `ArgMatches`.
pub(crate) trait Action {
    /// The CLI action name, e.g. "build", "check", "benchmark".
    fn name(&self) -> &'static str;

    /// Register this action's CLI flags on the given `App` and return it.
    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static>;

    /// Execute the action using values extracted from the parsed CLI matches.
    fn run(&self, matches: &ArgMatches) -> Result<()>;
}
