//! This file provides reusable file-system operations for the launcher.

use anyhow::{bail, Context, Result};
use std::{fs, path::Path};
// Used by codesign_ad_hoc on macOS:
#[cfg(target_os = "macos")]
use std::process::Command;

/// Rewrite a file line-by-line, passing each line to the provided closure that can modify it.
/// Skips writing if the output would be identical to the input (no-op detection).
#[must_use]
pub(crate) fn modify_file<A: FnMut(String) -> String>(
    input_path: &Path,
    output_path: &Path,
    mut action: A,
) -> Result<()> {
    // Read the entire file into memory and apply the transformation to each line.
    let input = fs::read_to_string(input_path)
        .with_context(|| format!("Failed to read {}", input_path.display()))?;

    // Normalize CRLF → LF so line-ending style changes don't cause
    // spurious rewrites or corrupt Cargo.toml / config.ini files.
    let input_normalized = input.replace("\r\n", "\n");

    // Prevent overwriting the same files
    let mut changed = false;

    // Read lines from input file
    let lines = input_normalized
        .lines()
        .map(|line| {
            let new_line = action(line.to_string());
            if new_line != line {
                changed = true;
            }
            new_line
        })
        .collect::<Vec<String>>();

    let mut out = lines.join("\n");
    if input_normalized.ends_with("\n") {
        out.push('\n');
    }

    // No-op detection: skip writing if nothing changed (avoids invalidating caches).
    // Compare against the original input (before CRLF→LF normalization) so that
    // files with CRLF line endings on Windows aren't needlessly rewritten.
    if input_path == output_path && !changed && out == input {
        return Ok(());
    }

    // Similarly we are writing to a different file and their outputs are identical - ignore
    if input_path != output_path {
        if let Ok(existing) = fs::read_to_string(output_path) {
            if existing == out {
                return Ok(());
            }
        }
    }

    // Write atomically when rewriting a file in-place: write to a temp file beside
    // the target and rename into place. A crash mid-write leaves the original intact.
    // Include the process ID in the temp filename to avoid collisions between
    // concurrent launcher processes.
    if input_path == output_path {
        let tmp_ext = format!("tmp-{}", std::process::id());
        let tmp_path = output_path.with_extension(tmp_ext);
        fs::write(&tmp_path, &out)
            .with_context(|| format!("Failed to write temp file {}", tmp_path.display()))?;
        fs::rename(&tmp_path, output_path).with_context(|| {
            format!(
                "Failed to rename {} to {}",
                tmp_path.display(),
                output_path.display()
            )
        })?;
    } else {
        fs::write(output_path, out)
            .with_context(|| format!("Failed to write {}", output_path.display()))?;
    }

    Ok(())
}

/// Ad-hoc code-sign a binary on macOS so it can be loaded as a dynamic library.
/// Required because macOS Gatekeeper refuses to load unsigned dylibs at runtime.
#[cfg(target_os = "macos")]
pub(crate) fn codesign_ad_hoc(path: &Path) -> Result<()> {
    let status = Command::new("codesign")
        .args(["--force", "--sign", "-", path.to_str().unwrap_or("")])
        .status()
        .with_context(|| format!("codesign failed for {}", path.display()))?;
    if !status.success() {
        bail!("codesign returned non-zero for {}", path.display());
    }
    Ok(())
}

/// Copy a file only if the source is newer than the destination (by mtime and size).
/// Returns true if a copy was performed, false if skipped.
#[must_use]
pub(crate) fn copy_file_if_newer(source: &Path, destination: &Path) -> Result<bool> {
    // Returns true if a copy was performed, false if the destination is already up-to-date.
    if !source.exists() {
        bail!("Source does not exist: {}", source.display());
    }

    let source_meta = fs::metadata(source)?;
    let source_mtime = source_meta.modified().ok();
    let source_len = source_meta.len();

    if let Ok(destination_meta) = fs::metadata(destination) {
        let destination_mtime = destination_meta.modified().ok();
        let destination_len = destination_meta.len();

        // If same size and destination is at least as new as source, skip copy.
        if destination_len == source_len {
            if let (Some(s), Some(d)) = (source_mtime, destination_mtime) {
                if d >= s {
                    return Ok(false);
                }
            }
        }
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)
        .with_context(|| format!("copy {} -> {}", source.display(), destination.display()))?;
    Ok(true)
}

