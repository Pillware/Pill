// This file declares all sub-modules under the actions/ directory and defines
// the common Action trait that every CLI action must implement.
//
// Modules:
// - assets: run the asset pipeline (HLSL→WGSL, etc.).
// - build: compile native or WASM projects.
// - run: build and launch native projects (or serve WASM).
// - cargo_passthrough: forward arbitrary cargo commands to the workspace.
// - create: scaffold a new project from template.
// - docs: generate cargo doc for engine crates.

use anyhow::Result;
use clap::{App, ArgMatches};

pub mod assets;
pub mod build;
pub mod cargo_passthrough;
pub mod create;
pub mod docs;
pub mod link;
pub mod run;

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
