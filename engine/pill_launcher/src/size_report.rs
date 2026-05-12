//! Post-build wasm size report — final (post-wasm-opt) vs pre-opt sizes,
//! per-crate breakdown, and top symbols. Backed by the `twiggy` CLI; prints
//! a hint if twiggy is not installed.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn print(build_wasm_dir: &Path, preopt_wasm: &Path) {
    let Ok(preopt_size) = fs::metadata(preopt_wasm).map(|m| m.len()) else {
        return;
    };
    let final_wasm = build_wasm_dir.join("pill_web_app_bg.wasm");
    let final_size = fs::metadata(&final_wasm).ok().map(|m| m.len());

    println!();
    match final_size {
        Some(f) => println!(
            "wasm size: final {} | pre-opt {}",
            fmt_bytes(f),
            fmt_bytes(preopt_size)
        ),
        None => println!("wasm size: pre-opt {}", fmt_bytes(preopt_size)),
    }

    // Run twiggy on both binaries when available so we can see what actually
    // survives wasm-opt (the pre-opt binary contains wasm-bindgen custom
    // sections that are stripped during optimization).
    if let Some(f) = final_size {
        println!();
        println!("--- Final binary analysis ({}) ---", fmt_bytes(f));
        match run_twiggy_analysis(&final_wasm, f) {
            TwiggyResult::NoTwiggy => {
                println!("(install twiggy for per-crate breakdown: cargo install twiggy)");
            }
            TwiggyResult::Done => {}
            TwiggyResult::Empty | TwiggyResult::Error => {}
        }
    }

    println!();
    println!("--- Pre-opt analysis ({}) ---", fmt_bytes(preopt_size));
    if let TwiggyResult::NoTwiggy = run_twiggy_analysis(preopt_wasm, preopt_size) {
        println!("(install twiggy for per-crate breakdown: cargo install twiggy)");
    }
}

enum TwiggyResult {
    Done,
    Empty,
    Error,
    NoTwiggy,
}

fn run_twiggy_analysis(wasm_path: &Path, total: u64) -> TwiggyResult {
    // `twiggy top` lists items by retained size. `-n 15000` returns effectively
    // the full symbol table (typical wasm bundles have a few thousand symbols);
    // we aggregate downstream in `classify_crate` / `parse_twiggy`, so we want
    // the whole list, not just the biggest N.
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
    const GAME_LIBS: &[&str] = &["pill_game"];
    let excluded: Vec<&str> = ENGINE_LIBS.iter().chain(GAME_LIBS.iter()).copied().collect();

    let engine_total: u64 = ENGINE_LIBS
        .iter()
        .map(|k| by_crate.get(*k).copied().unwrap_or(0))
        .sum();

    println!();
    println!("  Engine libs — BUDGET (% of {}):", fmt_bytes(total));
    println!("    {:<20} {:>10} {:>7}", "crate", "size", "%");
    for lib in ENGINE_LIBS {
        let bytes = by_crate.get(*lib).copied().unwrap_or(0);
        let pct = 100.0 * bytes as f64 / total as f64;
        println!("    {:<20} {:>10} {:>6.1}%", lib, fmt_bytes(bytes), pct);
    }
    let epct = 100.0 * engine_total as f64 / total as f64;
    println!("    {:<20} {:>10} {:>6.1}%  ← engine total", "---", fmt_bytes(engine_total), epct);

    let game_bytes = by_crate.get("pill_game").copied().unwrap_or(0);
    let game_rodata = by_crate.get("[game-rodata]").copied().unwrap_or(0);
    println!();
    println!("  Game (monitor only — excluded from engine budget):");
    println!("    {:<20} {:>10} {:>7}", "crate", "size", "%");
    println!("    {:<20} {:>10} {:>6.1}%  (game logic)", "pill_game", fmt_bytes(game_bytes), 100.0 * game_bytes as f64 / total as f64);
    println!("    {:<20} {:>10} {:>6.1}%  (embedded assets via include_bytes!)", "[game-assets]", fmt_bytes(game_rodata), 100.0 * game_rodata as f64 / total as f64);

    println!();
    println!("  3rd party (top 15):");
    println!("    {:<20} {:>10} {:>7}", "crate", "size", "%");
    for (crate_name, bytes) in groups
        .iter()
        .filter(|(k, _)| !excluded.contains(&k.as_str()))
        .take(15)
    {
        let pct = 100.0 * *bytes as f64 / total as f64;
        println!("    {:<20} {:>10} {:>6.1}%", crate_name, fmt_bytes(*bytes), pct);
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

// Parse twiggy's default text output. Each data row:
//   "   <bytes> ┊ <pct>% ┊ <item name>"
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

// Coarse bucketing of twiggy item names into crate families. Heuristic —
// relies on stable twiggy section-name output + the wasm-bindgen symbol
// prefix contract. Unknowns fall through to `[other]`; not used for
// correctness, only for the per-family rollup in the printed report.
fn classify_crate(name: &str) -> String {
    if name.contains(".rodata") || name.contains("data segment") {
        if name.contains("pill_game") {
            return "[game-rodata]".into();
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

    // Peel `<` and any `&` / `&mut ` so `<&mut Foo as Bar>::baz` resolves to `Foo`.
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
        "core" | "alloc" | "std" | "compiler_builtins" | "rustc_demangle" | "dlmalloc"
        | "str" | "bool" | "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64"
        | "f32" | "f64" | "char" | "usize" | "isize" | "T" => "[rust-std]".into(),
        "jpeg_decoder" | "png" | "tiff" | "gif" | "weezl" | "miniz_oxide" | "color_quant"
        | "qoi" | "exr" => "image".into(),
        "epaint" | "emath" | "egui_wgpu" | "egui_winit" => "egui".into(),
        "codespan_reporting" | "codespan" | "pp_rs" | "spirv" => "naga".into(),
        "wgpu_hal" | "wgpu_core" | "wgpu_types" => "wgpu".into(),
        "js_sys" => "web_sys".into(),
        _ => ident,
    }
}
