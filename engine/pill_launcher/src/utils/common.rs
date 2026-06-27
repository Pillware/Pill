// This file provides general-purpose utility functions used across the launcher.
//
// Responsibilities:
// - Runtime feature-flag detection (env vars).
// - ANSI terminal escape helpers (cached terminal detection).
// - Cargo stderr parsing for user-friendly error messages.
// - Duration formatting.
// - Filesystem helpers: pseudo-symlink resolution, directory copy, atomic writes.

use std::{
    fs,
    io::IsTerminal,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Error, Result};
use fs_extra::dir::CopyOptions;

use crate::types::Location;
use crate::utils::paths::get_path;

// ---------------------------------------------------------------------------
// OS-specific constants and the dynamic library naming helper.
// ---------------------------------------------------------------------------

/// Executable file extension (e.g. ".exe" on Windows, "" on Linux/macOS).
#[cfg(target_os = "windows")]
pub(crate) const EXECUTABLE_SUFFIX: &str = ".exe";
#[cfg(not(target_os = "windows"))]
pub(crate) const EXECUTABLE_SUFFIX: &str = ""; // Linux, macOS, etc. – no extension

#[cfg(target_os = "windows")]
pub(crate) const DYNAMIC_LIBRARY_PREFIX: &str = ""; //  pill_project.dll
#[cfg(not(target_os = "windows"))]
pub(crate) const DYNAMIC_LIBRARY_PREFIX: &str = "lib"; //  libpill_project.so / .dylib

#[cfg(target_os = "windows")]
pub(crate) const DYNAMIC_LIBRARY_SUFFIX: &str = ".dll";
#[cfg(target_os = "linux")]
pub(crate) const DYNAMIC_LIBRARY_SUFFIX: &str = ".so";
#[cfg(target_os = "macos")]
pub(crate) const DYNAMIC_LIBRARY_SUFFIX: &str = ".dylib";
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
pub(crate) const DYNAMIC_LIBRARY_SUFFIX: &str = ".so"; // reasonable default for Unix-like platforms

pub(crate) fn dynamic_library_name(name: &str) -> String {
    format!("{DYNAMIC_LIBRARY_PREFIX}{name}{DYNAMIC_LIBRARY_SUFFIX}")
}

// ---------------------------------------------------------------------------
// Feature flags (runtime)
// ---------------------------------------------------------------------------

/// When set, stderr from cargo is parsed and noisy lines are suppressed,
/// and only actionable error messages are extracted. This is experimental
/// and may drop useful diagnostics — keep disabled by default.
/// Set `PILL_LAUNCHER_EXPERIMENTAL_LOGS=1` to enable at runtime.
pub(crate) fn use_experimental_logs_parser() -> bool {
    std::env::var("PILL_LAUNCHER_EXPERIMENTAL_LOGS")
        .ok()
        .as_deref()
        == Some("1")
}

// ---------------------------------------------------------------------------
// ANSI helpers (cached terminal detection)
// ---------------------------------------------------------------------------

/// Lazily-initialized ANSI escape wrappers for success / failure messages.
/// Cached so `is_terminal()` is called only once per process.
pub(crate) fn ansi_green() -> (&'static str, &'static str) {
    static ANSI: LazyLock<(&str, &str)> = LazyLock::new(|| {
        if std::io::stdout().is_terminal() {
            ("\x1b[32m", "\x1b[0m")
        } else {
            ("", "")
        }
    });
    *ANSI
}

/// Lazily-initialized ANSI escape wrappers for error messages.
pub(crate) fn ansi_red() -> (&'static str, &'static str) {
    static ANSI: LazyLock<(&str, &str)> = LazyLock::new(|| {
        if std::io::stdout().is_terminal() {
            ("\x1b[31m", "\x1b[0m")
        } else {
            ("", "")
        }
    });
    *ANSI
}

// ---------------------------------------------------------------------------
// Cargo error parsing
// ---------------------------------------------------------------------------

