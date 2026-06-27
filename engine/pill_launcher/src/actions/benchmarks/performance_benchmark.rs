// This file implements the "benchmark" action: build+run iterations with stats.
//
// Responsibilities:
// - Builds the project with benchmark features (e.g. benchmark_window).
// - Runs N iterations, capturing stdout to extract JSON frame-time reports.
// - Parses full per-iteration statistics and prints a markdown table + aggregate summary.
// - Depends on: actions::build (build_project, run_project), utils::paths.

use anyhow::{anyhow, bail, Result};
use clap::{App, Arg, ArgMatches};
use path_absolutize::Absolutize;
use std::path::PathBuf;

use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::{compile_mode_flag, parse_compile_mode, path_flag};
use crate::utils::native_target::{build_project, run_project};
use crate::utils::paths::get_project_build_path;

#[derive(Debug)]
pub(crate) struct Benchmark;

impl Action for Benchmark {
    fn name(&self) -> &'static str {
        "benchmark"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        app.arg(path_flag())
            .arg(compile_mode_flag())
            .arg(
                Arg::with_name("benchmark-iterations")
                    .long("benchmark-iterations")
                    .takes_value(true)
                    .default_value("5")
                    .help("Number of iterations"),
            )
            .arg(
                Arg::with_name("benchmark-features")
                    .long("benchmark-features")
                    .takes_value(true)
                    .default_value("benchmark_window")
                    .help("Cargo features for benchmark"),
            )
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let compile_mode = parse_compile_mode(matches);
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let iterations: u32 = matches
            .value_of("benchmark-iterations")
            .unwrap_or("5")
            .parse()
            .unwrap_or(5);
        let features = matches
            .value_of("benchmark-features")
            .unwrap_or("benchmark_window");
        do_benchmark(&path, &compile_mode, iterations, features)
    }
}

/// Per-iteration statistics parsed from the project's JSON report line.
#[derive(Debug, Clone)]
struct IterationStats {
    /// Which run this was (1-based).
    run: u32,
    /// Benchmark mode ("windowed" or "headless").
    mode: String,
    /// Total frames simulated (including warmup).
    total_frames: u64,
    /// Frames actually measured (after warmup).
    measured_frames: u64,
    /// Number of entities in the scene.
    entity_count: u64,
    /// Average frame time in milliseconds.
    average_ms: f64,
    /// Median frame time in milliseconds.
    median_ms: f64,
    /// Fastest frame time in milliseconds.
    min_ms: f64,
    /// Slowest frame time in milliseconds.
    max_ms: f64,
    /// Difference between max and min (milliseconds).
    range_ms: f64,
    /// Variance of frame times.
    variance: f64,
    /// Standard deviation of frame times in milliseconds.
    standard_deviation_ms: f64,
}

/// Build the project with benchmark features, run N iterations capturing stdout
/// JSON, parse full stats, and print a markdown table + aggregate summary.
pub(crate) fn do_benchmark(
    project_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    benchmark_iterations: u32,
    benchmark_features: &str,
) -> Result<()> {
    println!(
        "Benchmark: {} ({} iterations, features: {})",
        project_directory_path.display(),
        benchmark_iterations,
        benchmark_features,
    );

    let mut output_directory_path = PathBuf::from(".");
    output_directory_path =
        get_project_build_path(project_directory_path, &output_directory_path, compile_mode)?;

    println!("Building with features: {} ...", benchmark_features);
    build_project(
        project_directory_path,
        &output_directory_path,
        compile_mode,
        Some(benchmark_features),
    )?;

    // Run each iteration, capture stdout, and parse the JSON report.
    let mut all_stats: Vec<IterationStats> = Vec::new();
    for i in 1..=benchmark_iterations {
        println!("Iteration {} / {} ...", i, benchmark_iterations);
        let output = run_project(
            project_directory_path,
            &output_directory_path,
            compile_mode,
            &[],
            Some(benchmark_features),
            true,
        )?;

        if let Some(stdout) = output {
            // Find the first line starting with '{' — that's the JSON report.
            for line in stdout.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with('{') {
                    match parse_iteration_json(trimmed, i) {
                        Ok(stats) => all_stats.push(stats),
                        Err(e) => eprintln!("  Warning: iteration {} — {:#}", i, e),
                    }
                    break;
                }
            }
        }

        if all_stats.len() < i as usize {
            eprintln!("  Warning: no JSON output captured for iteration {}", i);
        }
    }

    if all_stats.is_empty() {
        bail!("No benchmark iterations produced valid JSON output");
    }

    print_markdown_report(&all_stats);

    Ok(())
}

