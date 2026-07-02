//! This file implements the "create" action: scaffolding a new project.
//!
//! Responsibilities:
//! - Copies the pill_default template into the target directory.
//! - Rewrites config.ini (TITLE, WINDOW_TITLE) and Cargo.toml (pill_engine path,
//!   workspace membership) to match the new project.

use anyhow::{Context, Error, Result};
use clap::{App, Arg, ArgMatches};
use path_absolutize::Absolutize;
use std::path::{Path, PathBuf};

use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::path_flag;
use crate::utils::files::{copy_directory_recursive, modify_file};
use crate::utils::paths::*;

/// The `create` subcommand: scaffold a new Pill project from a template.
#[derive(Debug)]
pub(crate) struct Create;

impl Action for Create {
    fn name(&self) -> &'static str {
        "create"
    }

    fn description(&self) -> &'static str {
        "Scaffold a new project from template"
    }

    fn register(&self, application: App<'static, 'static>) -> App<'static, 'static> {
        application.arg(path_flag()).arg(
            Arg::with_name("name")
                .short("n")
                .long("name")
                .takes_value(true)
                .help("Name of new project"),
        )
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let parent = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let name = matches
            .value_of("name")
            .ok_or_else(|| Error::msg("--name <name> is required for the 'create' action"))?
            .to_string();
        create_project(&parent, &name)
    }
}

/// Scaffold a new Pill project from the new_project template.
///
/// 1. Guards against overwriting an existing directory.
/// 2. Copies the template directory to the target location.
/// 3. Rewrites config.ini with the project name.
/// 4. Rewrites Cargo.toml with absolute engine paths and workspace membership.
pub(crate) fn create_project(
    project_parent_directory_path: &Path,
    project_name: &str,
) -> Result<()> {
    const TEMPLATE_NAME: &str = "new_project";

    let project_directory_path = project_parent_directory_path.join(project_name);

    // 1. Guard against overwriting an existing directory.
    if project_directory_path.exists() {
        return Err(Error::msg(format!(
            "Project directory {} already exists",
            project_directory_path.display()
        )));
    }

    let project_resource_directory_path = project_directory_path.join("res");

    println!(
        "Creating new project {} in directory {}",
        project_name,
        project_directory_path.display()
    );

    // 2. Copy the template to the target directory.
    let template_project_directory_path = get_path(Location::PillLauncherCrate)
        .join("res")
        .join("templates");

    println!("Copying project template...");
    copy_directory_recursive(
        &template_project_directory_path.join(TEMPLATE_NAME),
        &project_directory_path,
    )
    .context("Cannot copy template directory")?;

    // 3. Rewrite config.ini: replace TITLE and WINDOW_TITLE with the project name.
    println!("Setting up config file...");
    modify_file(
        &project_resource_directory_path.join("config.ini"),
        &project_resource_directory_path.join("config.ini"),
        |line: String| -> String {
            if line.starts_with("TITLE") {
                return format!("TITLE={}", project_name);
            }
            if line.starts_with("WINDOW_TITLE") {
                return format!("WINDOW_TITLE={}", project_name);
            }
            line
        },
    )?;

    // 4. Rewrite Cargo.toml: set absolute engine paths and workspace.
    println!("Setting up manifest file...");
    let cargo_toml_path = project_directory_path.join("Cargo.toml");
    let pill_engine_path = get_path(Location::PillEngineCrate)
        .to_string_lossy()
        .replace('\\', "/");
    let engine_workspace_path = get_path(Location::EngineCrates)
        .to_string_lossy()
        .replace('\\', "/");
    modify_file(
        &cargo_toml_path,
        &cargo_toml_path,
        |line: String| -> String {
            if line.contains("pill_engine") {
                // Preserve the original features list; only rewrite the path.
                let features = if let Some(start) = line.find("features") {
                    let remainder = &line[start..];
                    remainder
                        .trim_end_matches(|character: char| {
                            character == '}' || character.is_whitespace()
                        })
                        .to_string()
                } else {
                    "features = [\"project\"]".to_string()
                };
                return format!("pill_engine = {{ path = \"{pill_engine_path}\", {features} }}");
            }
            if line.contains("workspace") {
                return format!("workspace = \"{engine_workspace_path}\"");
            }
            line
        },
    )?;

    println!("Project creation completed!");
    Ok(())
}
