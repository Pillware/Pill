//! This file orchestrates WASM/WebGPU builds via wasm-pack.
//!
//! Responsibilities:
//! - build(): entry point - copies a WASM template into a scratch directory,
//!   rewrites Cargo.toml path-deps to absolute paths, runs wasm-pack, flattens
//!   outputs into build/wasm/, and prints a size report on release builds.
//! - Uses a scratch-copy strategy: nothing is written under engine/ during a
//!   WASM build, keeping the workspace pristine across multi-project use.
//! - Handles pseudo-symlinks (Git on Windows without core.symlinks).

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{bail, Context, Error, Result};

use crate::types::CompileMode;
use crate::utils::common::{
    ansi_green, copy_dir_files, copy_project_assets, embed_project_config, format_elapsed_time,
    get_template_directory, prepare_scratch_crate, rewrite_scratch_manifest,
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
    let build_start = Instant::now();

    println!("Building WASM target...");
    if *compile_mode == CompileMode::HotReload {
        println!("NOTE: hot-reload is not meaningful for WASM; using --dev mode.");
    }

    // Paths: templates are in pill_launcher/res/templates/, output goes to <project>/build/wasm/.
    let wasm_get_template_directory = get_template_directory("wasm");
    let web_get_template_directory = get_template_directory("web");
    let build_wasm_dir = project_directory_path.join("build").join("wasm");
    let scratch_pill_web_app_dir = build_wasm_dir.join(".build").join("pill_web_app");
    let scratch_package_directory = build_wasm_dir.join(".build").join("pkg");
    let scratch_target_dir = build_wasm_dir.join(".build").join("target");

    // Clean stale source and package artifacts from previous builds to
    // prevent contamination (stale crates, old API code referencing removed
    // assets).  Keep the target/ directory so compiled dependencies are
    // cached across rebuilds.
    if scratch_pill_web_app_dir.exists() {
        fs::remove_dir_all(&scratch_pill_web_app_dir)
            .context("Failed to clean stale pill_web_app directory")?;
    }
    if scratch_package_directory.exists() {
        fs::remove_dir_all(&scratch_package_directory)
            .context("Failed to clean stale pkg directory")?;
    }

    // Pipeline: prepare scratch crate → embed config → rewrite manifest → wasm-pack → copy outputs.
    prepare_scratch_crate(&wasm_get_template_directory, &scratch_pill_web_app_dir)?;
    embed_project_config(project_directory_path, &scratch_pill_web_app_dir)?;
    rewrite_scratch_manifest(&scratch_pill_web_app_dir, project_directory_path)?;

    run_wasm_pack(
        compile_mode,
        &scratch_pill_web_app_dir,
        &scratch_package_directory,
        &scratch_target_dir,
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

    // Size budget + optional twiggy analysis on release builds.
    if *compile_mode == CompileMode::Release {
        let final_wasm = build_wasm_dir.join("pill_web_app_bg.wasm");
        if let Some(limit) = max_size_kb {
            let limit_bytes = limit * 1024;
            let actual = fs::metadata(&final_wasm)
                .context("Cannot stat final WASM")?
                .len();
            if actual > limit_bytes {
                bail!(
                    "WASM binary {} exceeds budget {}",
                    fmt_bytes(actual),
                    fmt_bytes(limit_bytes)
                );
            }
            println!(
                "Size guard OK ({} ≤ {})",
                fmt_bytes(actual),
                fmt_bytes(limit_bytes)
            );
        }

        if wasm_analyze {
            let pre_optimization_wasm = scratch_target_dir
                .join("wasm32-unknown-unknown")
                .join("release")
                .join("pill_web_app.wasm");
            print_wasm_size_report(&final_wasm, &pre_optimization_wasm);
        }
    }

    println!();
    let elapsed = build_start.elapsed();
    let time_str = format_elapsed_time(elapsed);
    let (open, close) = ansi_green();

    let final_wasm = build_wasm_dir.join("pill_web_app_bg.wasm");
    let size_str = match fs::metadata(&final_wasm) {
        Ok(meta) => fmt_bytes(meta.len()),
        Err(_) => "unknown".to_string(),
    };

    println!("{open}Build completed successfully {time_str}{close}");
    println!("Build size: {size_str}");
    Ok(())
}

/// Invoke wasm-pack in the scratch directory. Prefers rustup's toolchain on PATH.
fn run_wasm_pack(
    compile_mode: &CompileMode,
    scratch_pill_web_app_dir: &Path,
    scratch_package_directory: &Path,
    scratch_target_dir: &Path,
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
    // Isolate cargo target directory per project so artifacts from different
    // projects (which share the crate name "project") never collide.
    args.push("--".into());
    args.push("--target-dir".into());
    args.push(scratch_target_dir.to_string_lossy().to_string());

    println!("Running WASM-pack in scratch crate {scratch_pill_web_app_dir:?}...");

    let mut command = Command::new("wasm-pack");
    command.args(&args).current_dir(scratch_pill_web_app_dir);
    // getrandom >=0.3 requires explicit opt-in for wasm32-unknown-unknown.
    command.env("RUSTFLAGS", "--cfg getrandom_backend=\"wasm_js\"");
    if let Some(home) = env::var_os("HOME") {
        let cargo_bin = PathBuf::from(home).join(".cargo").join("bin");
        let existing = env::var_os("PATH").unwrap_or_default();
        let mut parts: Vec<PathBuf> = vec![cargo_bin];
        parts.extend(env::split_paths(&existing));
        if let Ok(joined) = env::join_paths(parts) {
            command.env("PATH", joined);
        }
    }
    let status = command.status().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::msg("wasm-pack not found on PATH. Install it with: cargo install wasm-pack")
        } else {
            Error::new(error).context("Failed to execute wasm-pack")
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

/// Print a side-by-side size report comparing the final (post-wasm-opt) and
/// pre-optimization WASM binaries.  The pre-opt binary retains function-name
/// sections that wasm-opt strips, so crate-level attribution is only available
/// from the pre-opt analysis.  Requires `twiggy` on PATH (cargo install twiggy).
fn print_wasm_size_report(final_wasm: &Path, pre_optimization_wasm: &Path) {
    let Ok(final_size) = fs::metadata(final_wasm).map(|metadata| metadata.len()) else {
        return;
    };
    let pre_optimization_size = fs::metadata(pre_optimization_wasm)
        .ok()
        .map(|metadata| metadata.len());

    // Summary line with savings.
    println!();
    match pre_optimization_size {
        Some(pre_optimization_bytes) => {
            let saved = pre_optimization_bytes.saturating_sub(final_size);
            let saved_percentage = if pre_optimization_bytes > 0 {
                100.0 * saved as f64 / pre_optimization_bytes as f64
            } else {
                0.0
            };
            println!(
                "wasm size: final {} | pre-opt {} (wasm-opt: −{}, −{:.1}%)",
                fmt_bytes(final_size),
                fmt_bytes(pre_optimization_bytes),
                fmt_bytes(saved),
                saved_percentage
            );
        }
        None => println!("wasm size: {}", fmt_bytes(final_size)),
    }

    // Pre-optimization analysis — the only one with crate-level attribution.
    let pre_items =
        pre_optimization_size.and_then(|size| match collect_twiggy_items(pre_optimization_wasm) {
            Ok(items) if !items.is_empty() => Some((size, items)),
            _ => None,
        });

    if let Some((pre_size, ref pre_items)) = pre_items {
        let pre_by_crate = aggregate_by_crate(pre_items);
        print_crate_table(
            "Crate breakdown (pre-optimization, where attribution is available)",
            pre_size,
            &pre_by_crate,
        );
        print_symbol_table("Top symbols (pre-optimization)", pre_size, pre_items);
    } else {
        println!();
        println!("(install twiggy for per-crate breakdown: cargo install twiggy)");
    }

    // Final analysis — coarse sections only (code, data, wasm-meta).
    if let Ok(final_items) = collect_twiggy_items(final_wasm) {
        if !final_items.is_empty() {
            let final_by_crate = aggregate_by_crate(&final_items);
            println!();
            println!(
                "--- Final binary layout (post-optimization, no function-name attribution) ---"
            );
            println!();
            // Show the meaningful WASM sections: code, data, and meta.
            let section_order = ["code", "data", "export", "[wasm-meta]", "[custom]"];
            let lookup: HashMap<&str, u64> = final_by_crate
                .iter()
                .map(|(k, v)| (k.as_str(), *v))
                .collect();
            let mut accounted: u64 = 0;
            println!("  {:<20} {:>10} {:>7}", "section", "size", "%");
            for section in &section_order {
                if let Some(bytes) = lookup.get(section).copied() {
                    accounted += bytes;
                    println!(
                        "    {:<18} {:>10} {:>6.1}%",
                        section,
                        fmt_bytes(bytes),
                        pct(bytes, final_size)
                    );
                }
            }
            if accounted < final_size {
                let remainder = final_size - accounted;
                println!(
                    "    {:<18} {:>10} {:>6.1}%",
                    "(other)",
                    fmt_bytes(remainder),
                    pct(remainder, final_size)
                );
            }
            println!(
                "    {:<18} {:>10} {:>6} ",
                "---",
                fmt_bytes(final_size),
                "100%"
            );
            print_symbol_table("Top symbols (final)", final_size, &final_items);
        }
    }
}

/// Collect twiggy items for a WASM binary.  Returns empty Vec if twiggy is not
/// installed or fails.
fn collect_twiggy_items(wasm_path: &Path) -> Result<Vec<(u64, String)>, TwiggyError> {
    let output = match Command::new("twiggy")
        .args(["top", "-n", "15000"])
        .arg(wasm_path)
        .stderr(Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(TwiggyError::NotFound),
        Err(_) => return Err(TwiggyError::Failed),
    };
    if !output.status.success() {
        return Err(TwiggyError::Failed);
    }
    let stdout = String::from_utf8(output.stdout).map_err(|_| TwiggyError::Failed)?;
    let items = parse_twiggy(&stdout);
    Ok(items)
}

enum TwiggyError {
    NotFound,
    Failed,
}

/// Aggregate twiggy items into crate-family buckets.
fn aggregate_by_crate(items: &[(u64, String)]) -> Vec<(String, u64)> {
    let mut by_crate: HashMap<String, u64> = HashMap::new();
    for (bytes, name) in items {
        *by_crate.entry(classify_crate(name)).or_insert(0) += *bytes;
    }
    let mut groups: Vec<(String, u64)> = by_crate.into_iter().collect();
    groups.sort_by(|a, b| b.1.cmp(&a.1));
    groups
}

/// Print a unified crate-size table with engine libs, project, 3rd-party
/// crates, and WASM internal sections.
///
/// Percentages use twiggy's retained-size model — items may overlap, so
/// percentages can sum beyond 100%.  Use the relative sizes, not the absolute
/// percentages, to compare categories.
fn print_crate_table(title: &str, total: u64, by_crate: &[(String, u64)]) {
    const ENGINE_LIBS: &[&str] = &["pill_engine", "pill_renderer", "pill_core", "pill_web"];
    const PROJECT_KEY: &str = "project";
    const PROJECT_ASSETS_KEY: &str = "[project-assets]";
    // WASM internal / metadata sections (not crates, but useful for breakdown).
    // `export` is twiggy's accounting for exported symbols — they live inside
    // code/data, so the export percentage overlaps with those sections.
    const WASM_SECTIONS: &[&str] = &[
        "[wasm-bindgen]",
        "[debug:names]",
        "[rust-std]",
        "[rodata]",
        "[wasm-meta]",
        "[custom]",
        "export",
    ];
    // Items to never display (unhelpful catch-all).
    const SKIP: &[&str] = &["[other]"];

    let lookup: HashMap<&str, u64> = by_crate.iter().map(|(k, v)| (k.as_str(), *v)).collect();

    println!();
    println!("--- {title} (% of {}) ---", fmt_bytes(total));
    println!();

    // -- Engine libs ----------------------------------------------------
    let engine_total: u64 = ENGINE_LIBS
        .iter()
        .map(|k| lookup.get(k).copied().unwrap_or(0))
        .sum();
    println!("  {:<28} {:>10} {:>7}", "Engine libs", "size", "%");
    for lib in ENGINE_LIBS {
        let bytes = lookup.get(lib).copied().unwrap_or(0);
        println!(
            "    {:<26} {:>10} {:>6.1}%",
            *lib,
            fmt_bytes(bytes),
            pct(bytes, total)
        );
    }
    println!(
        "    {:<26} {:>10} {:>6.1}%  ← engine total",
        "---",
        fmt_bytes(engine_total),
        pct(engine_total, total)
    );

    // -- Project --------------------------------------------------------
    let project_bytes = lookup.get(PROJECT_KEY).copied().unwrap_or(0);
    let project_assets = lookup.get(PROJECT_ASSETS_KEY).copied().unwrap_or(0);
    println!();
    println!("  {:<28} {:>10} {:>7}", "Project", "size", "%");
    println!(
        "    {:<26} {:>10} {:>6.1}%  (logic)",
        PROJECT_KEY,
        fmt_bytes(project_bytes),
        pct(project_bytes, total)
    );
    println!(
        "    {:<26} {:>10} {:>6.1}%  (embedded assets)",
        PROJECT_ASSETS_KEY,
        fmt_bytes(project_assets),
        pct(project_assets, total)
    );

    // -- 3rd-party crates -----------------------------------------------
    let excluded: Vec<&str> = ENGINE_LIBS
        .iter()
        .chain([PROJECT_KEY, PROJECT_ASSETS_KEY].iter())
        .chain(WASM_SECTIONS.iter())
        .chain(SKIP.iter())
        .copied()
        .collect();
    let crates: Vec<_> = by_crate
        .iter()
        .filter(|(k, _)| !excluded.contains(&k.as_str()) && !k.starts_with('['))
        .take(15)
        .collect();
    if !crates.is_empty() {
        let crate_total: u64 = crates.iter().map(|(_, b)| *b).sum();
        println!();
        println!("  {:<28} {:>10} {:>7}", "3rd-party crates", "size", "%");
        for (name, bytes) in &crates {
            println!(
                "    {:<26} {:>10} {:>6.1}%",
                name,
                fmt_bytes(*bytes),
                pct(*bytes, total)
            );
        }
        println!(
            "    {:<26} {:>10} {:>6.1}%  ← crate subtotal",
            "---",
            fmt_bytes(crate_total),
            pct(crate_total, total)
        );
    }

    // -- WASM internal sections -----------------------------------------
    let sections: Vec<_> = WASM_SECTIONS
        .iter()
        .filter_map(|s| lookup.get(s).map(|b| (*s, *b)))
        .filter(|(_, b)| *b > 0)
        .collect();
    if !sections.is_empty() {
        let section_total: u64 = sections.iter().map(|(_, b)| *b).sum();
        println!();
        println!("  {:<28} {:>10} {:>7}", "WASM internals", "size", "%");
        for (name, bytes) in &sections {
            let note = if *name == "export" {
                "  (overlaps code/data)"
            } else {
                ""
            };
            println!(
                "    {:<26} {:>10} {:>6.1}%{}",
                name,
                fmt_bytes(*bytes),
                pct(*bytes, total),
                note
            );
        }
        println!(
            "    {:<26} {:>10} {:>6.1}%  ← section subtotal",
            "---",
            fmt_bytes(section_total),
            pct(section_total, total)
        );
    }
}

/// Helper: percentage with zero-guard.
fn pct(bytes: u64, total: u64) -> f64 {
    if total > 0 {
        100.0 * bytes as f64 / total as f64
    } else {
        0.0
    }
}

/// Print the top symbols from a twiggy analysis.
fn print_symbol_table(title: &str, total: u64, items: &[(u64, String)]) {
    println!();
    println!("--- {title} ---",);
    println!();
    for (bytes, name) in items.iter().take(10) {
        let percentage = 100.0 * *bytes as f64 / total as f64;
        let display = truncate_display(name, 72);
        println!(
            "  {:>10} {:>5.1}%  {}",
            fmt_bytes(*bytes),
            percentage,
            display
        );
    }
}

/// Parse twiggy's default text output. Each data row:
///   "   <bytes> ┊ <percentage>% ┊ <item name>"
fn parse_twiggy(stdout: &str) -> Vec<(u64, String)> {
    let mut items = Vec::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Shallow")
            || trimmed.starts_with('-')
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
        if name.contains("project") {
            return "[project-assets]".into();
        }
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
