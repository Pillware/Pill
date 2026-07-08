//! This file provides path resolution for the native standalone runner.
//!
//! Detects project and engine workspace directories at startup using a
//! priority chain: Cargo.toml manifest → environment variable → filesystem scan.
//! Also classifies the run layout (Development vs Packaged) and provides
//! platform-specific dynamic-library naming helpers.

use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Platform Helpers
// ---------------------------------------------------------------------------

/// Platform-specific dynamic-library naming conventions.
#[cfg(target_os = "windows")]
pub(crate) const DYLIB_PREFIX: &str = "";
#[cfg(not(target_os = "windows"))]
pub(crate) const DYLIB_PREFIX: &str = "lib";

#[cfg(target_os = "windows")]
pub(crate) const DYLIB_SUFFIX: &str = ".dll";
#[cfg(target_os = "linux")]
pub(crate) const DYLIB_SUFFIX: &str = ".so";
#[cfg(target_os = "macos")]
pub(crate) const DYLIB_SUFFIX: &str = ".dylib";
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub(crate) const DYLIB_SUFFIX: &str = ".so";

/// Builds a platform-appropriate dynamic-library file name from a base name.
pub(crate) fn dylib(name: &str) -> String {
    format!("{DYLIB_PREFIX}{name}{DYLIB_SUFFIX}")
}

// ---------------------------------------------------------------------------
// Data Structures
// ---------------------------------------------------------------------------

/// Distinguishes between a development workspace layout (project lives
/// alongside the engine) and a packaged release layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunLayout {
    Development,
    Packaged,
}

/// Describes whether the runtime (pill_runtime) is loaded as a separate
/// dynamic library or compiled directly into the executable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeLoadMode {
    Dylib,
    InProcess,
}

/// Parses the runtime load mode from an environment variable value.
pub(crate) fn parse_runtime_load_mode(value: Option<String>) -> Option<RuntimeLoadMode> {
    match value.as_deref() {
        Some("dylib") => Some(RuntimeLoadMode::Dylib),
        Some("in_process") => Some(RuntimeLoadMode::InProcess),
        _ => None,
    }
}

/// Central store for every path the standalone runner needs at runtime.
/// Avoids scattering path joins throughout the codebase.
pub(crate) struct ProjectPaths {
    pub(crate) build_data_directory_path: PathBuf,
    pub(crate) engine_source_directory_path: Option<PathBuf>,
    pub(crate) project_directory_path: PathBuf,
    pub(crate) project_resources_directory_path: PathBuf,
    pub(crate) project_source_directory_path: PathBuf,
    pub(crate) config_path: PathBuf,
    pub(crate) runtime_dynamic_library_path: PathBuf,
    pub(crate) runtime_dynamic_library_hot_reloaded_path: PathBuf,
    pub(crate) project_dynamic_library_path: PathBuf,
    pub(crate) project_dynamic_library_hot_reloaded_path: PathBuf,
}

// ---------------------------------------------------------------------------
// Project Detection
// ---------------------------------------------------------------------------

/// Checks whether a directory contains the minimum set of files
/// required to be treated as a valid Pill project.
pub(crate) fn project_exists(path: &Path) -> bool {
    path.join("Cargo.toml").exists()
        && path.join("res").join("config.ini").exists()
        && path.join("src").exists()
}

/// Determines the project directory from either the PROJECT_DIR
/// environment variable or by walking up from the executable path.
pub(crate) fn infer_project_directory(current_directory_path: &Path) -> Result<PathBuf> {
    // First check the explicit environment override.
    if let Ok(value) = std::env::var("PROJECT_DIR") {
        let path = PathBuf::from(value);
        if project_exists(&path) {
            return Ok(path);
        }
        bail!(
            "PROJECT_DIR was set but {} is not a valid project",
            path.display()
        );
    }

    // Fall back: the executable is at <project>/build/<dev|release>/pill_native.exe,
    // so go up two levels to reach the project root.
    current_directory_path
        .parent()
        .context("Build directory has no parent")?
        .parent()
        .context("Project directory resolution failed")
        .map(Path::to_path_buf)
}

/// Classifies the current run as Development or Packaged based on
/// environment variables and filesystem heuristics.
pub(crate) fn resolve_run_layout(project_directory_path: &Path) -> RunLayout {
    match std::env::var("PILL_STANDALONE_LAYOUT").ok().as_deref() {
        Some("development") => RunLayout::Development,
        Some("packaged") => RunLayout::Packaged,
        _ if project_exists(project_directory_path) => RunLayout::Development,
        _ => RunLayout::Packaged,
    }
}

// ---------------------------------------------------------------------------
// Engine Workspace Discovery
// ---------------------------------------------------------------------------

