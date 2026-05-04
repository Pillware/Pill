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
    let final_size = fs::metadata(build_wasm_dir.join("pill_web_app_bg.wasm"))
        .ok()
        .map(|m| m.len());

    println!();
    match final_size {
        Some(f) => println!(
            "wasm size: final {} | pre-opt {}",
            fmt_bytes(f),
            fmt_bytes(preopt_size)
        ),
        None => println!("wasm size: pre-opt {}", fmt_bytes(preopt_size)),
    }

    // `twiggy top` lists items by retained size. `-n 15000` returns effectively
    // the full symbol table (typical wasm bundles have a few thousand symbols);
    // we aggregate downstream in `classify_crate` / `parse_twiggy`, so we want
    // the whole list, not just the biggest N.
    let output = match Command::new("twiggy")
        .args(["top", "-n", "15000"])
        .arg(preopt_wasm)
        .stderr(Stdio::null())
        .output()
    {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("(install twiggy for per-crate breakdown: cargo install twiggy)");
            return;
        }
        Err(_) => return,
    };
    if !output.status.success() {
        return;
    }
    let Ok(stdout) = String::from_utf8(output.stdout) else {
        return;
    };

    let items = parse_twiggy(&stdout);
    if items.is_empty() {
        return;
    }

    // Percentages are against the actual binary size, not the sum of twiggy's
    // shallow bytes (which double-counts — e.g. symbols referencing .rodata).
    let total = preopt_size;

    let mut by_crate: HashMap<String, u64> = HashMap::new();
    for (bytes, name) in &items {
        *by_crate.entry(classify_crate(name)).or_insert(0) += *bytes;
    }
    let mut groups: Vec<(String, u64)> = by_crate.into_iter().collect();
    groups.sort_by(|a, b| b.1.cmp(&a.1));

    println!();
    println!("Crate breakdown (% of pre-opt {}):", fmt_bytes(total));
    println!("  {:<18} {:>10} {:>7}", "crate", "size", "%");
    for (crate_name, bytes) in groups.iter().take(15) {
        let pct = 100.0 * *bytes as f64 / total as f64;
        println!("  {:<18} {:>10} {:>6.1}%", crate_name, fmt_bytes(*bytes), pct);
    }

    println!();
    println!("Top 10 symbols:");
    for (bytes, name) in items.iter().take(10) {
        let pct = 100.0 * *bytes as f64 / total as f64;
        let display = truncate_display(name, 72);
        println!("  {:>10} {:>5.1}%  {}", fmt_bytes(*bytes), pct, display);
    }
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
