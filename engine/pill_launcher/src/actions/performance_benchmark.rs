// This file implements the "benchmark" action: build+run iterations with stats.
//
// Responsibilities:
// - Builds the game with benchmark features (e.g. benchmark_window).
// - Runs N iterations, capturing stdout to extract JSON frame-time reports.
// - Parses full per-iteration statistics and prints a markdown table + aggregate summary.
// - Depends on: actions::build (build_game_project, run_game_project), utils::paths.

use anyhow::*;
use clap::{App, Arg, ArgMatches};
use path_absolutize::Absolutize;
use std::path::PathBuf;

use crate::actions::build::{build_game_project, run_game_project};
use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::{compile_mode_flag, parse_compile_mode, path_flag};
use crate::utils::paths::get_game_build_path;

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

/// Per-iteration statistics parsed from the game's JSON report line.
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

/// Build the game with benchmark features, run N iterations capturing stdout
/// JSON, parse full stats, and print a markdown table + aggregate summary.
pub(crate) fn do_benchmark(
    game_project_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    benchmark_iterations: u32,
    benchmark_features: &str,
) -> Result<()> {
    println!(
        "Benchmark: {} ({} iterations, features: {})",
        game_project_directory_path.display(),
        benchmark_iterations,
        benchmark_features,
    );

    let mut output_directory_path = PathBuf::from(".");
    output_directory_path = get_game_build_path(
        game_project_directory_path,
        &output_directory_path,
        compile_mode,
    )?;

    println!("Building with features: {} ...", benchmark_features);
    build_game_project(
        game_project_directory_path,
        &output_directory_path,
        compile_mode,
        Some(benchmark_features),
    )?;

    // Run each iteration, capture stdout, and parse the JSON report.
    let mut all_stats: Vec<IterationStats> = Vec::new();
    for i in 1..=benchmark_iterations {
        println!("Iteration {} / {} ...", i, benchmark_iterations);
        let output = run_game_project(
            game_project_directory_path,
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
                    if let Some(stats) = parse_iteration_json(trimmed, i) {
                        all_stats.push(stats);
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
fn parse_iteration_json(json: &str, run: u32) -> Option<IterationStats> {
    let mode = extract_json_string(json, "mode")?;
    let total_frames = extract_json_u64(json, "total_frames")?;
    let measured_frames = extract_json_u64(json, "measured_frames")?;
    let entity_count = extract_json_u64(json, "entity_count")?;
    let average_ms = extract_json_f64(json, "average_ms")?;
    let median_ms = extract_json_f64(json, "median_ms")?;
    let min_ms = extract_json_f64(json, "min_ms")?;
    let max_ms = extract_json_f64(json, "max_ms")?;
    let range_ms = extract_json_f64(json, "range_ms")?;
    let variance = extract_json_f64(json, "variance")?;
    let standard_deviation_ms = extract_json_f64(json, "stddev_ms")?;

    Some(IterationStats {
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
fn extract_json_f64(json: &str, key: &str) -> Option<f64> {
    let search = format!("\"{}\":", key);
    let start = json.find(&search)? + search.len();
    let rest = &json[start..];
    let end = rest
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<f64>().ok()
}

/// Extract a `u64` value from a JSON object for the given key.
fn extract_json_u64(json: &str, key: &str) -> Option<u64> {
    let search = format!("\"{}\":", key);
    let start = json.find(&search)? + search.len();
    let rest = &json[start..];
    let end = rest
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
        .unwrap_or(rest.len());
    rest[..end].trim().parse::<u64>().ok()
}

/// Extract a quoted string value from a JSON object for the given key.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let search = format!("\"{}\":\"", key);
    let start = json.find(&search)? + search.len();
    let rest = &json[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
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

    // Aggregate statistics computed from the per-iteration averages.
    let averages: Vec<f64> = all_stats.iter().map(|s| s.average_ms).collect();
    let aggregate_mean = mean(&averages);
    let aggregate_median = median_of(&averages);
    let aggregate_stddev = standard_deviation(&averages, aggregate_mean);

    // Best / worst iterations (by average_ms)
    let best = all_stats
        .iter()
        .min_by(|a, b| a.average_ms.partial_cmp(&b.average_ms).unwrap())
        .unwrap();
    let worst = all_stats
        .iter()
        .max_by(|a, b| a.average_ms.partial_cmp(&b.average_ms).unwrap())
        .unwrap();

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