// -- JSON parsing ------------------------------------------------------------

/// Parse a single iteration's JSON report line into an [`IterationStats`].
/// Returns an error explaining which field failed to parse, rather than silently
/// returning `None` and skipping the iteration.
fn parse_iteration_json(json: &str, run: u32) -> Result<IterationStats> {
    let mode = extract_json_string(json, "mode")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'mode' field"))?;
    let total_frames = extract_json_u64(json, "total_frames")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'total_frames' field"))?;
    let measured_frames = extract_json_u64(json, "measured_frames")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'measured_frames' field"))?;
    let entity_count = extract_json_u64(json, "entity_count")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'entity_count' field"))?;
    let average_ms = extract_json_f64(json, "average_ms")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'average_ms' field"))?;
    let median_ms = extract_json_f64(json, "median_ms")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'median_ms' field"))?;
    let min_ms = extract_json_f64(json, "min_ms")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'min_ms' field"))?;
    let max_ms = extract_json_f64(json, "max_ms")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'max_ms' field"))?;
    let range_ms = extract_json_f64(json, "range_ms")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'range_ms' field"))?;
    let variance = extract_json_f64(json, "variance")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'variance' field"))?;
    let standard_deviation_ms = extract_json_f64(json, "stddev_ms")
        .ok_or_else(|| anyhow!("iteration {run}: missing or invalid 'stddev_ms' field"))?;

    // Reject iterations with NaN in any numeric field — NaN indicates a bug
    // in the project's benchmark output, not a valid measurement.
    for (name, val) in [
        ("average_ms", average_ms),
        ("median_ms", median_ms),
        ("min_ms", min_ms),
        ("max_ms", max_ms),
        ("range_ms", range_ms),
        ("variance", variance),
        ("stddev_ms", standard_deviation_ms),
    ] {
        if val.is_nan() {
            bail!("iteration {run}: '{name}' is NaN — benchmark output is corrupted");
        }
    }

    Ok(IterationStats {
        run,
        mode,
        total_frames,
        measured_frames,
        entity_count,
        average_ms,
        median_ms,
        min_ms,
        max_ms,
        range_ms,
        variance,
        standard_deviation_ms,
    })
}

/// Extract an `f64` value from a JSON object for the given key.
/// Handles optional whitespace around the colon and value.
fn extract_json_f64(json: &str, key: &str) -> Option<f64> {
    let value_str = extract_json_value(json, key)?;
    value_str.trim().parse::<f64>().ok()
}

/// Extract a `u64` value from a JSON object for the given key.
fn extract_json_u64(json: &str, key: &str) -> Option<u64> {
    let value_str = extract_json_value(json, key)?;
    value_str.trim().parse::<u64>().ok()
}

/// Extract the raw value substring for the given key from a flat JSON object.
/// Handles string values with `\"` escapes, numbers, and skips interior whitespace.
fn extract_json_value<'j>(json: &'j str, key: &str) -> Option<&'j str> {
    let search = format!("\"{}\"", key);
    let after_key = json.find(&search)? + search.len();

    // Skip whitespace and the colon separator.
    let after_colon = json[after_key..].find(':').map(|i| after_key + i + 1)?;

    let rest = json[after_colon..].trim_start();
    if rest.is_empty() {
        return None;
    }

    let first_char = rest.chars().next()?;
    if first_char == '"' {
        // String value: scan for the closing unescaped quote.
        let inner = &rest[1..];
        let mut chars = inner.char_indices();
        loop {
            match chars.next() {
                Some((_, '\\')) => {
                    // Skip the escaped character (handles \\, \", \n, etc.)
                    chars.next();
                }
                Some((i, '"')) => return Some(&inner[..i]),
                None => return None, // unterminated string
                _ => {}
            }
        }
    } else {
        // Number or literal (true/false/null): scan until delimiter.
        let end = rest
            .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
            .unwrap_or(rest.len());
        Some(&rest[..end])
    }
}

/// Extract a quoted string value from a JSON object for the given key.
/// Returns the raw string content with escape sequences preserved.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let raw = extract_json_value(json, key)?;
    // Strip surrounding quotes if present.
    let inner = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"'))?;
    // Unescape common JSON escape sequences (\\, \", \n, \r, \t).
    let unescaped = unescape_json_string(inner);
    Some(unescaped)
}

