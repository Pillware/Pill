// This file implements the "check-code" action: fast compile-check of engine crates.
//
// Responsibilities:
// - Temporarily removes the project from engine/Cargo.toml's workspace members
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
#[derive(Debug)]
pub(crate) struct CheckCode;

impl Action for CheckCode {
    fn name(&self) -> &'static str {
        "check-code"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app
    }

    fn run(&self, _matches: &ArgMatches) -> Result<()> {
        do_check_code()
    }
}

/// Run cargo check on all engine crates (no project code).
/// Temporarily strips the project from workspace members,
/// runs the check, and restores the manifest even on error or Ctrl+C.
pub(crate) fn do_check_code() -> Result<()> {
    println!("Running cargo check on engine crates...");

    // Locate the engine workspace manifest and temporarily strip the project.
    let engine_dir = get_path(Location::EngineCrates);
    let cargo_toml = engine_dir.join("Cargo.toml");

    if !cargo_toml.exists() {
        bail!("Engine Cargo.toml not found at {}", cargo_toml.display());
    }

    // Read the original manifest so we can restore it after the check.
    let original = fs::read_to_string(&cargo_toml)
        .with_context(|| format!("Failed to read {}", cargo_toml.display()))?;

    // Remove the project-project workspace member line so cargo check doesn't
    // try to resolve a project that may have system-specific paths or missing deps.
    let stripped: String = original
        .lines()
        .filter(|line| !line.contains(PROJECT_CRATE_MARKER))
        .collect::<Vec<_>>()
        .join("\n");

    // Write a backup before modifying the original, so recovery is possible
    // even if we crash or the restore write fails.
    let backup_path = cargo_toml.with_extension("toml.pill-backup");
    fs::write(&backup_path, &original).with_context(|| {
        format!(
            "Failed to write backup manifest to {}",
            backup_path.display()
        )
    })?;

    // Write the stripped version
    fs::write(&cargo_toml, &stripped)
        .with_context(|| format!("Failed to write {}", cargo_toml.display()))?;

    // Drop guard: restores the original Cargo.toml even if we panic or get SIGINT.
    // Falls back to the backup file if the primary restoration fails.
    struct RestoreGuard {
        path: std::path::PathBuf,
        content: String,
        backup_path: std::path::PathBuf,
    }
    impl RestoreGuard {
        fn restore(&self) -> std::io::Result<()> {
            fs::write(&self.path, &self.content)?;
            // Clean up the backup on successful restoration
            let _ = fs::remove_file(&self.backup_path);
            std::result::Result::Ok(())
        }
    }
    impl Drop for RestoreGuard {
        fn drop(&mut self) {
            if let Err(e) = self.restore() {
                // Primary restore failed — try to restore from backup
                eprintln!(
                    "WARNING: Failed to restore {} after check: {e}. \
                     A backup is available at {}. \
                     Restore it manually: copy {} {}",
                    self.path.display(),
                    self.backup_path.display(),
                    self.backup_path.display(),
                    self.path.display(),
                );
            }
        }
    }
    let guard = RestoreGuard {
        path: cargo_toml.clone(),
        content: original,
        backup_path,
    };

    // Run cargo check
    let status = Command::new("cargo")
        .args(&[
            "check",
            "-p",
            "pill_core",
            "-p",
            "pill_abi",
            "-p",
            "pill_assets",
            "-p",
            "pill_engine",
            "-p",
            "pill_native",
            "-p",
            "pill_runtime",
            "-p",
            "pill_web",
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

    // Explicitly restore and check for errors before returning success.
    guard.restore().with_context(|| {
        format!(
            "Failed to restore {} after check. \
             A backup is available at {}. \
             Restore it manually: copy {} {}",
            cargo_toml.display(),
            cargo_toml.with_extension("toml.pill-backup").display(),
            cargo_toml.with_extension("toml.pill-backup").display(),
            cargo_toml.display(),
        )
    })?;
    // Prevent the Drop impl from running a second time (the restore above already succeeded).
    std::mem::forget(guard);
    println!("cargo check passed.");
    Ok(())
}
