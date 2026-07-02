//! This file handles all path resolution and workspace discovery for the launcher.

use anyhow::{bail, Context, Error, Result};
use config::Config;
use path_absolutize::Absolutize;
use std::path::{Path, PathBuf};

use crate::types::*;

/// Sentinel comment that marks a workspace-member line as the launcher-injected project.
/// Used by `prepare_workspace_for_project` to write the line, `extract_member_path_from_line`
/// to detect it, and the "check" action to strip it before running cargo check.
///
/// The `#` prefix ensures this is a valid TOML comment on all parsers.
/// The unique phrase minimizes false positives from coincidental text matches.
pub(crate) const PROJECT_CRATE_MARKER: &str = "# pill-launcher-managed-workspace-member";

/// Map CompileMode to the Cargo target directory name (debug/release/hot-reload).
pub(crate) fn get_target_directory_for_compile_mode(mode: &CompileMode) -> &'static str {
    match mode {
        CompileMode::Release => "release",
        CompileMode::Debug => "debug",
        CompileMode::HotReload => "hot-reload",
    }
}

pub(crate) fn get_standalone_layout_for_compile_mode(mode: &CompileMode) -> &'static str {
    match mode {
        CompileMode::Release => "packaged",
        CompileMode::Debug | CompileMode::HotReload => "development",
    }
}

/// Locate the engine workspace directory via env var, exe path, or cwd walk.
pub(crate) fn find_engine_workspace_directory() -> Result<PathBuf> {
    // Explicit override: the standalone host sets PILL_ENGINE_WORKSPACE_DIR.
    if let Ok(environment_value) = std::env::var("PILL_ENGINE_WORKSPACE_DIR") {
        let workspace_path = PathBuf::from(environment_value);
        let manifest_path = workspace_path.join("Cargo.toml");
        if manifest_path.exists() {
            return Ok(workspace_path);
        }
        bail!(
            "PILL_ENGINE_WORKSPACE_DIR was set but {} does not exist",
            manifest_path.display()
        );
    }

    // Walk up the directory tree from the executable location or cwd.
    fn search_up(start: PathBuf) -> Option<PathBuf> {
        for ancestor in start.ancestors() {
            // Look for an "engine" directory with a Cargo.toml inside.
            let candidate = ancestor.join("engine").join("Cargo.toml");
            if candidate.exists() {
                return Some(ancestor.join("engine"));
            }
            // Or: we might already be inside the engine/ directory itself.
            let candidate2 = ancestor.join("Cargo.toml");
            if candidate2.exists()
                && ancestor.file_name().and_then(|s| s.to_str()) == Some("engine")
            {
                return Some(ancestor.to_path_buf());
            }
        }
        None
    }

    // Try searching from the executable's location first, then from cwd.
    let executable_directory = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));
    if let Some(directory) = executable_directory.and_then(search_up) {
        return Ok(directory);
    }

    let current_directory = std::env::current_dir().context("current_dir failed")?;
    if let Some(directory) = search_up(current_directory) {
        return Ok(directory);
    }

    bail!("Cannot locate engine workspace directory (tried env + walking up from exe/cwd)");
}

/// Map a Location variant to its absolute filesystem path.
/// Panics if the engine workspace directory cannot be found.
pub(crate) fn get_path(location: Location) -> PathBuf {
    // engine workspace directory = .../Pill-Engine/engine
    let engine_workspace =
        find_engine_workspace_directory().expect("Failed to locate engine workspace directory");

    // repo root = parent of engine/
    let repo_root = engine_workspace.parent().unwrap().to_path_buf();

    match location {
        Location::EngineProjectRoot => repo_root,
        Location::EngineCrates => engine_workspace,
        Location::PillEngineCrate => engine_workspace.join("pill_engine"),
        Location::PillCoreCrate => engine_workspace.join("pill_core"),
        Location::PillNativeCrate => engine_workspace.join("pill_native"),
        Location::PillLauncherCrate => engine_workspace.join("pill_launcher"),
        Location::PillRendererCrate => engine_workspace.join("pill_renderer"),
        Location::PillAbiCrate => engine_workspace.join("pill_abi"),
        Location::PillAssetsCrate => engine_workspace.join("pill_assets"),
        Location::PillRuntimeCrate => engine_workspace.join("pill_runtime"),
        Location::PillWebCrate => engine_workspace.join("pill_web"),
    }
}

/// Convert a path to an absolute, forward-slash-normalized string for Cargo.toml.
pub(crate) fn normalize_path(p: &Path) -> Result<String> {
    Ok(p.absolutize()?
        .to_path_buf()
        .to_string_lossy()
        .replace('\\', "/"))
}

/// Extract the project path from a workspace member line containing the marker comment.
pub(crate) fn extract_member_path_from_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.contains(PROJECT_CRATE_MARKER) {
        return None;
    }
    let first_quote = trimmed.find('"')?;
    let rest = &trimmed[first_quote + 1..];
    let second_quote = rest.find('"')?;
    Some(rest[..second_quote].to_string())
}

pub(crate) fn get_output_directory_for_compile_mode(mode: &CompileMode) -> &'static str {
    match mode {
        CompileMode::Debug => "dev",
        CompileMode::Release => "release",
        CompileMode::HotReload => "hot-reload",
    }
}

/// Compute the build output directory for a project (defaults to <project>/build/<mode>/).
pub(crate) fn get_project_build_path(
    project_directory_path: &Path,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
) -> Result<PathBuf> {
    // Default output path: <project>/build/<dev|release|hot-reload>/.
    if output_directory_path.as_os_str() == "." {
        Ok(project_directory_path
            .join("build")
            .join(get_output_directory_for_compile_mode(compile_mode))
            .absolutize()?
            .to_path_buf())
    } else {
        Ok(output_directory_path.absolutize()?.to_path_buf())
    }
}

/// Read the TITLE field from a project's res/config.ini (spaces removed).
pub(crate) fn get_project_title(project_directory_path: &Path) -> Result<String> {
    // Get project title
    let config_path = project_directory_path.join("res").join("config.ini");
    let mut config = Config::default();
    config
        .merge(config::File::with_name(
            &config_path.to_string_lossy().into_owned(),
        ))
        .context("Failed to find config.ini file in project \"res\" folder")?;
    let project_title = config
        .get_str("TITLE")
        .context("Failed to get project config.ini")?
        .replace(' ', "");

    Ok(project_title)
}

/// Validate that a directory contains a Pill project (Cargo.toml, res/, src/, config.ini).
pub(crate) fn check_project_validity(project_directory_path: &Path) -> Result<()> {
    if !project_directory_path.join("Cargo.toml").exists() {
        return Err(Error::msg("Missing Cargo.toml file in project folder"));
    }
    if !project_directory_path.join("res").exists() {
        return Err(Error::msg("Missing \"res\" folder in project folder"));
    }
    if !project_directory_path.join("src").exists() {
        return Err(Error::msg("Missing \"src\" folder in project folder"));
    }
    if !project_directory_path
        .join("res")
        .join("config.ini")
        .exists()
    {
        return Err(Error::msg("Missing \"config.ini\" file in project folder"));
    }

    Ok(())
}
