// This file implements the "build" and "run" actions for native game projects.
//
// Responsibilities:
// - build_game_project(): invokes cargo build for pill_game + pill_native + pill_runtime
//   in the engine workspace, copies artifacts into the build output directory.
// - run_game_project(): builds then launches the standalone executable with the
//   appropriate environment variables (PILL_GAME_PROJECT_DIR, etc.).
// - Supports --features passthrough, hot-reload mode, and stdout capture (for benchmarks).
// - Depends on: workspace, utils::paths, utils::files, utils::platform, utils::assets.

use anyhow::*;
use clap::{App, Arg, ArgMatches};
use path_absolutize::Absolutize;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    time::Instant,
};

use crate::actions::Action;
use crate::types::*;
use crate::utils::cli::{
    clean_flag, compile_mode_flag, features_flag, output_path_flag, parse_build_target,
    parse_compile_mode, path_flag, target_flag,
};
use crate::utils::files::*;
use crate::utils::paths::*;
use crate::utils::plantuml::render_puml_for_crate;
use crate::utils::platform::*;
use crate::utils::wasm;
use crate::utils::web_dev_server;
use crate::utils::workspace::prepare_workspace_for_game;

/// Shared CLI flag registration for both "run" and "build" actions.
fn register_build_flags(app: App<'static, 'static>) -> App<'static, 'static> {
    app.arg(path_flag())
        .arg(output_path_flag())
        .arg(compile_mode_flag())
        .arg(target_flag())
        .arg(clean_flag())
        .arg(features_flag())
        .arg(
            Arg::with_name("max-wasm-size")
                .long("max-wasm-size")
                .takes_value(true)
                .help("Fail WASM build if binary exceeds N KB"),
        )
}

/// Extract a concise, actionable error message from raw cargo stderr.
/// Handles panics with "Caused by:" chains and tool-not-found hints.
fn parse_cargo_stderr(stderr: &str) -> String {
    let mut detail = String::new();
    let lines: Vec<&str> = stderr.lines().collect();

    let mut seen_panic = false;
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();

        // The panic message body is on the line immediately after "panicked at".
        if trimmed.starts_with("thread") && trimmed.contains("panicked at") {
            seen_panic = true;
            if i + 1 < lines.len() {
                let next = lines[i + 1].trim();
                if !next.is_empty() {
                    detail.push_str(next);
                }
            }
            i += 2;
            continue;
        }

        // Collect "Caused by:" chain entries that follow a panic.
        if seen_panic && trimmed.starts_with("Caused by:") {
            if i + 1 < lines.len() {
                let next = lines[i + 1].trim();
                if !next.is_empty() && next != "Caused by:" {
                    if !detail.is_empty() {
                        detail.push('\n');
                    }
                    detail.push_str(next);
                }
            }
            i += 2;
            continue;
        }

        if seen_panic && (trimmed.contains("not found on PATH") || trimmed.contains("Install ")) {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(trimmed);
        }

        i += 1;
    }

    if detail.is_empty() {
        detail = stderr.trim().to_string();
    }

    detail
        .lines()
        .map(|l| format!("\t{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format a duration as "after Xmin Ysec" or "after Ysec".
fn format_elapsed_time(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    let minutes = seconds / 60;
    let remainder = seconds % 60;
    if minutes > 0 {
        format!("after {}min {}sec", minutes, remainder)
    } else {
        format!("after {}sec", remainder)
    }
}

/// Format a build-failure message with ANSI red coloring and elapsed time.
fn format_build_error(detail: &str, elapsed: std::time::Duration) -> String {
    let time_str = format_elapsed_time(elapsed);
    format!(
        "\x1b[31mPill Standalone \"run\" command failed {time_str}\x1b[0m\n\nCaused by:\n{detail}"
    )
}

pub(crate) struct Run;

impl Action for Run {
    fn name(&self) -> &'static str {
        "run"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        register_build_flags(app)
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let compile_mode = parse_compile_mode(matches);
        let target = parse_build_target(matches);
        let features = matches.value_of("features");
        let passthrough: Vec<String> = matches
            .values_of("game-args")
            .map(|v| v.map(String::from).collect())
            .unwrap_or_default();
        let clean = matches.is_present("clean");

        if clean {
            crate::utils::assets::run_asset_pipeline(&path.join("res"), true)?;
        }

        match target {
            BuildTarget::Native => {
                let output_directory =
                    PathBuf::from(matches.value_of("output-path").unwrap_or("."));
                let output_directory =
                    get_game_build_path(&path, &output_directory, &compile_mode)?;
                run_game_project(
                    &path,
                    &output_directory,
                    &compile_mode,
                    &passthrough,
                    features,
                    false,
                )?;
            }
            BuildTarget::Web => {
                web_dev_server::run(&path, &compile_mode)?;
            }
        }
        Ok(())
    }
}

pub(crate) struct Build;

impl Action for Build {
    fn name(&self) -> &'static str {
        "build"
    }

    fn register(&self, app: App<'static, 'static>) -> App<'static, 'static> {
        register_build_flags(app)
    }

    fn run(&self, matches: &ArgMatches) -> Result<()> {
        let path = PathBuf::from(matches.value_of("path").unwrap_or("."))
            .absolutize()?
            .to_path_buf();
        let compile_mode = parse_compile_mode(matches);
        let target = parse_build_target(matches);
        let features = matches.value_of("features");
        let clean = matches.is_present("clean");
        let maximum_wasm_size: Option<u64> = matches
            .value_of("max-wasm-size")
            .and_then(|s| s.parse().ok());

        if clean {
            crate::utils::assets::run_asset_pipeline(&path.join("res"), true)?;
        }

        match target {
            BuildTarget::Native => {
                let output_directory =
                    PathBuf::from(matches.value_of("output-path").unwrap_or("."));
                let output_directory =
                    get_game_build_path(&path, &output_directory, &compile_mode)?;
                build_game_project(&path, &output_directory, &compile_mode, features)?;
            }
            BuildTarget::Web => {
                if matches.occurrences_of("output-path") > 0 {
                    println!("Note: `-o/--output-path` is ignored with `-t wasm`; output is fixed at <game>/build/wasm/");
                }
                wasm::build(&path, &compile_mode, maximum_wasm_size)?;
            }
        }
        Ok(())
    }
}

