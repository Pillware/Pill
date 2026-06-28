// This file implements the "cargo" passthrough action.
//
// Responsibilities:
// - Prepares the engine workspace for the given project.
// - Runs an arbitrary cargo command (fmt, clippy, check, etc.) in that workspace.
// - Depends on: workspace (prepare_workspace_for_project).

use anyhow::{bail, Context, Result};
use clap::{App, ArgMatches};
use path_absolutize::Absolutize;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::parse_compile_mode;
use crate::utils::workspace::prepare_workspace_for_project;

/// Registers `-p` / `--path`. Trailing args (after `--`) are collected
/// by the global passthrough mechanism in `utils::cli` and forwarded to cargo.
#[derive(Debug)]
pub(crate) struct Cargo;

impl Action for Cargo {
    fn name(&self) -> &'static str {
        "cargo"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app // shared flags (--path) registered in cli::run_app()
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let compile_mode = parse_compile_mode(matches);
        let passthrough: Vec<String> = matches
            .values_of("project-args")
            .map(|v| v.map(String::from).collect())
            .unwrap_or_default();
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        cargo_passthrough(&path, &compile_mode, &passthrough)
    }
}

/// Run an arbitrary cargo command in the engine workspace with the project linked.
/// Requires at least one cargo argument; fails if the cargo command exits non-zero.
pub(crate) fn cargo_passthrough(
    project_directory_path: &Path,
    compile_mode: &CompileMode,
    cargo_args: &[String],
) -> Result<()> {
    // Guard against accidental no-op invocations.
    if cargo_args.is_empty() {
        bail!("Must call cargo with at least one argument");
    }

    // Link the project into the workspace so cargo commands see the full context.
    let (engine_workspace_directory_path, _workspace_guard) =
        prepare_workspace_for_project(project_directory_path, compile_mode)?;
    // _workspace_guard restores engine/Cargo.toml on drop

    println!(
        "Running `cargo {}` in workspace {}...",
        cargo_args.join(" "),
        engine_workspace_directory_path.display()
    );

    let status = Command::new("cargo")
        .args(cargo_args)
        .current_dir(engine_workspace_directory_path)
        .status()
        .context("Failed to run cargo passthrough")?;

    if !status.success() {
        bail!(
            "Cargo command failed: cargo {:?} (exit {:?})",
            cargo_args,
            status.code()
        );
    }

    Ok(())
}
