// This file orchestrates WASM/WebGPU builds via wasm-pack.
//
// Responsibilities:
// - build(): entry point — copies a WASM template into a scratch directory,
//   rewrites Cargo.toml path-deps to absolute paths, runs wasm-pack, flattens
//   outputs into build/wasm/, and prints a size report on release builds.
// - Uses a scratch-copy strategy: nothing is written under engine/ during a
//   WASM build, keeping the workspace pristine across multi-game use.
// - Handles pseudo-symlinks (Git on Windows without core.symlinks).

use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Error, Result};
use fs_extra::dir::CopyOptions;

use crate::types::{CompileMode, Location};
use crate::utils::files::modify_file;
use crate::utils::paths::get_path;

/// Build a WASM/WebGPU bundle via wasm-pack using a scratch directory.
/// On release builds, prints a size report and enforces an optional size budget.
pub fn build(
    game_project_directory_path: &Path,
    compile_mode: &CompileMode,
    max_size_kb: Option<u64>,
) -> Result<()> {
    println!("Building WASM/WebGPU target for game project at {game_project_directory_path:?}...");
    if *compile_mode == CompileMode::HotReload {
        println!("Note: hot-reload is not meaningful for WASM; using --dev mode.");
    }

    // Paths: templates are in pill_launcher/res/templates/, output goes to <game>/build/wasm/.
    let wasm_template_dir = template_dir("wasm");
    let web_template_dir = template_dir("web");
    let build_wasm_dir = game_project_directory_path.join("build").join("wasm");
    let scratch_pill_web_app_dir = build_wasm_dir.join(".build").join("pill_web_app");
    let scratch_package_directory = build_wasm_dir.join(".build").join("pkg");

    // Pipeline: prepare scratch crate → embed config → rewrite manifest → wasm-pack → copy outputs.
    prepare_scratch_crate(&wasm_template_dir, &scratch_pill_web_app_dir)?;
    embed_game_config(game_project_directory_path, &scratch_pill_web_app_dir)?;
    rewrite_scratch_manifest(&scratch_pill_web_app_dir, game_project_directory_path)?;
    run_wasm_pack(compile_mode, &scratch_pill_web_app_dir, &scratch_package_directory)?;
    copy_build_outputs(
        &scratch_package_directory,
        &web_template_dir,
        &game_project_directory_path.join("web"),
        &build_wasm_dir,
    )?;
    copy_game_assets(
        &game_project_directory_path.join("res"),
        &build_wasm_dir.join("res"),
    )?;

    // Size budget check on release — fail if the binary exceeds the limit.
    if *compile_mode == CompileMode::Release {
        // Enforce optional size budget (analysis moved to size-benchmark action).
        if let Some(limit) = max_size_kb {
            let final_wasm = build_wasm_dir.join("pill_web_app_bg.wasm");
            let actual = fs::metadata(&final_wasm)
                .context("Cannot stat final WASM")?
                .len();
            if actual > limit * 1024 {
                bail!(
                    "WASM binary {:.1} KB exceeds budget {} KB",
                    actual as f64 / 1024.0,
                    limit
                );
            }
            println!(
                "Size guard OK ({:.1} KB ≤ {} KB)",
                actual as f64 / 1024.0,
                limit
            );
        }
    }

    println!();
    println!("Done! Serve with:");
    println!("  PillLauncher -a run -t wasm -p {game_project_directory_path:?}");
    println!("  (or any static server pointed at {build_wasm_dir:?})");
    Ok(())
}

fn template_dir(name: &str) -> PathBuf {
    get_path(Location::PillLauncherCrate)
        .join("res")
        .join("templates")
        .join(name)
}

/// Copy the WASM template into a scratch directory so the engine workspace stays pristine.
fn prepare_scratch_crate(wasm_template_dir: &Path, scratch_pill_web_app_dir: &Path) -> Result<()> {
    fs::create_dir_all(scratch_pill_web_app_dir)
        .with_context(|| format!("Failed to create scratch dir {scratch_pill_web_app_dir:?}"))?;

    fs::copy(
        wasm_template_dir.join("Cargo.toml"),
        scratch_pill_web_app_dir.join("Cargo.toml"),
    )
    .context("Failed to copy pill_web_app Cargo.toml to scratch")?;

    // Share the engine workspace's Cargo.lock so the scratch build resolves
    // identical crate versions to an in-place engine build. Without this,
    // cargo picks newer wasm-bindgen/etc. which can break WebGPU rendering.
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
        wasm_template_dir.join("src"),
        scratch_pill_web_app_dir,
        &CopyOptions::new().overwrite(true),
    )
    .context("Failed to copy pill_web_app src/ to scratch")?;

    Ok(())
}