/// Build and then launch the native standalone executable for a game project.
/// Supports optional stdout capture (for benchmarks) and --features passthrough.
/// Sets PILL_GAME_PROJECT_DIR, PILL_ENGINE_WORKSPACE_DIR, and other env vars.
pub(crate) fn run_game_project(
    game_project_directory_path: &PathBuf,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    game_args: &[String],
    features: Option<&str>,
    capture_stdout: bool,
) -> Result<Option<String>> {
    // Build game project
    build_game_project(
        game_project_directory_path,
        output_directory_path,
        compile_mode,
        features,
    )?;

    // Run game project
    if !capture_stdout {
        println!(
            "Running game project from {}...",
            output_directory_path.display()
        );
    }
    let game_title =
        get_game_title(game_project_directory_path).context("Failed to get game title")?;
    let standalone_executable_path =
        output_directory_path.join(format!("{game_title}{EXECUTABLE_SUFFIX}"));

    let launcher_bin = std::env::current_exe().context("current_exe failed")?;
    let engine_workspace = find_engine_workspace_directory()?; // .../Pill-Engine/engine

    let mut cmd = Command::new(&standalone_executable_path);
    cmd.current_dir(output_directory_path)
        .env("PILL_LAUNCHER_BIN", &launcher_bin)
        .env("PILL_ENGINE_WORKSPACE_DIR", &engine_workspace)
        .env("PILL_GAME_PROJECT_DIR", game_project_directory_path)
        .env(
            "PILL_STANDALONE_LAYOUT",
            get_standalone_layout_for_compile_mode(compile_mode),
        )
        .env(
            "PILL_ENABLE_HOT_RELOAD",
            if *compile_mode == CompileMode::HotReload {
                "1"
            } else {
                "0"
            },
        )
        .args(game_args);

    // Capture mode: pipe stdout into a String, inherit stderr to terminal.
    if capture_stdout {
        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output()
            .with_context(|| {
                format!(
                    "Failed to launch game project executable: {}",
                    standalone_executable_path.display()
                )
            })?;

        if !output.status.success() {
            eprintln!(
                "Game exited with error code: {}",
                output
                    .status
                    .code()
                    .map_or("unknown".into(), |c| c.to_string())
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(Some(stdout))
    } else {
        let status = cmd.status().with_context(|| {
            format!(
                "Failed to launch game project executable: {}",
                standalone_executable_path.display()
            )
        })?;

        if !status.success() {
            eprintln!(
                "Game exited with error code: {}",
                status.code().map_or("unknown".into(), |c| c.to_string())
            );
        }
        Ok(None)
    }
}

/// Build pill_game + pill_native + pill_runtime via cargo in the engine workspace.
/// Copies the standalone executable and dynamic libraries into the output directory.
/// Supports --features, hot-reload, PlantUML pre-rendering, and per-game target dirs.
pub(crate) fn build_game_project(
    game_project_directory_path: &PathBuf,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    features: Option<&str>,
) -> Result<()> {
    println!(
        "Building game project from {}...",
        game_project_directory_path.display()
    );

    let hot_reload_child = *compile_mode == CompileMode::HotReload
        && std::env::var("PILL_HOT_RELOAD_CHILD").ok().as_deref() == Some("1");

    let engine_workspace_directory_path =
        prepare_workspace_for_game(game_project_directory_path, compile_mode)?;

    // Get game title EARLY (we need it for per-game target dir)
    let game_title =
        get_game_title(game_project_directory_path).context("Failed to get game title")?;

    // Use a per-game target dir so switching games doesn't invalidate everything
    let cargo_target_dir = engine_workspace_directory_path
        .join("target_games")
        .join(&game_title);

    // Pre-render PUML only for non-hot-reload builds
    let pill_engine_dir = get_path(Location::PillEngineCrate);
    if *compile_mode != CompileMode::HotReload {
        if let Err(e) = render_puml_for_crate(&pill_engine_dir) {
            eprintln!("Warning: skipping PlantUML render ({})", e);
        }
    }

    // Build all three workspace crates together so type IDs are consistent.
    let mut arguments = vec![
        "build",
        "-p",
        "pill_game",
        "-p",
        "pill_native",
        "-p",
        "pill_runtime",
    ];
    // Hot-reload uses a custom Cargo profile with fast incremental compilation.
    if *compile_mode == CompileMode::HotReload {
        arguments.push("--profile");
        arguments.push("hot-reload");
        arguments.push("--quiet");
    }
    if *compile_mode == CompileMode::Release {
        arguments.push("--release");
    }
    // Append --features flags, splitting comma-separated values into individual args.
    if let Some(feats) = features {
        for feat in feats.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            arguments.push("--features");
            arguments.push(feat);
        }
    }
    let start = Instant::now();
    let mut cargo_child = Command::new("cargo")
        .args(&arguments)
        .current_dir(&engine_workspace_directory_path)
        .env("CARGO_TARGET_DIR", &cargo_target_dir)
        .stdout(Stdio::inherit()) // real-time to terminal
        .stderr(Stdio::piped()) // we'll read line-by-line
        .spawn()
        .context("failed to spawn cargo build")?;

    // Stream stderr in real time. When cargo hits an error, we suppress the
    // noisy error-chain output and present it cleanly in the final error message.
    let stderr_pipe = cargo_child
        .stderr
        .take()
        .context("failed to capture cargo stderr")?;
    let mut stderr_lines = String::new();
    {
        let reader = BufReader::new(stderr_pipe);
        let mut in_error = false;
        for line in reader.lines() {
            let line = line.unwrap_or_default();
            stderr_lines.push_str(&line);
            stderr_lines.push('\n');

            let trimmed = line.trim();

            // When we see an error header, start suppressing subsequent lines.
            // The full error will be extracted and shown cleanly at the end.
            if trimmed.starts_with("error:")
                || (trimmed.starts_with("thread") && trimmed.contains("panicked at"))
            {
                in_error = true;
                continue;
            }

            if in_error {
                // Stay in error mode until a clear "normal output" marker.
                if trimmed.starts_with("warning:")
                    || trimmed.starts_with("Compiling")
                    || trimmed.starts_with("Checking")
                    || trimmed.starts_with("Finished")
                {
                    in_error = false;
                    eprintln!("{line}");
                }
                continue;
            }

            eprintln!("{line}");
        }
    }

    let cargo_status = cargo_child
        .wait()
        .context("failed to wait on cargo build")?;
    let stderr = stderr_lines;

    // Build failed — extract only the actionable error message from the raw stderr.
    if !cargo_status.success() {
        let detail = parse_cargo_stderr(&stderr);
        let elapsed = start.elapsed();
        bail!(format_build_error(&detail, elapsed));
    }

    // Cargo placed the compiled binaries in CARGO_TARGET_DIR/<profile>/.
    let compilation_artifacts_folder_path =
        cargo_target_dir.join(get_target_directory_for_compile_mode(compile_mode));

    // Ensure build/data exists
    fs::create_dir_all(output_directory_path.join("data").as_path())
        .context("Failed to create build output directories")?;

    // Copy the standalone executable into the build output directory.
    // Skip for initial hot-reload builds (only copy on subsequent reloads).
    if *compile_mode != CompileMode::HotReload || !hot_reload_child {
        let standalone_output_path =
            compilation_artifacts_folder_path.join(format!("pill_native{EXECUTABLE_SUFFIX}"));
        if !standalone_output_path.exists() {
            return Err(Error::msg(
                "Standalone executable was not built successfully",
            ));
        }

        let destination_executable_path =
            output_directory_path.join(format!("{game_title}{EXECUTABLE_SUFFIX}"));

        let _copied = copy_file_if_newer(&standalone_output_path, &destination_executable_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&destination_executable_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&destination_executable_path, perms)?;
        }
    }

    let data_directory = output_directory_path.join("data");
    fs::create_dir_all(&data_directory)?;

    // Release builds bundle game resources into data/res/ for standalone distribution.
    if *compile_mode == CompileMode::Release {
        stage_packaged_resource_files(game_project_directory_path, &data_directory)?;
    }

    // Copy the game and runtime dynamic libraries into the build output.
    let game_source = compilation_artifacts_folder_path.join(dynamic_library_name("pill_game"));
    let runtime_source =
        compilation_artifacts_folder_path.join(dynamic_library_name("pill_runtime"));

    if !game_source.exists() {
        return Err(Error::msg(format!(
            "Game dynamic library missing: {}",
            game_source.display()
        )));
    }
    if !runtime_source.exists() {
        return Err(Error::msg(format!(
            "Runtime dynamic library missing: {}",
            runtime_source.display()
        )));
    }

    // Only copy dynamic libraries for initial builds — overwriting loaded libs
    // in a running hot-reload process would crash it.
    if *compile_mode != CompileMode::HotReload || !hot_reload_child {
        if copy_file_if_newer(
            &game_source,
            &data_directory.join(dynamic_library_name("pill_game")),
        )? {
            println!("Copied game dynamic library");
            #[cfg(target_os = "macos")]
            codesign_ad_hoc(&data_directory.join(dynamic_library_name("pill_game")))?;
        } else {
            println!("Skipping copying of game dynamic library");
        }
        if copy_file_if_newer(
            &runtime_source,
            &data_directory.join(dynamic_library_name("pill_runtime")),
        )? {
            println!("Copied runtime dynamic library");
            #[cfg(target_os = "macos")]
            codesign_ad_hoc(&data_directory.join(dynamic_library_name("pill_runtime")))?;
        } else {
            println!("Skipping copying of runtime dynamic library");
        }
    }

    // In hot-reload mode, also copy to the hot-reload names (file watcher looks for these).
    if *compile_mode == CompileMode::HotReload {
        if copy_file_if_newer(
            &game_source,
            &data_directory.join(dynamic_library_name("pill_game_hot_reloaded")),
        )? {
            println!("Copied game hot-reload dynamic library");
        } else {
            println!("Skipping copying of game hot-reload dynamic library");
        }
        if copy_file_if_newer(
            &runtime_source,
            &data_directory.join(dynamic_library_name("pill_runtime_hot_reloaded")),
        )? {
            println!("Copied runtime hot-reload dynamic library");
        } else {
            println!("Skipping copying of runtime hot-reload dynamic library");
        }
    }

    let time_str = format_elapsed_time(start.elapsed());
    println!("\x1b[32mGame built successfully {time_str}\x1b[0m");

    Ok(())
}
