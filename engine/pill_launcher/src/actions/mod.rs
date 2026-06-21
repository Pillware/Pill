// This file declares all sub-modules under the actions/ directory and defines
// the common Action trait that every CLI action must implement.
//
// Modules:
// - create: scaffold a new game project from template.
// - build: compile and run native game projects (Build + Run structs).
// - docs: generate cargo doc for engine crates.
// - cargo_passthrough: forward arbitrary cargo commands to the workspace.
// - check: fast compile-check of engine crates (no game code).
// - performance_benchmark: build+run iterations, collect frame-time JSON, print stats.
// - size_benchmark: build + analyze artifact sizes (native folder / WASM binary).
// - check_wasm: build WASM, smoke-test dev server, check size budget.
// - ci: meta-action running check → fmt → clippy → build sequentially.

use anyhow::Result;
use clap::{App, ArgMatches};

pub mod assets;
pub mod build;
pub mod cargo_passthrough;
pub mod check;
pub mod ci;
pub mod create;
pub mod docs;
pub mod performance_benchmark;
pub mod size_benchmark;
pub mod check_wasm;

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
