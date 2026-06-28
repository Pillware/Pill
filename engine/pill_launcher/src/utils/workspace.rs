// This file manages Cargo workspace membership for project compilation.
//
// Responsibilities:
// - Temporarily injects the project into engine/Cargo.toml's workspace members.
// - Automatically restores engine/Cargo.toml on drop (WorkspaceGuard).
// - Cleans stale build artifacts when switching between different projects.
// - Rewrites the project's own Cargo.toml workspace path to point at the engine workspace.
// - Ensures pill_native and project share type IDs by compiling in the same workspace.

use anyhow::{bail, Context, Error, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::types::*;
use crate::utils::files::*;
use crate::utils::paths::*;

/// Restores engine/Cargo.toml to its original content when dropped.
/// Keeps the workspace clean between builds - no user-specific paths
/// left behind that could accidentally be committed.
pub(crate) struct WorkspaceGuard {
    manifest_path: PathBuf,
    original: String,
}

impl Drop for WorkspaceGuard {
    fn drop(&mut self) {
        let _ = fs::write(&self.manifest_path, &self.original);
    }
}

impl WorkspaceGuard {
    /// Commit the current state - don't restore on drop.  Use this when
    /// you want the project to stay linked (e.g. for IDE/rust-analyzer).
    #[allow(dead_code)]
    pub(crate) fn commit(self) {
        // Leak self so Drop doesn't run.  Intentionally forget the guard
        // to persist the workspace modification.
        let _ = std::mem::ManuallyDrop::new(self);
    }
}

/// Prepare the engine workspace so the given project can be built.
///
/// Returns the engine workspace directory path and a guard that restores
/// engine/Cargo.toml on drop.  Callers should hold the guard until cargo
/// finishes, then let it drop (or call `.commit()` to persist).
pub(crate) fn prepare_workspace_for_project(
    project_directory_path: &Path,
    compile_mode: &CompileMode,
) -> Result<(PathBuf, WorkspaceGuard)> {
    // Validate the project structure (Cargo.toml, res/, src/, config.ini).
    check_project_validity(project_directory_path).context("Project is invalid")?;

    // Both pill_native and project must compile in the same workspace so that
    // type IDs (used by generics/templates) are consistent between the host and project.
    let engine_workspace_directory_path = get_path(Location::EngineCrates);
    let workspace_manifest_path = engine_workspace_directory_path.join("Cargo.toml");
    if !workspace_manifest_path.exists() {
        return Err(Error::msg("Cannot find engine workspace manifest file"));
    }

    // Build the workspace member line that will be injected.
    let desired_project_path = normalize_path(project_directory_path)?;
    let desired_line = format!("    \"{}\", {}", desired_project_path, PROJECT_CRATE_MARKER);

    // Snapshot the original manifest so we can restore it on exit.
    let original_manifest = fs::read_to_string(&workspace_manifest_path)
        .with_context(|| format!("Failed to read {}", workspace_manifest_path.display()))?;

    // Determine which project (if any) is currently linked in the workspace.
    let manifest_text = original_manifest.clone();

    let mut current_linked: Option<String> = None;
    for line in manifest_text.lines() {
        if let Some(p) = extract_member_path_from_line(line) {
            current_linked = Some(p);
            break;
        }
    }

    // Only perform cleanup/rewrite if the project actually changed.
    // Compare canonicalized paths to handle case-insensitive filesystems (Windows/macOS).
    let switching_project = match &current_linked {
        Some(cur) => {
            let cur_canon = std::path::PathBuf::from(cur).canonicalize().ok();
            let des_canon = std::path::PathBuf::from(&desired_project_path)
                .canonicalize()
                .ok();
            cur_canon != des_canon
        }
        None => true,
    };

    // When switching projects, remove stale build artifacts from the previous project.
    if switching_project {
        let compilation_artifacts_folder_path = get_path(Location::EngineCrates)
            .join("target")
            .join(get_target_directory_for_compile_mode(compile_mode));

        // Clean artifacts for all three workspace crates that vary per-project
        for prefix in &["project", "pill_runtime", "pill_native"] {
            let artifact_prefix = if cfg!(target_os = "windows") {
                prefix.to_string()
            } else {
                format!("lib{prefix}")
            };
            remove_files_starting_with(&compilation_artifacts_folder_path, &artifact_prefix)?;
            remove_files_starting_with(
                &compilation_artifacts_folder_path.join("deps"),
                &artifact_prefix,
            )?;
        }
    }

    // Inject the project path into engine/Cargo.toml's workspace members.
    if switching_project {
        if current_linked.is_some() {
            // Replace the existing marker line with the new project path.
            modify_file(
                &workspace_manifest_path,
                &workspace_manifest_path,
                |line: String| -> String {
                    if line.contains(PROJECT_CRATE_MARKER) {
                        return desired_line.clone();
                    }
                    line
                },
            )?;
        } else {
            // No project currently linked - insert a new member line before
            // the closing `]` of the members array.
            let manifest_text = fs::read_to_string(&workspace_manifest_path)
                .with_context(|| format!("Failed to read {}", workspace_manifest_path.display()))?;

            // Find the closing bracket of the members list and insert before it.
            let mut out = String::with_capacity(manifest_text.len() + desired_line.len() + 2);
            let mut in_members = false;
            let mut inserted = false;
            for line in manifest_text.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("members") && trimmed.contains('[') {
                    in_members = true;
                }
                if in_members && !inserted && trimmed == "]" {
                    out.push_str(&desired_line);
                    out.push('\n');
                    inserted = true;
                }
                out.push_str(line);
                out.push('\n');
            }
            if !inserted {
                bail!("Could not find closing `]` in workspace members section");
            }
            // Remove trailing newline added by the loop
            out.truncate(out.trim_end().len());
            out.push('\n');
            fs::write(&workspace_manifest_path, &out).with_context(|| {
                format!("Failed to write {}", workspace_manifest_path.display())
            })?;
        }
    }

    // Ensure the project's own Cargo.toml workspace field points to the engine workspace.
    let project_manifest_path = project_directory_path.join("Cargo.toml");
    let engine_workspace_path = normalize_path(&get_path(Location::EngineCrates))?;
    let project_manifest_text = fs::read_to_string(&project_manifest_path)
        .with_context(|| format!("Failed to read {}", project_manifest_path.display()))?;

    let workspace_line_expected = format!("workspace = \"{}\"", engine_workspace_path);

    let already_has_workspace_line = project_manifest_text
        .lines()
        .any(|l| l.trim_start().starts_with("workspace") && l.contains(&engine_workspace_path));

    if !already_has_workspace_line {
        modify_file(
            &project_manifest_path,
            &project_manifest_path,
            |line: String| -> String {
                if line.trim_start().starts_with("workspace") {
                    return workspace_line_expected.clone();
                }
                line
            },
        )?;
    }

    Ok((
        engine_workspace_directory_path,
        WorkspaceGuard {
            manifest_path: workspace_manifest_path,
            original: original_manifest,
        },
    ))
}