/// Recursively copy all files and subdirectories from `source` to `destination`.
/// Creates the destination directory if it does not exist.  Preserves the full
/// directory tree structure.  Fails if `source` is missing or is not a directory.
pub(crate) fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<()> {
    if !source.exists() {
        bail!("Source directory does not exist: {}", source.display());
    }
    if !source.is_dir() {
        bail!("Source path is not a directory: {}", source.display());
    }

    fs::create_dir_all(destination)
        .with_context(|| format!("Failed to create directory {}", destination.display()))?;

    for entry in fs::read_dir(source)
        .with_context(|| format!("Failed to read directory {}", source.display()))?
    {
        let entry = entry?;
        let entry_path = entry.path();
        let destination_path = destination.join(entry.file_name());

        if entry_path.is_dir() {
            copy_directory_recursive(&entry_path, &destination_path)?;
        } else if entry_path.is_file() {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&entry_path, &destination_path).with_context(|| {
                format!(
                    "copy {} -> {}",
                    entry_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }

    Ok(())
}

/// Mirror a project's res/ directory into the build output data directory.
/// Used for packaged release builds where resources must be bundled alongside the executable.
/// Stages to a temporary directory first and validates before swapping, so a failed copy
/// never leaves a partial or corrupted resource directory.
pub(crate) fn stage_packaged_resource_files(
    project_directory_path: &Path,
    data_directory: &Path,
) -> Result<()> {
    let source_resources_dir = project_directory_path.join("res");
    let destination_resources_dir = data_directory.join("res");

    if !source_resources_dir.exists() {
        bail!(
            "Project resources directory does not exist: {}",
            source_resources_dir.display()
        );
    }

    fs::create_dir_all(data_directory).with_context(|| {
        format!(
            "Failed to create data directory {}",
            data_directory.display()
        )
    })?;

    // Stage into a temporary directory so we never leave a partial copy.
    let staging_dir = data_directory.join(".res_staging_tmp");
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir).with_context(|| {
            format!(
                "Failed to clear staging directory {}",
                staging_dir.display()
            )
        })?;
    }

    copy_directory_recursive(&source_resources_dir, &staging_dir)?;

    // Validate the staging before swapping it in.
    let staged_config_path = staging_dir.join("config.ini");
    if !staged_config_path.exists() {
        // Clean up the failed staging attempt.
        let _ = fs::remove_dir_all(&staging_dir);
        bail!(
            "Failed to stage resources (missing {})",
            staged_config_path.display()
        );
    }

    // Swap: remove the old directory and rename the staging into place.
    if destination_resources_dir.exists() {
        fs::remove_dir_all(&destination_resources_dir).with_context(|| {
            format!(
                "Failed to clear destination resources directory {}",
                destination_resources_dir.display()
            )
        })?;
    }

    fs::rename(&staging_dir, &destination_resources_dir).with_context(|| {
        format!(
            "Failed to swap staged resources from {} to {}",
            staging_dir.display(),
            destination_resources_dir.display()
        )
    })?;

    println!(
        "Staged resources from {} to {}",
        source_resources_dir.display(),
        destination_resources_dir.display()
    );

    Ok(())
}

/// Delete all regular files in a directory whose name starts with the given prefix.
/// Non-existent or non-directory paths are silently skipped.
pub(crate) fn remove_files_starting_with(
    directory_path: &Path,
    file_name_prefix: &str,
) -> Result<()> {
    if !directory_path.exists() || !directory_path.is_dir() {
        return Ok(()); // Skip non-existent or non-directory
    }

    for entry in fs::read_dir(directory_path).context("Failed to read directory")? {
        let entry = entry.context("Failed to read directory entry")?;
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                if name.starts_with(file_name_prefix) {
                    fs::remove_file(&path)
                        .with_context(|| format!("Failed to remove file: {}", path.display()))?;
                }
            }
        }
    }

    Ok(())
}

/// Recursively delete all cooked asset files (`.cooked_mesh`, `.cooked_tex`) under a directory.
/// Used by the asset pipeline's force-rebuild mode to clear previously built outputs.
pub(crate) fn delete_cooked_resource_files_recursive(directory: &Path) -> Result<()> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(directory)
        .with_context(|| format!("Failed to read {}", directory.display()))?
    {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            delete_cooked_resource_files_recursive(&path)?;
        } else if file_type.is_file() {
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if extension == "cooked_mesh" || extension == "cooked_tex" {
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to delete {}", path.display()))?;
                println!("Deleted {}", path.display());
            }
        }
    }
    Ok(())
}
