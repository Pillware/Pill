// This file implements the "create" action: scaffolding a new game project.
//
// Responsibilities:
// - Copies the pill_default template into the target directory.
// - Rewrites config.ini (TITLE, WINDOW_TITLE) and Cargo.toml (pill_engine path,
//   workspace membership) to match the new project.
// - Depends on: utils::paths (get_path, Location), utils::files (modify_file).

use anyhow::*;
use clap::{App, Arg, ArgMatches};
use fs_extra::dir::CopyOptions;
use path_absolutize::Absolutize;
use std::path::{Path, PathBuf};

use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::path_flag;
use crate::utils::files::modify_file;
use crate::utils::paths::*;

pub(crate) struct Create;

impl Action for Create {
    fn name(&self) -> &'static str {
        "create"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app.arg(
            Arg::with_name("name")
                .short("n")
                .long("name")
                .takes_value(true)
                .help("Name of new game project"),
        )
        .arg(path_flag())
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let parent = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let name = matches
            .value_of("name")
            .ok_or_else(|| Error::msg("--name <name> is required for the 'create' action"))?
            .to_string();
        create_game_project(&parent, &name)
    }
}

/// Scaffold a new Pill game project from the pill_default template.
/// Copies template files, renames the directory, and rewrites config.ini
/// and Cargo.toml with the new project name and absolute engine paths.
pub(crate) fn create_game_project(
    game_project_parent_directory_path: &Path,
    game_name: &str,
) -> Result<()> {
    const TEMPLATE_NAME: &str = "pill_default";

    let game_project_directory_path = game_project_parent_directory_path.join(game_name);
    // Guard against overwriting an existing directory.
    if game_project_directory_path.exists() {
        return Err(Error::msg(format!(
            "Game project directory {} already exists",
            game_project_directory_path.display()
        )));
    }

    let game_resource_directory_path = game_project_directory_path.join("res");

    println!(
        "Creating new game project {} in directory {}",
        game_name,
        game_project_directory_path.display()
    );

    // Get templates (assuming that they are stored in res folder of pill_launcher crate)
    let template_game_project_directory_path = get_path(Location::PillLauncherCrate)
        .join("res")
        .join("templates");

    // Copy the pill_default template into the target parent directory.
    println!("Copying project template...");

    fs_extra::dir::copy(
        template_game_project_directory_path.join(TEMPLATE_NAME),
        game_project_parent_directory_path,
        &CopyOptions::new().overwrite(true),
    )
    .context("Cannot copy template directory")?;

    // Rename the copied template directory to the new project name.
    std::fs::rename(
        game_project_parent_directory_path.join(TEMPLATE_NAME),
        &game_project_directory_path,
    )
    .context("Failed to rename template directory to game project name")?;

    // Setup config file
    println!("Setting up config file...");
    modify_file(
        &game_resource_directory_path.join("config.ini"),
        &game_resource_directory_path.join("config.ini"),
        |line: String| -> String {
            if line.starts_with("TITLE") {
                return format!("TITLE={}", game_name);
            }
            if line.starts_with("WINDOW_TITLE") {
                return format!("WINDOW_TITLE={}", game_name);
            }
            line
        },
    )?;

    // Rewrite Cargo.toml in a single pass — point pill_engine at the absolute path
    // and set the workspace field to the engine workspace directory.
    println!("Setting up manifest file...");
    let cargo_toml_path = game_project_directory_path.join("Cargo.toml");
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
                return format!(
                    "pill_engine = {{ path = \"{pill_engine_path}\", features = [\"game\"] }}"
                );
            }
            if line.contains("workspace") {
                return format!("workspace = \"{engine_workspace_path}\"");
            }
            line
        },
    )?;

    // Success
    println!("Game project creation completed!");

    Ok(())
}