// Copy the game's res/config.ini into the scratch crate at a known location
// so the template's lib.rs can include_str! it. Needed because wasm has no
// filesystem — the engine can't read config.ini at runtime.
/// Copy the game's res/config.ini into the scratch crate so it can be include_str!-ed.
fn embed_game_config(game_directory: &Path, scratch_pill_web_app_dir: &Path) -> Result<()> {
    let source = game_directory.join("res").join("config.ini");
    let destination = scratch_pill_web_app_dir.join("config.ini");
    if source.is_file() {
        fs::copy(&source, &destination)
            .with_context(|| format!("Failed to embed game config {source:?} → {destination:?}"))?;
    } else {
        // Write an empty file so the template's include_str! compiles.
        fs::write(&destination, "")
            .with_context(|| format!("Failed to write empty scratch config.ini at {destination:?}"))?;
    }
    Ok(())
}

/// Rewrite the scratch Cargo.toml with absolute path-deps to engine crates and the game.
fn rewrite_scratch_manifest(scratch_pill_web_app_dir: &Path, game_directory: &Path) -> Result<()> {
    let engine = get_path(Location::EngineCrates);
    let pill_engine = cargo_path(&engine.join("pill_engine"));
    let pill_renderer = cargo_path(&engine.join("pill_renderer"));
    let pill_core = cargo_path(&engine.join("pill_core"));
    let pill_web = cargo_path(&engine.join("pill_web"));
    let pill_game = cargo_path(game_directory);

    let manifest = scratch_pill_web_app_dir.join("Cargo.toml");
    modify_file(&manifest, &manifest, |line: String| -> String {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pill_engine ") || trimmed.starts_with("pill_engine=") {
            format!(
                "pill_engine = {{ path = \"{pill_engine}\", features = [\"game\", \"internal\"] }}"
            )
        } else if trimmed.starts_with("pill_renderer ") || trimmed.starts_with("pill_renderer=") {
            format!("pill_renderer = {{ path = \"{pill_renderer}\" }}")
        } else if trimmed.starts_with("pill_core ") || trimmed.starts_with("pill_core=") {
            format!("pill_core = {{ path = \"{pill_core}\" }}")
        } else if trimmed.starts_with("pill_web ") || trimmed.starts_with("pill_web=") {
            format!("pill_web = {{ path = \"{pill_web}\" }}")
        } else {
            line
        }
    })?;

    let mut file_handle = OpenOptions::new()
        .append(true)
        .open(&manifest)
        .context("Failed to open scratch Cargo.toml for append")?;

    // Build the appendix as a plain string to avoid write!-macro format-string
    // injection if the game path contains '{' or '}' characters.
    let appendix = format!(
        concat!(
            "\npill_game = {{ path = \"{0}\" }}\n",
            "\n[workspace]\nresolver = \"2\"\n",
            "\n[profile.release]\n",
            "opt-level = \"z\"\n",
            "lto = \"fat\"\n",
            "codegen-units = 1\n",
            "panic = \"abort\"\n",
            "strip = true\n",
            "\n[package.metadata.wasm-pack.profile.release]\n",
            "wasm-opt = [\"-Oz\", \"--strip-debug\", \"--strip-producers\", \"--enable-nontrapping-float-to-int\", \"--enable-bulk-memory\", \"--enable-sign-ext\", \"--enable-mutable-globals\", \"--enable-reference-types\"]\n",
            "\n[target.'cfg(target_arch = \"wasm32\")'.dependencies]\n",
            "lol_alloc = \"0.4\"\n",
        ),
        pill_game,
    );
    file_handle
        .write_all(appendix.as_bytes())
        .context("Failed to append to scratch Cargo.toml")?;

    Ok(())
}

fn cargo_path(path: &Path) -> String {
    path.to_string_lossy().replace("\\", "/")
}

