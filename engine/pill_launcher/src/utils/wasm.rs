// This file orchestrates WASM/WebGPU builds via wasm-pack.
//
// Responsibilities:
// - build(): entry point — copies a WASM template into a scratch directory,
//   rewrites Cargo.toml path-deps to absolute paths, runs wasm-pack, flattens
//   outputs into build/wasm/, and prints a size report on release builds.
// - Uses a scratch-copy strategy: nothing is written under engine/ during a
//   WASM build, keeping the workspace pristine across multi-project use.
// - Handles pseudo-symlinks (Git on Windows without core.symlinks).

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Error, Result};

use crate::types::CompileMode;
use crate::utils::common::{
    copy_dir_files, copy_project_assets, embed_project_config, get_template_directory,
    prepare_scratch_crate, rewrite_scratch_manifest,
};

/// Build a WASM/WebGPU bundle via wasm-pack using a scratch directory.
/// On release builds, prints a size report and enforces an optional size budget.
pub fn build_project(
    project_directory_path: &Path,
    compile_mode: &CompileMode,
    max_size_kb: Option<u64>,
) -> Result<()> {
    println!("Building WASM/WebGPU target for project at {project_directory_path:?}...");
    if *compile_mode == CompileMode::HotReload {
        println!("Note: hot-reload is not meaningful for WASM; using --dev mode.");
    }

    // Paths: templates are in pill_launcher/res/templates/, output goes to <project>/build/wasm/.
    let wasm_get_template_directory = get_template_directory("wasm");
    let web_get_template_directory = get_template_directory("web");
    let build_wasm_dir = project_directory_path.join("build").join("wasm");
    let scratch_pill_web_app_dir = build_wasm_dir.join(".build").join("pill_web_app");
    let scratch_package_directory = build_wasm_dir.join(".build").join("pkg");

    // Pipeline: prepare scratch crate → embed config → rewrite manifest → wasm-pack → copy outputs.
    prepare_scratch_crate(&wasm_get_template_directory, &scratch_pill_web_app_dir)?;
    embed_project_config(project_directory_path, &scratch_pill_web_app_dir)?;
    rewrite_scratch_manifest(&scratch_pill_web_app_dir, project_directory_path)?;
    run_wasm_pack(
        compile_mode,
        &scratch_pill_web_app_dir,
        &scratch_package_directory,
    )?;
    copy_build_outputs(
        &scratch_package_directory,
        &web_get_template_directory,
        &project_directory_path.join("web"),
        &build_wasm_dir,
    )?;
    copy_project_assets(
        &project_directory_path.join("res"),
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
    println!("  PillLauncher -a run -t wasm -p {project_directory_path:?}");
    println!("  (or any static server pointed at {build_wasm_dir:?})");
    Ok(())
}

/// Invoke wasm-pack in the scratch directory. Prefers rustup's toolchain on PATH.
fn run_wasm_pack(
    compile_mode: &CompileMode,
    scratch_pill_web_app_dir: &Path,
    scratch_package_directory: &Path,
) -> Result<()> {
    let mut args: Vec<String> = vec![
        "build".into(),
        "--target".into(),
        "web".into(),
        "--out-dir".into(),
        scratch_package_directory.to_string_lossy().to_string(),
    ];
    if !matches!(compile_mode, CompileMode::Release) {
        args.push("--dev".into());
    }

    println!("Running wasm-pack in scratch crate {scratch_pill_web_app_dir:?}...");

    let mut cmd = Command::new("wasm-pack");
    cmd.args(&args).current_dir(scratch_pill_web_app_dir);
    if let Some(home) = env::var_os("HOME") {
        let cargo_bin = PathBuf::from(home).join(".cargo").join("bin");
        let existing = env::var_os("PATH").unwrap_or_default();
        let mut parts: Vec<PathBuf> = vec![cargo_bin];
        parts.extend(env::split_paths(&existing));
        if let Ok(joined) = env::join_paths(parts) {
            cmd.env("PATH", joined);
        }
    }
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
    web_get_template_directory: &Path,
    user_web_dir: &Path,
    build_wasm_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(build_wasm_dir)
        .with_context(|| format!("Failed to create {build_wasm_dir:?}"))?;

    for file in ["pill_web_app.js", "pill_web_app_bg.wasm"] {
        let source = scratch_package_directory.join(file);
        let destination = build_wasm_dir.join(file);
        fs::copy(&source, &destination)
            .with_context(|| format!("Failed to copy {source:?} to {destination:?}"))?;
    }

    copy_dir_files(web_get_template_directory, build_wasm_dir, "template")?;
    if user_web_dir.is_dir() {
        copy_dir_files(user_web_dir, build_wasm_dir, "overlay")?;
    }
    Ok(())
}