/// Extract a concise, actionable error message from raw cargo stderr.
/// Handles panics with "Caused by:" chains and tool-not-found hints.
pub(crate) fn parse_cargo_stderr(stderr: &str) -> String {
    let mut detail = String::new();
    let lines: Vec<&str> = stderr.lines().collect();

    let mut seen_panic = false;
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();

        // The panic message body is on the line immediately after "panicked at".
        if trimmed.starts_with("thread") && trimmed.contains("panicked at") {
            seen_panic = true;
            if i + 1 < lines.len() {
                let next = lines[i + 1].trim();
                if !next.is_empty() {
                    detail.push_str(next);
                }
            }
            i += 2;
            continue;
        }

        // Collect "Caused by:" chain entries that follow a panic.
        if seen_panic && trimmed.starts_with("Caused by:") {
            if i + 1 < lines.len() {
                let next = lines[i + 1].trim();
                if !next.is_empty() && next != "Caused by:" {
                    if !detail.is_empty() {
                        detail.push('\n');
                    }
                    detail.push_str(next);
                }
            }
            i += 2;
            continue;
        }

        if seen_panic && (trimmed.contains("not found on PATH") || trimmed.contains("Install ")) {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(trimmed);
        }

        i += 1;
    }

    if detail.is_empty() {
        detail = stderr.trim().to_string();
    }

    detail
        .lines()
        .map(|l| format!("\t{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Time formatting
// ---------------------------------------------------------------------------

/// Format a duration as "after Xmin Ysec" or "after Ysec".
pub(crate) fn format_elapsed_time(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    let minutes = seconds / 60;
    let remainder = seconds % 60;
    if minutes > 0 {
        format!("after {}min {}sec", minutes, remainder)
    } else {
        format!("after {}sec", remainder)
    }
}

// ---------------------------------------------------------------------------
// WASM / filesystem helpers
// ---------------------------------------------------------------------------

/// Resolve the path to a launcher template directory.
pub(crate) fn get_template_directory(name: &str) -> PathBuf {
    get_path(Location::PillLauncherCrate)
        .join("res")
        .join("templates")
        .join(name)
}

/// Copy the WASM template into a scratch directory so the engine workspace stays pristine.
pub(crate) fn prepare_scratch_crate(
    wasm_template_directory: &Path,
    scratch_pill_web_app_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(scratch_pill_web_app_dir).with_context(|| {
        format!("Failed to create scratch directory {scratch_pill_web_app_dir:?}")
    })?;

    fs::copy(
        wasm_template_directory.join("Cargo.toml"),
        scratch_pill_web_app_dir.join("Cargo.toml"),
    )
    .context("Failed to copy pill_web_app Cargo.toml to scratch")?;

    let engine_lock = get_path(Location::EngineCrates).join("Cargo.lock");
    if engine_lock.exists() {
        fs::copy(&engine_lock, scratch_pill_web_app_dir.join("Cargo.lock"))
            .context("Failed to copy engine Cargo.lock into scratch")?;
    }

    let scratch_src_dir = scratch_pill_web_app_dir.join("src");
    if scratch_src_dir.exists() {
        fs::remove_dir_all(&scratch_src_dir).context("Failed to clean scratch src/")?;
    }
    fs_extra::dir::copy(
        wasm_template_directory.join("src"),
        scratch_pill_web_app_dir,
        &CopyOptions::new().overwrite(true),
    )
    .context("Failed to copy pill_web_app src/ to scratch")?;

    Ok(())
}

/// Copy the project's res/config.ini into the scratch crate so it can be include_str!-ed.
pub(crate) fn embed_project_config(
    project_directory: &Path,
    scratch_pill_web_app_dir: &Path,
) -> Result<()> {
    let source = project_directory.join("res").join("config.ini");
    let destination = scratch_pill_web_app_dir.join("config.ini");
    if source.is_file() {
        fs::copy(&source, &destination).with_context(|| {
            format!("Failed to embed project config {source:?} → {destination:?}")
        })?;
    } else {
        fs::write(&destination, "").with_context(|| {
            format!("Failed to write empty scratch config.ini at {destination:?}")
        })?;
    }
    Ok(())
}

fn get_cargo_path(path: &Path) -> String {
    path.to_string_lossy().replace("\\", "/")
}

/// Rewrite the scratch Cargo.toml with absolute path-deps to engine crates and the project.
/// Builds the entire manifest content in memory and writes it atomically.
pub(crate) fn rewrite_scratch_manifest(
    scratch_pill_web_app_dir: &Path,
    project_directory: &Path,
) -> Result<()> {
    let engine = get_path(Location::EngineCrates);
    let pill_engine = get_cargo_path(&engine.join("pill_engine"));
    let pill_renderer = get_cargo_path(&engine.join("pill_renderer"));
    let pill_core = get_cargo_path(&engine.join("pill_core"));
    let pill_web = get_cargo_path(&engine.join("pill_web"));
    let pill_project = get_cargo_path(project_directory);

    let manifest = scratch_pill_web_app_dir.join("Cargo.toml");

    let template = fs::read_to_string(&manifest)
        .with_context(|| format!("Failed to read scratch manifest {}", manifest.display()))?;

    let mut content = String::new();
    for line in template.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pill_engine ") || trimmed.starts_with("pill_engine=") {
            content.push_str(&format!(
                "pill_engine = {{ path = \"{pill_engine}\", features = [\"project\", \"internal\"] }}\n"
            ));
        } else if trimmed.starts_with("pill_renderer ") || trimmed.starts_with("pill_renderer=") {
            content.push_str(&format!(
                "pill_renderer = {{ path = \"{pill_renderer}\" }}\n"
            ));
        } else if trimmed.starts_with("pill_core ") || trimmed.starts_with("pill_core=") {
            content.push_str(&format!("pill_core = {{ path = \"{pill_core}\" }}\n"));
        } else if trimmed.starts_with("pill_web ") || trimmed.starts_with("pill_web=") {
            content.push_str(&format!("pill_web = {{ path = \"{pill_web}\" }}\n"));
        } else {
            content.push_str(line);
            content.push('\n');
        }
    }

    content.push_str(&format!(
        "\npill_project = {{ path = \"{pill_project}\" }}\n"
    ));
    content.push_str("\n[workspace]\nresolver = \"2\"\n");
    content.push_str("\n[profile.release]\n");
    content.push_str("opt-level = \"z\"\n");
    content.push_str("lto = \"fat\"\n");
    content.push_str("codegen-units = 1\n");
    content.push_str("panic = \"abort\"\n");
    content.push_str("strip = true\n");
    content.push_str("\n[package.metadata.wasm-pack.profile.release]\n");
    content.push_str("wasm-opt = [\"-Oz\", \"--strip-debug\", \"--strip-producers\", \"--enable-nontrapping-float-to-int\", \"--enable-bulk-memory\", \"--enable-sign-ext\", \"--enable-mutable-globals\", \"--enable-reference-types\"]\n");
    content.push_str("\n[target.'cfg(target_arch = \"wasm32\")'.dependencies]\n");
    content.push_str("lol_alloc = \"0.4\"\n");

    let tmp_path = manifest.with_extension(format!("toml-tmp-{}", std::process::id()));
    fs::write(&tmp_path, &content)
        .with_context(|| format!("Failed to write scratch manifest to {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &manifest).with_context(|| {
        format!(
            "Failed to rename {} to {}",
            tmp_path.display(),
            manifest.display()
        )
    })?;

    Ok(())
}

