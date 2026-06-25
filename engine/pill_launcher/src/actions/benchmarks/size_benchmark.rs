// This file implements the "size-benchmark" action: build + artifact size analysis.
//
// Responsibilities:
// - Builds the game project for the given target (native or WASM).
// - Analyzes the final build artifact sizes:
//   - Native: executable, dynamic libraries, resource directories.
//   - WASM: final and pre-optimization .wasm sizes, twiggy per-crate breakdown.
// - Prints a formatted size report to the console.

use anyhow::Result;
use clap::{App, ArgMatches};
use path_absolutize::Absolutize;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::{
    parse_build_target, path_flag, target_flag,
};
use crate::utils::paths::get_game_build_path;

#[derive(Debug)]
pub(crate) struct SizeBenchmark;

impl Action for SizeBenchmark {
    fn name(&self) -> &'static str {
        "size-benchmark"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app.arg(path_flag())
            .arg(target_flag())
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let target = parse_build_target(matches);
        // Always release — debug sizes are dominated by debuginfo and meaningless.
        let compile_mode = CompileMode::Release;
        do_size_benchmark(&path, &compile_mode, &target)
    }
}

/// Build the game and print a size report for the final artifact.
pub(crate) fn do_size_benchmark(
    game_project_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    target: &BuildTarget,
) -> Result<()> {
    println!(
        "Size benchmark: {} (target: {}, compile mode: release)",
        game_project_directory_path.display(),
        target,
    );

    match target {
        BuildTarget::Native => {
            let mut output_directory_path = PathBuf::from(".");
            output_directory_path = get_game_build_path(
                game_project_directory_path,
                &output_directory_path,
                compile_mode,
            )?;

            println!("Building native target...");
            crate::actions::build::build_game_project(
                game_project_directory_path,
                &output_directory_path,
                compile_mode,
                None,
            )?;

            print_native_report(&output_directory_path);
        }
        BuildTarget::Web => {
            println!("Building WASM target...");
            crate::utils::wasm::build(game_project_directory_path, compile_mode, None)?;

            let build_wasm_dir = game_project_directory_path.join("build").join("wasm");
            let pre_optimization_wasm = build_wasm_dir
                .join(".build")
                .join("pill_web_app")
                .join("target")
                .join("wasm32-unknown-unknown")
                .join(match compile_mode {
                    CompileMode::Release => "release",
                    _ => "debug",
                })
                .join("pill_web_app.wasm");

            if pre_optimization_wasm.exists() {
                print_wasm_report(&build_wasm_dir, &pre_optimization_wasm);
            } else {
                println!("Note: pre-optimization WASM not found, skipping twiggy analysis.");
                println!(
                    "Final WASM: {}",
                    build_wasm_dir.join("pill_web_app_bg.wasm").display()
                );
            }
        }
    }

    Ok(())
}

// ============================================================================
// Native size report
// ============================================================================

/// Print a native build size report: executable, dynamic libraries, and
/// resource directory sizes with a breakdown.
fn print_native_report(build_output_dir: &Path) {
    println!();
    println!("=== Native Build Size Report ===");
    println!();

    let data_directory = build_output_dir.join("data");
    let executable_ext = if cfg!(target_os = "windows") {
        ".exe"
    } else {
        ""
    };

    let mut executable_size: Option<(String, u64)> = None;
    let mut dynamic_library_sizes: Vec<(String, u64)> = Vec::new();

    if let Ok(entries) = fs::read_dir(build_output_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name.ends_with(executable_ext) && !name.starts_with("pill_") {
                    if let Ok(meta) = path.metadata() {
                        executable_size = Some((name, meta.len()));
                    }
                }
            }
        }
    }

    // Measure dynamic libraries in data/
    if data_directory.exists() {
        if let Ok(entries) = fs::read_dir(&data_directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    if name.contains("pill_")
                        && (name.contains(".dll")
                            || name.contains(".so")
                            || name.contains(".dylib"))
                    {
                        if let Ok(meta) = path.metadata() {
                            dynamic_library_sizes.push((name, meta.len()));
                        }
                    }
                }
            }
        }
    }

    // Summary table
    println!("| {:<30} | {:<12} |", "Component", "Size");
    println!("|{:-^32}|{:-^14}|", "", "");

    let mut total: u64 = 0;

    if let Some((ref name, size)) = executable_size {
        println!(
            "| {:<30} | {:>10} |",
            format!("Executable ({name})"),
            format_bytes(size)
        );
        total += size;
    }

    for (ref name, size) in &dynamic_library_sizes {
        println!(
            "| {:<30} | {:>10} |",
            format!("Library ({name})"),
            format_bytes(*size)
        );
        total += *size;
    }

    // Measure data/res/
    let resources_directory = data_directory.join("res");
    if resources_directory.exists() {
        let resources_size = dir_size(&resources_directory);
        if resources_size > 0 {
            println!(
                "| {:<30} | {:>10} |",
                "Resources (data/res/)",
                format_bytes(resources_size)
            );
            total += resources_size;
        }

        // Breakdown of resource subdirectories
        if let Ok(entries) = fs::read_dir(&resources_directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if path.is_dir() {
                    let sub_size = dir_size(&path);
                    if sub_size > 0 {
                        println!(
                            "| {:<30} | {:>10} |",
                            format!("  └─ {name}/"),
                            format_bytes(sub_size)
                        );
                    }
                }
            }
        }
    }

    println!("|{:-^32}|{:-^14}|", "", "");
    println!(
        "| {:<30} | {:>10} |",
        "TOTAL",
        format_bytes(total)
    );
    println!();
}

