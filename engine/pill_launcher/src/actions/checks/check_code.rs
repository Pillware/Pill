// This file implements the "check-code" action: fast compile-check of engine crates.
//
// Responsibilities:
// - Temporarily removes the game project from engine/Cargo.toml's workspace members
//   so cargo check doesn't try to resolve a project with system-specific paths.
// - Runs `cargo check` on pill_core, pill_abi, pill_assets, pill_engine, pill_native,
//   pill_runtime, and pill_web.
// - Restores the original Cargo.toml even on error (guard pattern).
// - Depends on: utils::paths (get_path, Location).

use anyhow::*;
use clap::{App, ArgMatches};
use std::fs;
use std::process::Command;

use crate::actions::Action;
use crate::types::*;
use crate::utils::paths::*;

/// No flags needed — just registers the action name.
pub(crate) struct CheckCode;

impl Action for CheckCode {
    fn name(&self) -> &'static str { "check-code" }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app
    }

    fn run(&self, _matches: &ArgMatches) -> Result<()> {
        do_check_code()
    }
}

/// Run cargo check on all engine crates (no game code).
/// Temporarily strips the game project from workspace members,
/// runs the check, and restores the manifest even on error.
pub(crate) fn do_check_code() -> Result<()> {
    println!("Running cargo check on engine crates...");

    // Locate the engine workspace manifest and temporarily strip the game project.
    let engine_dir = get_path(Location::EngineCrates);
    let cargo_toml = engine_dir.join("Cargo.toml");

    if !cargo_toml.exists() {
        bail!("Engine Cargo.toml not found at {}", cargo_toml.display());
    }

    // Read the original manifest so we can restore it after the check.
    let original = fs::read_to_string(&cargo_toml)
        .with_context(|| format!("Failed to read {}", cargo_toml.display()))?;

    // Remove the game-project workspace member line so cargo check doesn't
    // try to resolve a project that may have system-specific paths or missing deps.
    let stripped: String = original
        .lines()
        .filter(|line| !line.contains(GAME_PROJECT_CRATE_MARKER))
        .collect::<Vec<_>>()
        .join("\n");

    // Write the stripped version
    fs::write(&cargo_toml, &stripped)
        .with_context(|| format!("Failed to write {}", cargo_toml.display()))?;

    // Run cargo check in a closure so the manifest is always restored.
    let result = (|| -> Result<()> {
        let status = Command::new("cargo")
            .args(&[
                "check",
                "-p", "pill_core",
                "-p", "pill_abi",
                "-p", "pill_assets",
                "-p", "pill_engine",
                "-p", "pill_native",
                "-p", "pill_runtime",
                "-p", "pill_web",
            ])
            .current_dir(&engine_dir)
            .status()
            .context("Failed to spawn cargo check")?;

        if !status.success() {
            bail!(
                "cargo check failed with exit code {}",
                status.code().map_or("unknown".into(), |c| c.to_string())
            );
        }
        Ok(())
    })();

    // Restore the original manifest regardless of success/failure
    fs::write(&cargo_toml, &original)
        .with_context(|| format!("Failed to restore {}", cargo_toml.display()))?;

    result?;

    println!("cargo check passed.");
    Ok(())
}