/// Returns true when the engine workspace's Cargo.toml lists the given
/// project directory name as a workspace member.
fn workspace_includes_project(
    engine_source_directory_path: &Path,
    project_directory_path: &Path,
) -> bool {
    let cargo_toml_path = engine_source_directory_path.join("Cargo.toml");
    let Ok(contents) = fs::read_to_string(cargo_toml_path) else {
        return false;
    };
    let Some(project_directory_name) = project_directory_path
        .file_name()
        .and_then(|name| name.to_str())
    else {
        return false;
    };
    contents.contains(project_directory_name)
}

/// Quick heuristic: a directory looks like the engine workspace if it
/// contains the three core engine crates.
pub(crate) fn looks_like_engine_workspace(path: &Path) -> bool {
    path.join("pill_core").exists()
        && path.join("pill_engine").exists()
        && path.join("pill_renderer").exists()
}

/// Attempts to read the `workspace` key from the project's Cargo.toml
/// and returns the resolved path if it points to an existing directory.
fn engine_workspace_from_project_manifest(project_directory_path: &Path) -> Option<PathBuf> {
    let manifest_path = project_directory_path.join("Cargo.toml");
    let contents = fs::read_to_string(manifest_path).ok()?;

    for line in contents.lines() {
        let line = line.trim();
        if !line.starts_with("workspace") {
            continue;
        }
        let (_, rhs) = line.split_once('=')?;
        let rhs = rhs.trim().strip_prefix('"')?.strip_suffix('"')?;
        let path = PathBuf::from(rhs);
        if path.exists() {
            return Some(path);
        }
    }

    None
}

/// Searches for the engine workspace directory by walking up from the
/// executable location and by scanning sibling directories of the project.
/// Returns the first path that looks like a valid engine workspace.
///
/// Two-phase search: (1) ancestor walk, (2) sibling scan as fallback.
fn find_engine_source_directory(
    current_directory_path: &Path,
    project_directory_path: &Path,
) -> Option<PathBuf> {
    // Walk up the directory tree from the current executable path.
    for ancestor in current_directory_path.ancestors() {
        let engine_candidate = ancestor.join("engine");
        if looks_like_engine_workspace(&engine_candidate)
            || engine_candidate
                .join("pill_engine")
                .join("Cargo.toml")
                .exists()
        {
            return Some(engine_candidate);
        }

        if looks_like_engine_workspace(ancestor)
            || ancestor.join("pill_engine").join("Cargo.toml").exists()
        {
            return Some(ancestor.to_path_buf());
        }
    }

    // Scan sibling directories of the project as a fallback.
    if let Some(parent) = project_directory_path.parent() {
        if let Ok(entries) = fs::read_dir(parent) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let engine_candidate = path.join("engine");
                if looks_like_engine_workspace(&engine_candidate)
                    || engine_candidate
                        .join("pill_engine")
                        .join("Cargo.toml")
                        .exists()
                {
                    return Some(engine_candidate);
                }

                if looks_like_engine_workspace(&path)
                    || path.join("pill_engine").join("Cargo.toml").exists()
                {
                    return Some(path);
                }
            }
        }
    }

    None
}

/// Resolves the engine workspace directory using a priority chain:
///   1. The `workspace` key in the project's Cargo.toml
///   2. The PILL_ENGINE_WORKSPACE_DIR environment variable
///   3. Filesystem scanning (ancestor and sibling search)
///
/// Optionally validates that the workspace actually includes the project.
pub(crate) fn resolve_engine_workspace_dir(
    current_directory_path: &Path,
    project_directory_path: &Path,
    require_workspace_membership: bool,
) -> Result<PathBuf> {
    let by_manifest = engine_workspace_from_project_manifest(project_directory_path);
    let by_environment = std::env::var("PILL_ENGINE_WORKSPACE_DIR")
        .ok()
        .map(PathBuf::from);
    let by_scan = find_engine_source_directory(current_directory_path, project_directory_path);

    // Try each candidate in priority order: manifest → env var → filesystem scan.
    // The first valid engine workspace that (optionally) contains the project wins.
    for candidate in [by_manifest, by_environment, by_scan].into_iter().flatten() {
        // Skip candidates that don't look like valid engine workspaces.
        if !looks_like_engine_workspace(&candidate) && !candidate.join("pill_engine").exists() {
            continue;
        }

        // Optionally check that the workspace manifest lists this project.
        if require_workspace_membership
            && !workspace_includes_project(&candidate, project_directory_path)
        {
            continue;
        }

        return Ok(candidate);
    }

    bail!(
        "Engine workspace not detected. Set PILL_ENGINE_WORKSPACE_DIR to the engine directory{}.",
        if require_workspace_membership {
            " that includes the pill project workspace member"
        } else {
            ""
        }
    )
}
