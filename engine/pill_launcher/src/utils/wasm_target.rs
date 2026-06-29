// This file orchestrates WASM/WebGPU builds via wasm-pack.
//
// Responsibilities:
// - build(): entry point - copies a WASM template into a scratch directory,
//   rewrites Cargo.toml path-deps to absolute paths, runs wasm-pack, flattens
//   outputs into build/wasm/, and prints a size report on release builds.
// - Uses a scratch-copy strategy: nothing is written under engine/ during a
//   WASM build, keeping the workspace pristine across multi-project use.
// - Handles pseudo-symlinks (Git on Windows without core.symlinks).

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Error, Result};

use crate::types::CompileMode;
use crate::utils::common::{
    copy_dir_files, copy_project_assets, embed_project_config, get_template_directory,
    prepare_scratch_crate, rewrite_scratch_manifest,
};

/// Build a WASM/WebGPU bundle via wasm-pack using a scratch directory.
/// On release builds, enforces an optional size budget.
/// When `wasm_analyze` is true, prints a twiggy-powered size breakdown after build.
pub fn build_project(
    project_directory_path: &Path,
    compile_mode: &CompileMode,
    max_size_kb: Option<u64>,
    wasm_analyze: bool,
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

    // The WASM scratch app joins the engine workspace (via workspace = "..."
    // in its generated Cargo.toml).  Temporarily add it to engine/Cargo.toml's
    // members list so cargo accepts it as a workspace member.
    let _member_guard = ScratchMemberGuard::add(&scratch_pill_web_app_dir)?;

    run_wasm_pack(
        compile_mode,
        &scratch_pill_web_app_dir,
        &scratch_package_directory,
    )?;

    drop(_member_guard);

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

    // Size budget + optional twiggy analysis on release builds.
    if *compile_mode == CompileMode::Release {
        let final_wasm = build_wasm_dir.join("pill_web_app_bg.wasm");
        if let Some(limit) = max_size_kb {
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

        if wasm_analyze {
            print_wasm_size_report(&final_wasm);
        }
    }

    println!();
    println!("Done! Serve with:");
    println!("  PillLauncher run -t web -p {project_directory_path:?}");
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
    // getrandom >=0.3 requires explicit opt-in for wasm32-unknown-unknown.
    cmd.env("RUSTFLAGS", "--cfg getrandom_backend=\"wasm_js\"");
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

// ---------------------------------------------------------------------------
// WASM size report (twiggy-backed, opt-in via --wasm-analyze)
// ---------------------------------------------------------------------------

/// Print a per-crate size breakdown and top-symbols report for a WASM binary.
/// Requires `twiggy` on PATH (cargo install twiggy); prints a hint if missing.
fn print_wasm_size_report(wasm_path: &Path) {
    let Ok(total) = fs::metadata(wasm_path).map(|m| m.len()) else {
        return;
    };

    println!();
    println!("--- WASM size analysis ({}) ---", fmt_bytes(total));

    match run_twiggy_analysis(wasm_path, total) {
        TwiggyResult::NoTwiggy => {
            println!("(install twiggy: cargo install twiggy)");
        }
        TwiggyResult::Done => {}
        TwiggyResult::Empty | TwiggyResult::Error => {}
    }
}

enum TwiggyResult {
    Done,
    Empty,
    Error,
    NoTwiggy,
}

fn run_twiggy_analysis(wasm_path: &Path, total: u64) -> TwiggyResult {
    let output = match Command::new("twiggy")
        .args(["top", "-n", "15000"])
        .arg(wasm_path)
        .stderr(Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return TwiggyResult::NoTwiggy,
        Err(_) => return TwiggyResult::Error,
    };
    if !output.status.success() {
        return TwiggyResult::Error;
    }
    let Ok(stdout) = String::from_utf8(output.stdout) else {
        return TwiggyResult::Error;
    };

    let items = parse_twiggy(&stdout);
    if items.is_empty() {
        return TwiggyResult::Empty;
    }

    let mut by_crate: HashMap<String, u64> = HashMap::new();
    for (bytes, name) in &items {
        *by_crate.entry(classify_crate(name)).or_insert(0) += *bytes;
    }
    let mut groups: Vec<(String, u64)> = by_crate.clone().into_iter().collect();
    groups.sort_by(|a, b| b.1.cmp(&a.1));

    const ENGINE_LIBS: &[&str] = &["pill_engine", "pill_renderer", "pill_core", "pill_web"];
    let engine_total: u64 = ENGINE_LIBS
        .iter()
        .map(|k| by_crate.get(*k).copied().unwrap_or(0))
        .sum();

    println!();
    println!("  Engine libs (% of {}):", fmt_bytes(total));
    println!("    {:<20} {:>10} {:>7}", "crate", "size", "%");
    for lib in ENGINE_LIBS {
        let bytes = by_crate.get(*lib).copied().unwrap_or(0);
        let pct = 100.0 * bytes as f64 / total as f64;
        println!("    {:<20} {:>10} {:>6.1}%", lib, fmt_bytes(bytes), pct);
    }
    let epct = 100.0 * engine_total as f64 / total as f64;
    println!(
        "    {:<20} {:>10} {:>6.1}%  ← engine total",
        "---",
        fmt_bytes(engine_total),
        epct
    );

    println!();
    println!("  3rd party (top 15):");
    println!("    {:<20} {:>10} {:>7}", "crate", "size", "%");
    let excluded: Vec<&str> = ENGINE_LIBS.iter().copied().collect();
    for (crate_name, bytes) in groups
        .iter()
        .filter(|(k, _)| !excluded.contains(&k.as_str()))
        .take(15)
    {
        let pct = 100.0 * *bytes as f64 / total as f64;
        println!(
            "    {:<20} {:>10} {:>6.1}%",
            crate_name,
            fmt_bytes(*bytes),
            pct
        );
    }

    println!();
    println!("  Top 10 symbols:");
    for (bytes, name) in items.iter().take(10) {
        let pct = 100.0 * *bytes as f64 / total as f64;
        let display = truncate_display(name, 72);
        println!("  {:>10} {:>5.1}%  {}", fmt_bytes(*bytes), pct, display);
    }

    TwiggyResult::Done
}

/// Parse twiggy's default text output. Each data row:
///   "   <bytes> ┊ <pct>% ┊ <item name>"
fn parse_twiggy(stdout: &str) -> Vec<(u64, String)> {
    let mut items = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Shallow")
            || trimmed.starts_with('─')
            || trimmed.starts_with('Σ')
            || (trimmed.contains("and ") && trimmed.contains("more"))
        {
            continue;
        }
        let parts: Vec<&str> = trimmed.split('┊').collect();
        if parts.len() < 3 {
            continue;
        }
        let Ok(bytes) = parts[0].trim().parse::<u64>() else {
            continue;
        };
        if bytes == 0 {
            continue;
        }
        items.push((bytes, parts[2].trim().to_string()));
    }
    items
}

fn truncate_display(name: &str, max_chars: usize) -> String {
    if name.chars().count() > max_chars {
        let head: String = name.chars().take(max_chars - 3).collect();
        format!("{head}...")
    } else {
        name.to_string()
    }
}

fn fmt_bytes(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let f = n as f64;
    if f >= MB {
        format!("{:.2} MB", f / MB)
    } else if f >= KB {
        format!("{:.1} KB", f / KB)
    } else {
        format!("{n} B")
    }
}

/// Coarse bucketing of twiggy item names into crate families.
fn classify_crate(name: &str) -> String {
    if name.contains(".rodata") || name.contains("data segment") {
        return "[rodata]".into();
    }
    if name.contains("function names") {
        return "[debug:names]".into();
    }
    if name.contains("__wasm_bindgen") {
        return "[wasm-bindgen]".into();
    }
    if name.contains("custom section") {
        return "[custom]".into();
    }
    if name.starts_with("elem[")
        || name.starts_with("type[")
        || name.starts_with("import ")
        || name.starts_with("table[")
    {
        return "[wasm-meta]".into();
    }

    let rest = name.strip_prefix('<').unwrap_or(name);
    let rest = rest
        .strip_prefix("&mut ")
        .or_else(|| rest.strip_prefix('&'))
        .unwrap_or(rest);
    let ident: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();

    if ident.is_empty() {
        return "[other]".into();
    }
    match ident.as_str() {
        "core" | "alloc" | "std" | "compiler_builtins" | "rustc_demangle" | "dlmalloc" | "str"
        | "bool" | "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "f32" | "f64"
        | "char" | "usize" | "isize" | "T" => "[rust-std]".into(),
        "jpeg_decoder" | "png" | "tiff" | "gif" | "weezl" | "miniz_oxide" | "color_quant"
        | "qoi" | "exr" => "image".into(),
        "epaint" | "emath" | "egui_wgpu" | "egui_winit" => "egui".into(),
        "codespan_reporting" | "codespan" | "pp_rs" | "spirv" => "naga".into(),
        "wgpu_hal" | "wgpu_core" | "wgpu_types" => "wgpu".into(),
        "js_sys" => "web_sys".into(),
        _ => ident,
    }
}

// ---------------------------------------------------------------------------
// Scratch member guard for WASM builds
// ---------------------------------------------------------------------------

/// Temporarily adds the WASM scratch directory to engine/Cargo.toml's
/// workspace members so cargo accepts the scratch app as a workspace member.
/// Restores the original manifest on drop.
struct ScratchMemberGuard {
    manifest_path: PathBuf,
    original: String,
}

impl ScratchMemberGuard {
    fn add(scratch_dir: &Path) -> Result<Self> {
        use crate::types::Location;
        use crate::utils::paths::{get_path, PROJECT_CRATE_MARKER};

        let engine_manifest = get_path(Location::EngineCrates).join("Cargo.toml");
        let original = fs::read_to_string(&engine_manifest)
            .with_context(|| format!("Failed to read {}", engine_manifest.display()))?;

        let normalized = scratch_dir.to_string_lossy().replace('\\', "/");
        let member_line = format!("    \"{normalized}\", {PROJECT_CRATE_MARKER}");

        let mut in_members = false;
        let mut patched = String::with_capacity(original.len() + member_line.len() + 2);
        let mut inserted = false;
        for line in original.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("members") && trimmed.contains('[') {
                in_members = true;
            }
            if in_members && !inserted && trimmed == "]" {
                patched.push_str(&member_line);
                patched.push('\n');
                inserted = true;
            }
            patched.push_str(line);
            patched.push('\n');
        }
        if !inserted {
            bail!("Could not find closing `]` of workspace members array");
        }

        fs::write(&engine_manifest, patched.trim_end())
            .with_context(|| format!("Failed to update {}", engine_manifest.display()))?;

        Ok(Self {
            manifest_path: engine_manifest,
            original,
        })
    }
}

impl Drop for ScratchMemberGuard {
    fn drop(&mut self) {
        let _ = fs::write(&self.manifest_path, &self.original);
    }
}