/// Copy project resource files into the WASM output directory.
pub(crate) fn copy_project_assets(
    source_resources: &Path,
    destination_resources: &Path,
) -> Result<()> {
    if !source_resources.is_dir() {
        return Ok(());
    }
    if destination_resources.exists() {
        fs::remove_dir_all(destination_resources).with_context(|| {
            format!("Failed to clean previous res/ at {destination_resources:?}")
        })?;
    }
    let destination_parent = destination_resources
        .parent()
        .ok_or_else(|| Error::msg("invalid res/ destination path"))?;
    fs::create_dir_all(destination_parent)?;
    fs_extra::dir::copy(
        source_resources,
        destination_parent,
        &CopyOptions::new().overwrite(true),
    )
    .with_context(|| {
        format!(
            "Failed to copy project res/ from {source_resources:?} to {destination_resources:?}"
        )
    })?;
    Ok(())
}

/// Copy all regular files from `source` into `destination`, resolving pseudo-symlinks.
pub(crate) fn copy_dir_files(source: &Path, destination: &Path, label: &str) -> Result<()> {
    for entry in fs::read_dir(source)
        .with_context(|| format!("Failed to read {label} directory {source:?}"))?
    {
        let entry = entry?;
        if entry.path().metadata()?.is_file() {
            let target = destination.join(entry.file_name());
            let entry_path = resolve_pseudo_symlink(&entry.path());
            fs::copy(&entry_path, &target)
                .with_context(|| format!("Failed to {label}-copy {entry_path:?} to {target:?}"))?;
        }
    }
    Ok(())
}

/// Resolve Git-on-Windows pseudo-symlinks (small text files containing a relative path).
/// Real symlinks and regular files pass through unchanged.
pub(crate) fn resolve_pseudo_symlink(path: &Path) -> PathBuf {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return path.to_path_buf();
    };
    if meta.file_type().is_symlink() || meta.len() > 1024 {
        return path.to_path_buf();
    }
    let Ok(content) = fs::read_to_string(path) else {
        return path.to_path_buf();
    };
    let trimmed = content.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let candidate = parent.join(trimmed);
    if candidate.is_file() {
        candidate
    } else {
        path.to_path_buf()
    }
}

/// Return the maximum mtime among regular files in `directory` (shallow scan).
/// Skips dotfiles and the `.build` scratch directory.
/// NOTE: This only watches top-level files. Changes in subdirectories
/// (e.g. build/wasm/res/textures/) do not trigger a reload.
pub(crate) fn get_latest_mtime_in_directory(directory: &Path) -> Option<SystemTime> {
    fs::read_dir(directory)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name();
            if name.to_string_lossy().starts_with('.') {
                return None;
            }
            let md = e.metadata().ok()?;
            if !md.is_file() {
                return None;
            }
            md.modified().ok()
        })
        .max()
}

// ---------------------------------------------------------------------------
// Build error formatting
// ---------------------------------------------------------------------------

/// Format a build-failure message with ANSI red coloring and elapsed time.
pub(crate) fn format_build_error(detail: &str, elapsed: Duration) -> String {
    let time_str = format_elapsed_time(elapsed);
    let (open, close) = ansi_red();
    format!(
        "{open}Pill Standalone \"run\" command failed {time_str}{close}\n\nCaused by:\n{detail}"
    )
}