/// Minimal JSON string unescaping: handles \\, \", \n, \r, \t.
fn unescape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

// -- Report formatting -------------------------------------------------------

/// Print a markdown table of per-iteration stats plus an aggregate summary.
fn print_markdown_report(all_stats: &[IterationStats]) {
    let count = all_stats.len();
    let mode = &all_stats[0].mode;
    let entities = all_stats[0].entity_count;
    let total_frames = all_stats[0].total_frames;
    let measured_frames = all_stats[0].measured_frames;

    println!();
    println!("=== Benchmark Report ===");
    println!();
    println!("| {:<18} | {:<16} |", "Parameter", "Value");
    println!("|{:-^20}|{:-^18}|", "", "");
    println!("| {:<18} | {:<16} |", "Mode", mode);
    println!("| {:<18} | {:<16} |", "Entities", entities);
    println!("| {:<18} | {:<16} |", "Total frames/run", total_frames);
    println!("| {:<18} | {:<16} |", "Measured frames", measured_frames);
    println!("| {:<18} | {:<16} |", "Iterations", count);
    println!();

    // Per-iteration table
    println!(
        "| {:<3} | {:<8} | {:<11} | {:<8} | {:<8} | {:<10} | {:<11} | {:<8} |",
        "Run",
        "Avg (ms)",
        "Median (ms)",
        "Min (ms)",
        "Max (ms)",
        "Range (ms)",
        "StdDev (ms)",
        "Variance"
    );
    println!(
        "|{:-^5}|{:-^10}|{:-^13}|{:-^10}|{:-^10}|{:-^12}|{:-^13}|{:-^10}|",
        "", "", "", "", "", "", "", ""
    );
    for stats in all_stats {
        println!(
            "| {:<3} | {:>8.3} | {:>11.3} | {:>8.3} | {:>8.3} | {:>10.3} | {:>11.3} | {:>8.4} |",
            stats.run,
            stats.average_ms,
            stats.median_ms,
            stats.min_ms,
            stats.max_ms,
            stats.range_ms,
            stats.standard_deviation_ms,
            stats.variance,
        );
    }
    println!();

    // Filter NaN values before computing aggregates.
    let averages: Vec<f64> = all_stats
        .iter()
        .map(|s| s.average_ms)
        .filter(|v| v.is_finite())
        .collect();
    if averages.is_empty() {
        println!("Warning: all frame-time averages were NaN; skipping aggregate stats.");
        return;
    }
    let aggregate_mean = mean(&averages);
    let aggregate_median = median_of(&averages);
    let aggregate_stddev = standard_deviation(&averages, aggregate_mean);

    // Best / worst iterations (by average_ms), using total_cmp for NaN-safe ordering.
    let best = all_stats
        .iter()
        .min_by(|a, b| a.average_ms.total_cmp(&b.average_ms))
        .unwrap_or(&all_stats[0]);
    let worst = all_stats
        .iter()
        .max_by(|a, b| a.average_ms.total_cmp(&b.average_ms))
        .unwrap_or(&all_stats[0]);

    println!("### Aggregate (cross-iteration)");
    println!();
    println!("| {:<21} | {:<16} |", "Metric", "Value");
    println!("|{:-^23}|{:-^18}|", "", "");
    println!("| {:<21} | {:<16.3} ms |", "Mean avg_ms", aggregate_mean);
    println!(
        "| {:<21} | {:<16.3} ms |",
        "Median avg_ms", aggregate_median
    );
    println!(
        "| {:<21} | {:<16.3} ms |",
        "StdDev avg_ms", aggregate_stddev
    );
    println!(
        "| {:<21} | #{run} — {val:<16.3} ms |",
        "Best run (avg_ms)",
        run = best.run,
        val = best.average_ms
    );
    println!(
        "| {:<21} | #{run} — {val:<16.3} ms |",
        "Worst run (avg_ms)",
        run = worst.run,
        val = worst.average_ms
    );
    println!(
        "| {:<21} | {:<16.3} ms |",
        "Spread (worst−best)",
        worst.average_ms - best.average_ms
    );
    println!();
}

// -- Statistics helpers ------------------------------------------------------

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

fn median_of(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

fn standard_deviation(values: &[f64], mean: f64) -> f64 {
    if values.len() < 2 {
        return 0.0;
    }
    let variance: f64 =
        values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (values.len() - 1) as f64;
    variance.sqrt()
}
