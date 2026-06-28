// This file implements the "link" / "unlink" actions for IDE support.
//
// Responsibilities:
// - link:   persist a project into engine/Cargo.toml's workspace members
//           so rust-analyzer can resolve types across engine + project.
// - unlink: remove the persisted project from workspace members.

use anyhow::{bail, Result};
use clap::{App, ArgMatches};
use std::fs;

use crate::actions::Action;
use crate::utils::cli::path_flag;
use crate::utils::paths::*;

// ---------------------------------------------------------------------------
// Link
// ---------------------------------------------------------------------------

pub(crate) struct Link;

impl Action for Link {
    fn name(&self) -> &'static str {
        "link"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app.arg(path_flag())
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let path = matches.value_of("path").unwrap_or(".");
        let engine_toml = get_path(crate::types::Location::EngineCrates).join("Cargo.toml");
        let normalized = normalize_path(&std::path::PathBuf::from(path))?;
        let marker_line = format!("    \"{}\", {}", normalized, PROJECT_CRATE_MARKER);

        let text = fs::read_to_string(&engine_toml)?;

        if text.contains(&marker_line) {
            println!("Project already linked: {normalized}");
            return Ok(());
        }

        // Remove any existing marker, then insert the new one before the closing ]
        let cleaned: String = text
            .lines()
            .filter(|l| !l.contains(PROJECT_CRATE_MARKER))
            .collect::<Vec<_>>()
            .join("\n");

        let mut in_members = false;
        let mut out = String::with_capacity(cleaned.len() + marker_line.len() + 2);
        let mut inserted = false;
        for line in cleaned.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("members") && trimmed.contains('[') {
                in_members = true;
            }
            if in_members && !inserted && trimmed == "]" {
                out.push_str(&marker_line);
                out.push('\n');
                inserted = true;
            }
            out.push_str(line);
            out.push('\n');
        }
        if !inserted {
            bail!("Could not find closing `]` of workspace members array");
        }
        let out = out.trim_end().to_string();
        fs::write(&engine_toml, format!("{out}\n"))?;

        println!("Linked {normalized} into engine workspace.");
        println!("rust-analyzer should pick it up automatically.");
        println!("Run `PillLauncher -a unlink` to remove it.");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Unlink
// ---------------------------------------------------------------------------

pub(crate) struct Unlink;

impl Action for Unlink {
    fn name(&self) -> &'static str {
        "unlink"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app
    }

    fn run(&self, _matches: &ArgMatches) -> Result<()> {
        let engine_toml = get_path(crate::types::Location::EngineCrates).join("Cargo.toml");
        let text = fs::read_to_string(&engine_toml)?;

        if !text.contains(PROJECT_CRATE_MARKER) {
            println!("No project currently linked.");
            return Ok(());
        }

        let cleaned: String = text
            .lines()
            .filter(|l| !l.contains(PROJECT_CRATE_MARKER))
            .collect::<Vec<_>>()
            .join("\n");

        fs::write(&engine_toml, cleaned)?;
        println!("Unlinked project from engine workspace.");
        Ok(())
    }
}
