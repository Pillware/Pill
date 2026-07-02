//! This file declares all sub-modules under the actions/ directory and defines
//! the common Action trait that every CLI action must implement.
//!
//! Modules:
//! - assets: run the asset pipeline (HLSL→WGSL, etc.).
//! - build: compile native or WASM projects.
//! - run: build and launch native projects (or serve WASM).
//! - cargo_passthrough: forward arbitrary cargo commands to the workspace.
//! - create: scaffold a new project from template.
//! - docs: generate cargo doc for engine crates.
//! - link: link a project to the engine workspace (for dev builds).

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
/// - Has a unique name that becomes a subcommand (e.g., "build", "run", "create").
/// - Registers its own CLI flags via `register()`.
/// - Executes its logic via `run()`, extracting any needed values from the
///   parsed `ArgMatches`.
pub(crate) trait Action {
    /// The CLI action name, e.g. "build", "check", "benchmark".
    fn name(&self) -> &'static str;

    /// A short description shown in `PillLauncher --help` next to each subcommand.
    fn description(&self) -> &'static str {
        ""
    }

    /// Register this action's CLI flags on its subcommand `App` and return it.
    /// Shared flags (`-p`, `-c`, `-t`, etc.) must be added by the action itself.
    fn register(&self, application: App<'static, 'static>) -> App<'static, 'static>;

    /// Execute the action using values extracted from its subcommand's matches.
    fn run(&self, matches: &ArgMatches) -> Result<()>;
}
