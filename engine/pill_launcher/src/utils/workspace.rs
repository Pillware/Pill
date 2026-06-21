// This file manages Cargo workspace membership for game compilation.
//
// Responsibilities:
// - Temporarily injects the game project into engine/Cargo.toml's workspace members.
// - Cleans stale build artifacts when switching between different game projects.
// - Rewrites the game's own Cargo.toml workspace path to point at the engine workspace.
// - Ensures pill_native and pill_game share type IDs by compiling in the same workspace.
// - Depends on: utils::paths (location resolution), utils::files (file rewriting).

use anyhow::*;
use std::{fs, path::{Path, PathBuf}};

use crate::types::*;
use crate::utils::files::*;
use crate::utils::paths::*;

/// Prepare the engine workspace so the given game project can be built.
///
/// Side effects:
/// - May clean old game artifacts from engine/target/.
/// - Rewrites engine/Cargo.toml to include the game as a workspace member.
/// - Rewrites the game's Cargo.toml workspace field if needed.
///
/// Returns the engine workspace directory path.
pub(crate) fn prepare_workspace_for_game(
    game_project_directory_path: &Path,
    compile_mode: &CompileMode,
) -> Result<PathBuf> {
    // Validate the game project structure (Cargo.toml, res/, src/, config.ini).
    check_game_project_validity(game_project_directory_path).context("Game project is invalid")?;

    // Both pill_native and pill_game must compile in the same workspace so that
    // type IDs (used by generics/templates) are consistent between the host and game.
    let engine_workspace_directory_path = get_path(Location::EngineCrates);
    let workspace_manifest_path = engine_workspace_directory_path.join("Cargo.toml");
    if !workspace_manifest_path.exists() {
        return Err(Error::msg("Cannot find engine workspace manifest file"));
    }

    // Build the workspace member line that will be injected.
    let desired_game_path = normalize_path(game_project_directory_path)?;
    let desired_line = format!(
        "    \"{}\", {}",
        desired_game_path, GAME_PROJECT_CRATE_MARKER
    );

    // Determine which game project (if any) is currently linked in the workspace.
    let manifest_text = fs::read_to_string(&workspace_manifest_path)
        .with_context(|| format!("Failed to read {}", workspace_manifest_path.display()))?;

    let mut current_linked: Option<String> = None;
    for line in manifest_text.lines() {
        if let Some(p) = extract_member_path_from_line(line) {
            current_linked = Some(p);
            break;
        }
    }

    // Only perform cleanup/rewrite if the game project actually changed.
    let switching_game = match &current_linked {
        Some(cur) => cur != &desired_game_path,
        None => true,
    };

    // When switching projects, remove stale build artifacts from the previous game.
    if switching_game {
        let compilation_artifacts_folder_path = get_path(Location::EngineCrates)
            .join("target")
            .join(get_target_directory_for_compile_mode(compile_mode));

        let artifact_prefix = if cfg!(target_os = "windows") {
            "pill_game"
        } else {
            "libpill_game"
        };
        remove_files_starting_with(&compilation_artifacts_folder_path, artifact_prefix)?;
        remove_files_starting_with(
            &compilation_artifacts_folder_path.join("deps"),
            artifact_prefix,
        )?;
    }

    // Inject the game project path into engine/Cargo.toml's workspace members.
    if switching_game {
        modify_file(
            &workspace_manifest_path,
            &workspace_manifest_path,
            |line: String| -> String {
                if line.contains(GAME_PROJECT_CRATE_MARKER) {
                    return desired_line.clone();
                }
                line
            },
        )?;
    }

    // Ensure the game's own Cargo.toml workspace field points to the engine workspace.
    let game_manifest_path = game_project_directory_path.join("Cargo.toml");
    let engine_workspace_path = normalize_path(&get_path(Location::EngineCrates))?;
    let game_manifest_text = fs::read_to_string(&game_manifest_path)
        .with_context(|| format!("Failed to read {}", game_manifest_path.display()))?;

    let workspace_line_expected = format!("workspace = \"{}\"", engine_workspace_path);

    let already_has_workspace_line = game_manifest_text
        .lines()
        .any(|l| l.trim_start().starts_with("workspace") && l.contains(&engine_workspace_path));

    if !already_has_workspace_line {
        modify_file(
            &game_manifest_path,
            &game_manifest_path,
            |line: String| -> String {
                if line.trim_start().starts_with("workspace") {
                    return workspace_line_expected.clone();
                }
                line
            },
        )?;
    }

    Ok(engine_workspace_directory_path)
}
