//! This file implements the "link" and "unlink" actions for IDE support.
//!
//! Responsibilities:
//! - `link`:   persist a project into engine/Cargo.toml's workspace members
//!             so rust-analyzer can resolve types across engine + project.
//! - `unlink`: remove the persisted project from workspace members.

use anyhow::{bail, Result};
use clap::{App, ArgMatches};
use std::fs;

use crate::actions::Action;
use crate::utils::cli::path_flag;
use crate::utils::paths::*;

// ---------------------------------------------------------------------------
// Link action
// ---------------------------------------------------------------------------

/// The `link` subcommand: add a project to the engine workspace members
/// so IDE tooling (rust-analyzer) can resolve cross-crate references.
pub(crate) struct Link;

impl Action for Link {
    fn name(&self) -> &'static str {
        "link"
    }

    fn description(&self) -> &'static str {
        "Link a project into the engine workspace (for IDE support)"
    }

    fn register(&self, application: App<'static, 'static>) -> App<'static, 'static> {
        application.arg(path_flag())
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let path = matches.value_of("path").unwrap_or(".");
        let engine_toml = get_path(crate::types::Location::EngineCrates).join("Cargo.toml");
        let normalized = normalize_path(&std::path::PathBuf::from(path))?;
        let marker_line = format!("    \"{}\", {}", normalized, PROJECT_CRATE_MARKER);

        let text = fs::read_to_string(&engine_toml)?;

        // 1. If the project is already linked, do nothing.
        if text.contains(&marker_line) {
            println!("Project already linked: {normalized}");
            return Ok(());
        }

        // 2. Remove any existing marker line, then insert the new one before
        //    the closing `]` of the workspace members array.
        let cleaned: String = text
            .lines()
            .filter(|line| !line.contains(PROJECT_CRATE_MARKER))
            .collect::<Vec<_>>()
            .join("\n");

        let mut in_members = false;
        let mut output = String::with_capacity(cleaned.len() + marker_line.len() + 2);
        let mut inserted = false;
        for line in cleaned.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("members") && trimmed.contains('[') {
                in_members = true;
            }
            if in_members && !inserted && trimmed == "]" {
                output.push_str(&marker_line);
                output.push('\n');
                inserted = true;
            }
            output.push_str(line);
            output.push('\n');
        }
        if !inserted {
            bail!("Could not find closing `]` of workspace members array");
        }
        let output = output.trim_end().to_string();
        fs::write(&engine_toml, format!("{output}\n"))?;

        println!("Linked {normalized} into engine workspace.");
        println!("rust-analyzer should pick it up automatically.");
        println!("Run `PillLauncher unlink` to remove it.");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Unlink action
// ---------------------------------------------------------------------------

/// The `unlink` subcommand: remove the linked project from the engine
/// workspace members, restoring the Cargo.toml to its clean state.
pub(crate) struct Unlink;

impl Action for Unlink {
    fn name(&self) -> &'static str {
        "unlink"
    }

    fn description(&self) -> &'static str {
        "Remove a linked project from the engine workspace"
    }

    fn register(&self, application: App<'static, 'static>) -> App<'static, 'static> {
        application
    }

    fn run(&self, _matches: &ArgMatches) -> Result<()> {
        let engine_toml = get_path(crate::types::Location::EngineCrates).join("Cargo.toml");
        let text = fs::read_to_string(&engine_toml)?;

        // 1. If no project is linked, nothing to do.
        if !text.contains(PROJECT_CRATE_MARKER) {
            println!("No project currently linked.");
            return Ok(());
        }

        // 2. Remove all lines containing the marker comment.
        let cleaned: String = text
            .lines()
            .filter(|line| !line.contains(PROJECT_CRATE_MARKER))
            .collect::<Vec<_>>()
            .join("\n");

        fs::write(&engine_toml, cleaned)?;
        println!("Unlinked project from engine workspace.");
        Ok(())
    }
}