/// Recursively compute the total byte size of a directory.
/// Uses canonicalize to detect and skip symlink cycles.
fn dir_size(path: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut visited = std::collections::HashSet::new();
    dir_size_impl(path, &mut total, &mut visited);
    total
}

fn dir_size_impl(path: &Path, total: &mut u64, visited: &mut std::collections::HashSet<PathBuf>) {
    // Resolve symlinks for cycle detection; skip if already visited.
    let resolved = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => return,
    };
    if !visited.insert(resolved) {
        return; // already visited — symlink cycle
    }

    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                dir_size_impl(&entry_path, total, visited);
            } else if let Ok(meta) = entry_path.metadata() {
                *total = total.saturating_add(meta.len());
            }
        }
    }
}

// ============================================================================
// WASM size report (twiggy-based)
// ============================================================================

/// Print a WASM size report: final vs pre-optimization sizes, per-crate
/// breakdown, and top symbols. Uses twiggy if available.
fn print_wasm_report(build_wasm_dir: &Path, pre_optimization_wasm: &Path) {
    let Ok(pre_optimization_size) = fs::metadata(pre_optimization_wasm).map(|m| m.len()) else {
        return;
    };
    let final_wasm = build_wasm_dir.join("pill_web_app_bg.wasm");
    let final_size = fs::metadata(&final_wasm).ok().map(|m| m.len());

    println!();
    match final_size {
        Some(f) => println!(
            "wasm size: final {} | pre-optimization {}",
            format_bytes(f),
            format_bytes(pre_optimization_size)
        ),
        None => println!(
            "wasm size: pre-optimization {}",
            format_bytes(pre_optimization_size)
        ),
    }

    if let Some(f) = final_size {
        println!();
        println!("--- Final binary analysis ({}) ---", format_bytes(f));
        match run_twiggy_analysis(&final_wasm, f) {
            TwiggyResult::NoTwiggy => {
                println!("(install twiggy for per-crate breakdown: cargo install twiggy)");
            }
            TwiggyResult::Done => {}
            TwiggyResult::Empty | TwiggyResult::Error => {}
        }
    }

    println!();
    println!(
        "--- Pre-optimization analysis ({}) ---",
        format_bytes(pre_optimization_size)
    );
    if let TwiggyResult::NoTwiggy =
        run_twiggy_analysis(pre_optimization_wasm, pre_optimization_size)
    {
        println!("(install twiggy for per-crate breakdown: cargo install twiggy)");
    }
}

enum TwiggyResult {
    Done,
    Empty,
    Error,
    NoTwiggy,
}