/// Invoke wasm-pack in the scratch directory. Prefers rustup's toolchain on PATH.
fn run_wasm_pack(
    compile_mode: &CompileMode,
    scratch_pill_web_app_dir: &Path,
    scratch_package_directory: &Path,
) -> Result<()> {
    // Build `wasm-pack build --target web --out-dir <pkg_dir>`.
    let mut args: Vec<String> = vec![
        "build".into(),
        "--target".into(),
        "web".into(),
        "--out-dir".into(),
        scratch_package_directory.to_string_lossy().to_string(),
    ];
    // Non-release builds get --dev for faster iteration.
    if !matches!(compile_mode, CompileMode::Release) {
        args.push("--dev".into());
    }

    println!("Running wasm-pack in scratch crate {scratch_pill_web_app_dir:?}...");

    let mut cmd = Command::new("wasm-pack");
    cmd.args(&args).current_dir(scratch_pill_web_app_dir);
    // Prepend ~/.cargo/bin to PATH so rustup's toolchain shadows any
    // system-installed rustc that may lack the wasm32 target.
    if let Some(home) = env::var_os("HOME") {
        let cargo_bin = PathBuf::from(home).join(".cargo").join("bin");
        let existing = env::var_os("PATH").unwrap_or_default();
        let mut parts: Vec<PathBuf> = vec![cargo_bin];
        parts.extend(env::split_paths(&existing));
        if let Ok(joined) = env::join_paths(parts) {
            cmd.env("PATH", joined);
        }
    }
    // Give a clear install hint if wasm-pack is not installed.
    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::msg("wasm-pack not found on PATH. Install it with: cargo install wasm-pack")
        } else {
            Error::new(e).context("Failed to execute wasm-pack")
        }
    })?;

    if !status.success() {
        bail!("wasm-pack build failed (exit {:?})", status.code());
    }
    Ok(())
}

fn copy_build_outputs(
    scratch_package_directory: &Path,
    web_template_dir: &Path,
    user_web_dir: &Path,
    build_wasm_dir: &Path,
) -> Result<()> {
    // Ensure the output directory exists.
    fs::create_dir_all(build_wasm_dir)
        .with_context(|| format!("Failed to create {build_wasm_dir:?}"))?;

    // Copy the two core wasm-pack outputs: JS glue + WASM binary.
    for file in ["pill_web_app.js", "pill_web_app_bg.wasm"] {
        let source = scratch_package_directory.join(file);
        let destination = build_wasm_dir.join(file);
        fs::copy(&source, &destination).with_context(|| format!("Failed to copy {source:?} to {destination:?}"))?;
    }

    // Layer the engine's default web shell, then the game's customizations on top.
    copy_dir_files(web_template_dir, build_wasm_dir, "template")?;
    if user_web_dir.is_dir() {
        copy_dir_files(user_web_dir, build_wasm_dir, "overlay")?;
    }
    Ok(())
}

fn copy_game_assets(source_resources: &Path, destination_resources: &Path) -> Result<()> {
    // If the game has no res/ directory, there's nothing to copy.
    if !source_resources.is_dir() {
        return Ok(());
    }
    // Remove previous assets so deleted files don't linger from a prior build.
    if destination_resources.exists() {
        fs::remove_dir_all(destination_resources)
            .with_context(|| format!("Failed to clean previous res/ at {destination_resources:?}"))?;
    }
    let destination_parent = destination_resources
        .parent()
        .ok_or_else(|| Error::msg("invalid res/ destination path"))?;
    fs::create_dir_all(destination_parent)?;
    fs_extra::dir::copy(source_resources, destination_parent, &CopyOptions::new().overwrite(true))
        .with_context(|| format!("Failed to copy game res/ from {source_resources:?} to {destination_resources:?}"))?;
    Ok(())
}

fn copy_dir_files(source: &Path, destination: &Path, label: &str) -> Result<()> {
    for entry in fs::read_dir(source).with_context(|| format!("Failed to read {label} dir {source:?}"))? {
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
fn resolve_pseudo_symlink(path: &Path) -> PathBuf {
    // Check file metadata: real symlinks and large files pass through.
    let Ok(meta) = fs::symlink_metadata(path) else {
        return path.to_path_buf();
    };
    // Real symlink or file > 1KB — not a pseudo-symlink.
    if meta.file_type().is_symlink() || meta.len() > 1024 {
        return path.to_path_buf();
    }
    // Read the file content; must be a single relative path on one line.
    let Ok(content) = fs::read_to_string(path) else {
        return path.to_path_buf();
    };
    let trimmed = content.trim();
    // Empty or multi-line files are not pseudo-symlinks.
    if trimmed.is_empty() || trimmed.contains('\n') {
        return path.to_path_buf();
    }
    // Resolve the relative path against the symlink's parent directory.
    let parent = path.parent().unwrap_or(Path::new("."));
    let candidate = parent.join(trimmed);
    // Only use the resolved path if it actually points to an existing file.
    if candidate.is_file() {
        candidate
    } else {
        path.to_path_buf()
    }
}
