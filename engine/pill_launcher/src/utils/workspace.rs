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
    time::Instant,
};

use crate::types::*;
use crate::utils::files::*;
use crate::utils::paths::*;

/// Restores engine/Cargo.toml (and optionally the project's Cargo.toml)
/// to its original content when dropped.
pub(crate) struct WorkspaceGuard {
    manifest_path: PathBuf,
    original: String,
    /// If the project manifest was patched (NO_PATH placeholders), restore it too.
    project_manifest: Option<(PathBuf, String)>,
}

impl Drop for WorkspaceGuard {
    fn drop(&mut self) {
        let _ = fs::write(&self.manifest_path, &self.original);
        if let Some((path, original)) = &self.project_manifest {
            let _ = fs::write(path, original);
        }
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
    let t_workspace_total = Instant::now();

    // Validate the project structure (Cargo.toml, res/, src/, config.ini).
    let t = Instant::now();
    check_project_validity(project_directory_path).context("Project is invalid")?;
    println!("[TIMING] check_project_validity: {:.3}s", t.elapsed().as_secs_f64());

    // Rewrite NO_PATH placeholders in the project's Cargo.toml (if present).
    // Projects created outside the launcher (or with stale templates) may
    // still have `path = "NO_PATH"` or `workspace = "NO_PATH"` that need to
    // be replaced with absolute engine paths before cargo can build.
    let t = Instant::now();
    let project_manifest_patch = patch_project_manifest(project_directory_path)?;
    println!("[TIMING] patch_project_manifest: {:.3}s", t.elapsed().as_secs_f64());

    // Both pill_native and project must compile in the same workspace so that
    // type IDs (used by generics/templates) are consistent between the host and project.
    let t = Instant::now();
    let engine_workspace_directory_path = get_path(Location::EngineCrates);
    println!("[TIMING] find_engine_workspace (get_path): {:.3}s", t.elapsed().as_secs_f64());

    let workspace_manifest_path = engine_workspace_directory_path.join("Cargo.toml");
    if !workspace_manifest_path.exists() {
        return Err(Error::msg("Cannot find engine workspace manifest file"));
    }

    // Build the workspace member line that will be injected.
    let t = Instant::now();
    let desired_project_path = normalize_path(project_directory_path)?;
    let desired_line = format!("    \"{}\", {}", desired_project_path, PROJECT_CRATE_MARKER);
    println!("[TIMING] normalize_path: {:.3}s", t.elapsed().as_secs_f64());

    // Snapshot the original manifest so we can restore it on exit.
    let t = Instant::now();
    let original_manifest = fs::read_to_string(&workspace_manifest_path)
        .with_context(|| format!("Failed to read {}", workspace_manifest_path.display()))?;
    println!("[TIMING] read_workspace_manifest: {:.3}s", t.elapsed().as_secs_f64());

    // Determine which project (if any) is currently linked in the workspace.
    // Primary source: the injected marker line in engine/Cargo.toml.
    // Fallback: the sentinel file written at the end of each successful workspace
    // preparation.  This is needed because WorkspaceGuard restores engine/Cargo.toml
    // on drop (removing the marker), so the *next* run would see `None` from the
    // manifest and incorrectly treat it as a project switch — triggering slow
    // artifact cleanup on every single run.
    let manifest_text = original_manifest.clone();

    let mut current_linked: Option<String> = None;
    for line in manifest_text.lines() {
        if let Some(p) = extract_member_path_from_line(line) {
            current_linked = Some(p);
            break;
        }
    }

    // If the manifest was restored (no marker), fall back to the sentinel file.
    let sentinel_path = engine_workspace_directory_path.join(".pill_last_project");
    if current_linked.is_none() {
        if let Ok(saved) = fs::read_to_string(&sentinel_path) {
            let saved = saved.trim().to_string();
            if !saved.is_empty() {
                current_linked = Some(saved);
            }
        }
    }

    // Only perform cleanup/rewrite if the project actually changed.
    // Compare canonicalized paths to handle case-insensitive filesystems (Windows/macOS).
    let t = Instant::now();
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
    println!("[TIMING] switching_project check (canonicalize): {:.3}s (switching={})", t.elapsed().as_secs_f64(), switching_project);

    // When switching projects, remove stale build artifacts from the previous project.
    if switching_project {
        let compilation_artifacts_folder_path = get_path(Location::EngineCrates)
            .join("target")
            .join(get_target_directory_for_compile_mode(compile_mode));

        // Clean artifacts for all three workspace crates that vary per-project
        let t = Instant::now();
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
        println!("[TIMING] remove_stale_artifacts: {:.3}s", t.elapsed().as_secs_f64());
    }

    // Inject the project path into engine/Cargo.toml's workspace members.
    // This must happen on EVERY run, not just when switching, because
    // WorkspaceGuard restores engine/Cargo.toml on drop (removing the member
    // line). The next run would otherwise fail with "package 'project' not found".
    {
        let t = Instant::now();
        // Re-read the manifest to get its current state (may have been restored).
        let current_manifest_text = fs::read_to_string(&workspace_manifest_path)
            .with_context(|| format!("Failed to read {}", workspace_manifest_path.display()))?;
        let already_linked = current_manifest_text
            .lines()
            .any(|l| l.contains(PROJECT_CRATE_MARKER) && l.contains(&desired_project_path));

        if !already_linked {
            // Check if there is any (possibly stale) marker line to replace.
            let has_marker = current_manifest_text.lines().any(|l| l.contains(PROJECT_CRATE_MARKER));
            if has_marker {
                // Replace the existing marker line with the correct project path.
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
                // No marker line at all — insert before the closing `]` of members.
                let mut out = String::with_capacity(current_manifest_text.len() + desired_line.len() + 2);
                let mut in_members = false;
                let mut inserted = false;
                for line in current_manifest_text.lines() {
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
                out.truncate(out.trim_end().len());
                out.push('\n');
                fs::write(&workspace_manifest_path, &out).with_context(|| {
                    format!("Failed to write {}", workspace_manifest_path.display())
                })?;
            }
        }
        println!("[TIMING] inject_workspace_member: {:.3}s", t.elapsed().as_secs_f64());
    }

    // Persist the current project path so the next run can detect a real project
    // switch even after WorkspaceGuard has restored engine/Cargo.toml.
    let _ = fs::write(&sentinel_path, &desired_project_path);

    // Ensure the project's own Cargo.toml workspace field points to the engine workspace.
    let t = Instant::now();
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
    println!("[TIMING] update_project_workspace_line: {:.3}s", t.elapsed().as_secs_f64());

    println!("[TIMING] prepare_workspace TOTAL: {:.3}s", t_workspace_total.elapsed().as_secs_f64());

    Ok((
        engine_workspace_directory_path,
        WorkspaceGuard {
            manifest_path: workspace_manifest_path,
            original: original_manifest,
            project_manifest: project_manifest_patch,
        },
    ))
}

/// Rewrite `NO_PATH` placeholders in the project's Cargo.toml to absolute
/// engine paths.  Returns `None` if no rewriting was needed, or
/// `Some((path, original_content))` so the caller can restore on drop.
fn patch_project_manifest(project_path: &Path) -> Result<Option<(PathBuf, String)>> {
    let pill_engine_path = get_path(Location::PillEngineCrate)
        .to_string_lossy()
        .replace('\\', "/");
    let engine_workspace = get_path(Location::EngineCrates)
        .to_string_lossy()
        .replace('\\', "/");

    let manifest = project_path.join("Cargo.toml");
    if !manifest.exists() {
        return Ok(None);
    }
    let original = fs::read_to_string(&manifest)
        .with_context(|| format!("Failed to read {}", manifest.display()))?;

    let mut patched = String::new();
    let mut changed = false;
    for line in original.lines() {
        if line.contains("NO_PATH") {
            changed = true;
            if line.contains("pill_engine") {
                // Preserve features, only replace path.
                let features = if let Some(start) = line.find("features") {
                    let remainder = &line[start..];
                    remainder
                        .trim_end_matches(|c: char| c == '}' || c.is_whitespace())
                        .to_string()
                } else {
                    "features = [\"project\"]".to_string()
                };
                patched.push_str(&format!(
                    "pill_engine = {{ path = \"{pill_engine_path}\", {features} }}\n"
                ));
            } else if line.contains("workspace") {
                patched.push_str(&format!("workspace = \"{engine_workspace}\"\n"));
            } else {
                patched.push_str(line);
                patched.push('\n');
            }
        } else {
            patched.push_str(line);
            patched.push('\n');
        }
    }

    if changed {
        fs::write(&manifest, &patched)
            .with_context(|| format!("Failed to patch {}", manifest.display()))?;
        Ok(Some((manifest, original)))
    } else {
        Ok(None)
    }
}
