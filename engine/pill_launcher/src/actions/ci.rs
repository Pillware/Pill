// This file implements the "ci" meta-action: run check → fmt → clippy → build.
//
// Responsibilities:
// - Executes four CI steps sequentially, stopping on the first failure.
// - Steps: cargo check (engine crates), rustfmt, clippy -D warnings, cargo build.
// - Uses actions::check, actions::cargo_passthrough (for fmt/clippy), and actions::build.
// - Prints a summary when all steps pass.

use anyhow::*;
use clap::{App, ArgMatches};
use path_absolutize::Absolutize;
use std::io::IsTerminal;
use std::path::PathBuf;

use crate::actions::build::build_game_project;
use crate::actions::cargo_passthrough::cargo_passthrough;
use crate::actions::check::do_check_code;
use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::{compile_mode_flag, parse_compile_mode, path_flag};
use crate::utils::paths::get_game_build_path;

#[derive(Debug)]
pub(crate) struct Ci;

impl Action for Ci {
    fn name(&self) -> &'static str {
        "ci"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app.arg(path_flag()).arg(compile_mode_flag())
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let compile_mode = parse_compile_mode(matches);
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        do_ci(&path, &compile_mode)
    }
}

/// Run the CI pipeline: cargo check → rustfmt → clippy → build.
/// Stops on the first failing step. Prints a summary on success.
pub(crate) fn do_ci(game_project_path: &PathBuf, compile_mode: &CompileMode) -> Result<()> {
    println!("=== CI Pipeline ===");
    println!();

    // Step 1: fast compile-check of all engine crates (no game code).
    println!("--- 1/4: cargo check ---");
    do_check_code().context("check step failed")?;

    // Step 2: code formatting via rustfmt.
    println!("--- 2/4: rustfmt ---");
    cargo_passthrough(game_project_path, compile_mode, &["fmt".into()])
        .context("fmt step failed")?;

    // Step 3: clippy linting with deny-by-default warnings.
    println!("--- 3/4: clippy (-D warnings) ---");
    cargo_passthrough(
        game_project_path,
        compile_mode,
        &["clippy".into(), "--".into(), "-D".into(), "warnings".into()],
    )
    .context("clippy step failed")?;

    // Step 4: full native build of the game project.
    println!("--- 4/4: build ---");
    let output_path = PathBuf::from(".");
    let output_directory_path = get_game_build_path(game_project_path, &output_path, compile_mode)?;
    build_game_project(
        game_project_path,
        &output_directory_path,
        compile_mode,
        None,
    )
    .context("build step failed")?;

    println!();
    if std::io::stdout().is_terminal() {
        println!("\x1b[32mAll CI checks passed.\x1b[0m");
    } else {
        println!("All CI checks passed.");
    }
    Ok(())
}