fn run_twiggy_analysis(wasm_file_path: &Path, total: u64) -> TwiggyResult {
    let output = match Command::new("twiggy")
        .args(["top", "-n", "15000"])
        .arg(wasm_file_path)
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

    let items = parse_twiggy_output(&stdout);
    if items.is_empty() {
        return TwiggyResult::Empty;
    }

    // Aggregate symbol bytes by crate family.
    let mut by_crate: HashMap<String, u64> = HashMap::new();
    for (bytes, name) in &items {
        *by_crate.entry(classify_crate(name)).or_insert(0) += *bytes;
    }
    let mut groups: Vec<(String, u64)> = by_crate.clone().into_iter().collect();
    groups.sort_by(|a, b| b.1.cmp(&a.1));

    const ENGINE_LIBS: &[&str] = &["pill_engine", "pill_renderer", "pill_core", "pill_web"];
    const GAME_LIBS: &[&str] = &["pill_game"];
    let excluded: Vec<&str> = ENGINE_LIBS
        .iter()
        .chain(GAME_LIBS.iter())
        .copied()
        .collect();

    let engine_total: u64 = ENGINE_LIBS
        .iter()
        .map(|k| by_crate.get(*k).copied().unwrap_or(0))
        .sum();

    // Engine libs breakdown
    println!();
    println!("  Engine libs — BUDGET (% of {}):", format_bytes(total));
    println!("    {:<20} {:>10} {:>7}", "crate", "size", "%");
    for lib in ENGINE_LIBS {
        let bytes = by_crate.get(*lib).copied().unwrap_or(0);
        let percent = 100.0 * bytes as f64 / total as f64;
        println!("    {:<20} {:>10} {:>6.1}%", lib, format_bytes(bytes), percent);
    }
    let engine_percent = 100.0 * engine_total as f64 / total as f64;
    println!(
        "    {:<20} {:>10} {:>6.1}%  ← engine total",
        "---",
        format_bytes(engine_total),
        engine_percent
    );

    // Game code + embedded assets (monitoring only)
    let game_bytes = by_crate.get("pill_game").copied().unwrap_or(0);
    let game_rodata = by_crate.get("[game-rodata]").copied().unwrap_or(0);
    println!();
    println!("  Game (monitor only — excluded from engine budget):");
    println!("    {:<20} {:>10} {:>7}", "crate", "size", "%");
    println!(
        "    {:<20} {:>10} {:>6.1}%  (game logic)",
        "pill_game",
        format_bytes(game_bytes),
        100.0 * game_bytes as f64 / total as f64
    );
    println!(
        "    {:<20} {:>10} {:>6.1}%  (embedded assets via include_bytes!)",
        "[game-assets]",
        format_bytes(game_rodata),
        100.0 * game_rodata as f64 / total as f64
    );

    // Top 15 third-party dependencies
    println!();
    println!("  3rd party (top 15):");
    println!("    {:<20} {:>10} {:>7}", "crate", "size", "%");
    for (crate_name, bytes) in groups
        .iter()
        .filter(|(k, _)| !excluded.contains(&k.as_str()))
        .take(15)
    {
        let percent = 100.0 * *bytes as f64 / total as f64;
        println!(
            "    {:<20} {:>10} {:>6.1}%",
            crate_name,
            format_bytes(*bytes),
            percent
        );
    }

    // Top 10 individual symbols
    println!();
    println!("  Top 10 symbols:");
    for (bytes, name) in items.iter().take(10) {
        let percent = 100.0 * *bytes as f64 / total as f64;
        let display = truncate_display(name, 72);
        println!(
            "  {:>10} {:>5.1}%  {}",
            format_bytes(*bytes),
            percent,
            display
        );
    }

    TwiggyResult::Done
}

fn parse_twiggy_output(stdout: &str) -> Vec<(u64, String)> {
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

fn format_bytes(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let bytes_float = n as f64;
    if bytes_float >= MB {
        format!("{:.2} MB", bytes_float / MB)
    } else if bytes_float >= KB {
        format!("{:.1} KB", bytes_float / KB)
    } else {
        format!("{n} B")
    }
}

/// Coarse bucketing of twiggy item names into crate families.
/// Heuristic — relies on stable twiggy section names and wasm-bindgen symbol prefixes.
fn classify_crate(name: &str) -> String {
    if name.contains(".rodata") || name.contains("data segment") {
        if name.contains("pill_game") {
            return "[game-rodata]".into();
        }
        return "[other-rodata]".into();
    }
    if name.contains("lol_alloc") {
        return "lol_alloc".into();
    }
    if name.starts_with("<") {
        if let Some(idx) = name.find(" as ") {
            let crate_name = &name[1..idx];
            return crate_name.to_string();
        }
        return "[unknown]".into();
    }
    if let Some(idx) = name.find("::") {
        let crate_name = &name[..idx];
        if crate_name.eq_ignore_ascii_case("PillEngine") {
            return "pill_engine".into();
        }
        if crate_name.eq_ignore_ascii_case("PillRenderer") {
            return "pill_renderer".into();
        }
        if crate_name.eq_ignore_ascii_case("PillCore") {
            return "pill_core".into();
        }
        if crate_name.eq_ignore_ascii_case("PillWeb") {
            return "pill_web".into();
        }
        if crate_name.eq_ignore_ascii_case("PillGame") {
            return "pill_game".into();
        }
        return crate_name.to_string();
    }
    "[other]".into()
}
