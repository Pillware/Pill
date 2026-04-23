//! WASM/WebGPU build for `-a build -t wasm`. Copies the wasm crate template
//! into a scratch dir inside the game directory, rewrites path-deps to
//! absolute paths (engine crates + the game at -p), runs wasm-pack, flattens
//! outputs, and prints a size report on release builds.
//!
//! The scratch-copy strategy keeps the engine dir pristine across multi-game
//! use — nothing is written under engine/ during a wasm build.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Error, Result};
use fs_extra::dir::CopyOptions;

use crate::{get_path, modify_file, size_report, CompileMode, Location};

pub fn build(game_project_directory_path: &Path, compile_mode: &CompileMode) -> Result<()> {
    println!("Building WASM/WebGPU target for game project at {game_project_directory_path:?}...");
    if *compile_mode == CompileMode::HotReload {
        println!("Note: hot-reload is not meaningful for WASM; using --dev mode.");
    }

    let wasm_template_dir = template_dir("wasm");
    let web_template_dir = template_dir("web");
    let build_wasm_dir = game_project_directory_path.join("build").join("wasm");
    let scratch_pill_web_dir = build_wasm_dir.join(".build").join("pill_web");
    let scratch_pkg_dir = build_wasm_dir.join(".build").join("pkg");

    prepare_scratch_crate(&wasm_template_dir, &scratch_pill_web_dir)?;
    embed_game_config(game_project_directory_path, &scratch_pill_web_dir)?;
    rewrite_scratch_manifest(&scratch_pill_web_dir, game_project_directory_path)?;
    run_wasm_pack(compile_mode, &scratch_pill_web_dir, &scratch_pkg_dir)?;
    copy_build_outputs(
        &scratch_pkg_dir,
        &web_template_dir,
        &game_project_directory_path.join("web"),
        &build_wasm_dir,
    )?;
    copy_game_assets(
        &game_project_directory_path.join("res"),
        &build_wasm_dir.join("res"),
    )?;

    // Size report — only meaningful on release (debug wasm is dominated by debuginfo).
    if *compile_mode == CompileMode::Release {
        let preopt_wasm = scratch_pill_web_dir
            .join("target")
            .join("wasm32-unknown-unknown")
            .join("release")
            .join("pill_web.wasm");
        size_report::print(&build_wasm_dir, &preopt_wasm);
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

fn prepare_scratch_crate(wasm_template_dir: &Path, scratch_pill_web_dir: &Path) -> Result<()> {
    fs::create_dir_all(scratch_pill_web_dir)
        .with_context(|| format!("Failed to create scratch dir {scratch_pill_web_dir:?}"))?;

    fs::copy(
        wasm_template_dir.join("Cargo.toml"),
        scratch_pill_web_dir.join("Cargo.toml"),
    )
    .context("Failed to copy pill_web Cargo.toml to scratch")?;

    // Share the engine workspace's Cargo.lock so the scratch build resolves
    // identical crate versions to an in-place engine build. Without this,
    // cargo picks newer wasm-bindgen/etc. which can break WebGPU rendering.
    let engine_lock = get_path(Location::EngineCrates).join("Cargo.lock");
    if engine_lock.exists() {
        fs::copy(&engine_lock, scratch_pill_web_dir.join("Cargo.lock"))
            .context("Failed to copy engine Cargo.lock into scratch")?;
    }

    let scratch_src_dir = scratch_pill_web_dir.join("src");
    if scratch_src_dir.exists() {
        fs::remove_dir_all(&scratch_src_dir).context("Failed to clean scratch src/")?;
    }
    fs_extra::dir::copy(
        wasm_template_dir.join("src"),
        scratch_pill_web_dir,
        &CopyOptions::new().overwrite(true),
    )
    .context("Failed to copy pill_web src/ to scratch")?;

    Ok(())
}

// Copy the game's res/config.ini into the scratch crate at a known location
// so the template's lib.rs can include_str! it. Needed because wasm has no
// filesystem — the engine can't read config.ini at runtime.
fn embed_game_config(game_dir: &Path, scratch_pill_web_dir: &Path) -> Result<()> {
    let src = game_dir.join("res").join("config.ini");
    let dst = scratch_pill_web_dir.join("config.ini");
    if src.is_file() {
        fs::copy(&src, &dst)
            .with_context(|| format!("Failed to embed game config {src:?} → {dst:?}"))?;
    } else {
        // Write an empty file so the template's include_str! compiles.
        fs::write(&dst, "")
            .with_context(|| format!("Failed to write empty scratch config.ini at {dst:?}"))?;
    }
    Ok(())
}

fn rewrite_scratch_manifest(scratch_pill_web_dir: &Path, game_dir: &Path) -> Result<()> {
    let engine = get_path(Location::EngineCrates);
    let pill_engine = cargo_path(&engine.join("pill_engine"));
    let pill_renderer = cargo_path(&engine.join("pill_renderer"));
    let pill_core = cargo_path(&engine.join("pill_core"));
    let pill_game = cargo_path(game_dir);

    let manifest = scratch_pill_web_dir.join("Cargo.toml");
    modify_file(&manifest, &manifest, |line: String| -> String {
        let t = line.trim_start();
        if t.starts_with("pill_engine ") || t.starts_with("pill_engine=") {
            format!(
                "pill_engine = {{ path = \"{pill_engine}\", features = [\"game\", \"internal\"] }}"
            )
        } else if t.starts_with("pill_renderer ") || t.starts_with("pill_renderer=") {
            format!("pill_renderer = {{ path = \"{pill_renderer}\" }}")
        } else if t.starts_with("pill_core ") || t.starts_with("pill_core=") {
            format!("pill_core = {{ path = \"{pill_core}\" }}")
        } else {
            line
        }
    })?;

    // Append three things the committed template omits:
    //  - pill_game dep (injected per-game, not in the template)
    //  - [workspace] + resolver = "2" so cargo doesn't walk up into the game's
    //    parent workspace; resolver matches engine/Cargo.toml (wgpu feature
    //    unification is sensitive to resolver version)
    //  - [profile.release] + wasm-pack's wasm-opt flags for Tier 1 size opt.
    //    `strip` is NOT set here: the size report analyzes the pre-opt wasm
    //    and needs the function-names subsection. wasm-opt strips the shipped
    //    binary via --strip-debug --strip-producers.
    let mut f = OpenOptions::new()
        .append(true)
        .open(&manifest)
        .context("Failed to open scratch Cargo.toml for append")?;
    write!(
        f,
        concat!(
            "\npill_game = {{ path = \"{pill_game}\" }}\n",
            "\n[workspace]\nresolver = \"2\"\n",
            "\n[profile.release]\n",
            "opt-level = \"z\"\n",
            "lto = \"fat\"\n",
            "codegen-units = 1\n",
            "panic = \"abort\"\n",
            "\n[package.metadata.wasm-pack.profile.release]\n",
            "wasm-opt = [\"-Oz\", \"--strip-debug\", \"--strip-producers\", \"--enable-nontrapping-float-to-int\", \"--enable-bulk-memory\", \"--enable-sign-ext\", \"--enable-mutable-globals\", \"--enable-reference-types\"]\n",
        ),
        pill_game = pill_game,
    )
    .context("Failed to append to scratch Cargo.toml")?;

    Ok(())
}

fn cargo_path(p: &Path) -> String {
    p.to_string_lossy().replace("\\", "/")
}

fn run_wasm_pack(
    compile_mode: &CompileMode,
    scratch_pill_web_dir: &Path,
    scratch_pkg_dir: &Path,
) -> Result<()> {
    let mut args: Vec<String> = vec![
        "build".into(),
        "--target".into(),
        "web".into(),
        "--out-dir".into(),
        scratch_pkg_dir.to_string_lossy().to_string(),
    ];
    if !matches!(compile_mode, CompileMode::Release) {
        args.push("--dev".into());
    }

    println!("Running wasm-pack in scratch crate {scratch_pill_web_dir:?}...");

    // Prefer rustup's toolchain over Homebrew's rustc — Homebrew's doesn't ship
    // the wasm32-unknown-unknown target.
    let mut cmd = Command::new("wasm-pack");
    cmd.args(&args).current_dir(scratch_pill_web_dir);
    if let Some(home) = env::var_os("HOME") {
        let cargo_bin = PathBuf::from(home).join(".cargo").join("bin");
        let existing = env::var_os("PATH").unwrap_or_default();
        let mut parts: Vec<PathBuf> = vec![cargo_bin];
        parts.extend(env::split_paths(&existing).filter(|p| p != Path::new("/opt/homebrew/bin")));
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
    scratch_pkg_dir: &Path,
    web_template_dir: &Path,
    user_web_dir: &Path,
    build_wasm_dir: &Path,
) -> Result<()> {
    fs::create_dir_all(build_wasm_dir)
        .with_context(|| format!("Failed to create {build_wasm_dir:?}"))?;

    for file in ["pill_web.js", "pill_web_bg.wasm"] {
        let src = scratch_pkg_dir.join(file);
        let dst = build_wasm_dir.join(file);
        fs::copy(&src, &dst)
            .with_context(|| format!("Failed to copy {src:?} to {dst:?}"))?;
    }

    // Default chrome from the engine template (index.html + logo + ...).
    copy_dir_files(web_template_dir, build_wasm_dir, "template")?;
    // Overlay per-game customizations; each file individually overrides the default.
    if user_web_dir.is_dir() {
        copy_dir_files(user_web_dir, build_wasm_dir, "overlay")?;
    }
    Ok(())
}

// The game's runtime res/ — meshes, textures, config.ini, etc. — must be
// served alongside the wasm for fetch()-based asset loading to succeed. Mirror
// <game>/res/ into <build_wasm_dir>/res/ on every build (overwrite so deleted
// assets don't linger).
fn copy_game_assets(src_res: &Path, dst_res: &Path) -> Result<()> {
    if !src_res.is_dir() {
        return Ok(());
    }
    if dst_res.exists() {
        fs::remove_dir_all(dst_res)
            .with_context(|| format!("Failed to clean previous res/ at {dst_res:?}"))?;
    }
    let dst_parent = dst_res
        .parent()
        .ok_or_else(|| Error::msg("invalid res/ destination path"))?;
    fs::create_dir_all(dst_parent)?;
    fs_extra::dir::copy(src_res, dst_parent, &CopyOptions::new().overwrite(true))
        .with_context(|| format!("Failed to copy game res/ from {src_res:?} to {dst_res:?}"))?;
    Ok(())
}

// Flat-copy files from `src` into `dst`, following symlinks. `label` is used
// in error messages to distinguish template vs user-overlay copies.
fn copy_dir_files(src: &Path, dst: &Path, label: &str) -> Result<()> {
    for entry in
        fs::read_dir(src).with_context(|| format!("Failed to read {label} dir {src:?}"))?
    {
        let entry = entry?;
        // path().metadata() follows symlinks so symlinked assets are copied as files.
        if entry.path().metadata()?.is_file() {
            let target = dst.join(entry.file_name());
            let entry_path = entry.path();
            fs::copy(&entry_path, &target).with_context(|| {
                format!("Failed to {label}-copy {entry_path:?} to {target:?}")
            })?;
        }
    }
    Ok(())
}
