// This file provides shared build/run utilities consumed by the "build" and
// "run" actions as well as benchmarks and CI.
//
// Responsibilities:
// - CLI flag registration for build-related actions.
// - Cargo stderr parsing for user-friendly error messages.
// - ANSI terminal detection (cached).
// - build_project(): compile pill_project + pill_native + pill_runtime.
// - run_project(): build then launch the standalone executable.

use anyhow::{bail, Context, Error, Result};
use clap::Arg;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    time::Instant,
};

use crate::types::*;
use crate::utils::cli::{
    clean_flag, compile_mode_flag, features_flag, output_path_flag, path_flag, target_flag,
};
use crate::utils::common::{
    ansi_green, format_build_error, format_elapsed_time, parse_cargo_stderr,
    use_experimental_logs_parser,
};
use crate::utils::files::*;
use crate::utils::paths::*;
use crate::utils::plantuml::render_puml_for_crate;
use crate::utils::platform::*;
use crate::utils::workspace::prepare_workspace_for_project;

/// Shared CLI flag registration for both "run" and "build" actions.
pub(crate) fn register_build_flags(
    app: clap::App<'static, 'static>,
) -> clap::App<'static, 'static> {
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
        .arg(
            Arg::with_name("wasm-port")
                .long("wasm-port")
                .takes_value(true)
                .default_value("8080")
                .help("Dev server port for WASM targets"),
        )
}

// ---------------------------------------------------------------------------
// Build & run logic
// ---------------------------------------------------------------------------

/// Build and then launch the native standalone executable for a project.
/// Supports optional stdout capture (for benchmarks) and --features passthrough.
/// Sets PILL_PROJECT_DIR, PILL_ENGINE_WORKSPACE_DIR, and other env vars.
pub(crate) fn run_project(
    project_directory_path: &PathBuf,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    project_args: &[String],
    features: Option<&str>,
    capture_stdout: bool,
) -> Result<Option<String>> {
    build_project(
        project_directory_path,
        output_directory_path,
        compile_mode,
        features,
    )?;

    if !capture_stdout {
        println!(
            "Running project from {}...",
            output_directory_path.display()
        );
    }
    let project_title =
        get_project_title(project_directory_path).context("Failed to get project title")?;
    let standalone_executable_path =
        output_directory_path.join(format!("{project_title}{EXECUTABLE_SUFFIX}"));

    let launcher_bin = std::env::current_exe().context("current_exe failed")?;
    let engine_workspace = find_engine_workspace_directory()?;

    let mut cmd = Command::new(&standalone_executable_path);
    cmd.current_dir(output_directory_path)
        .env("PILL_LAUNCHER_BIN", &launcher_bin)
        .env("PILL_ENGINE_WORKSPACE_DIR", &engine_workspace)
        .env("PILL_PROJECT_DIR", project_directory_path)
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
        .args(project_args);

    if capture_stdout {
        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .output()
            .with_context(|| {
                format!(
                    "Failed to launch project executable: {}",
                    standalone_executable_path.display()
                )
            })?;

        if !output.status.success() {
            eprintln!(
                "Project exited with error code: {}",
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
                "Failed to launch project executable: {}",
                standalone_executable_path.display()
            )
        })?;

        if !status.success() {
            eprintln!(
                "Project exited with error code: {}",
                status.code().map_or("unknown".into(), |c| c.to_string())
            );
        }
        Ok(None)
    }
}

/// Build pill_project + pill_native + pill_runtime via cargo in the engine workspace.
/// Copies the standalone executable and dynamic libraries into the output directory.
/// Supports --features, hot-reload, PlantUML pre-rendering, and per-project target dirs.
pub(crate) fn build_project(
    project_directory_path: &PathBuf,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    features: Option<&str>,
) -> Result<()> {
    println!(
        "Building project from {}...",
        project_directory_path.display()
    );

    let hot_reload_child = *compile_mode == CompileMode::HotReload
        && std::env::var("PILL_HOT_RELOAD_CHILD").ok().as_deref() == Some("1");

    let engine_workspace_directory_path =
        prepare_workspace_for_project(project_directory_path, compile_mode)?;

    let project_title =
        get_project_title(project_directory_path).context("Failed to get project title")?;

    let cargo_target_dir = engine_workspace_directory_path
        .join("target_projects")
        .join(&project_title);

    let pill_engine_dir = get_path(Location::PillEngineCrate);
    if *compile_mode != CompileMode::HotReload {
        if let Err(e) = render_puml_for_crate(&pill_engine_dir) {
            eprintln!("Warning: skipping PlantUML render ({})", e);
        }
    }

    let mut arguments = vec![
        "build",
        "-p",
        "pill_project",
        "-p",
        "pill_native",
        "-p",
        "pill_runtime",
    ];
    if *compile_mode == CompileMode::HotReload {
        arguments.push("--profile");
        arguments.push("hot-reload");
        arguments.push("--quiet");
    }
    if *compile_mode == CompileMode::Release {
        arguments.push("--release");
    }
    if let Some(feats) = features {
        for feat in feats.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            arguments.push("--features");
            arguments.push(feat);
        }
    }
    let start = Instant::now();
    let mut cargo_child = if use_experimental_logs_parser() {
        Command::new("cargo")
            .args(&arguments)
            .current_dir(&engine_workspace_directory_path)
            .env("CARGO_TARGET_DIR", &cargo_target_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn cargo build")?
    } else {
        Command::new("cargo")
            .args(&arguments)
            .current_dir(&engine_workspace_directory_path)
            .env("CARGO_TARGET_DIR", &cargo_target_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .context("failed to spawn cargo build")?
    };

    if use_experimental_logs_parser() {
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

                if trimmed.starts_with("error:")
                    || (trimmed.starts_with("thread") && trimmed.contains("panicked at"))
                {
                    in_error = true;
                    continue;
                }

                if in_error {
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

        if !cargo_status.success() {
            let detail = parse_cargo_stderr(&stderr);
            let elapsed = start.elapsed();
            bail!(format_build_error(&detail, elapsed));
        }
    } else {
        let cargo_status = cargo_child
            .wait()
            .context("failed to wait on cargo build")?;

        if !cargo_status.success() {
            bail!(
                "cargo build failed with exit code {:?}",
                cargo_status.code()
            );
        }
    }

    let compilation_artifacts_folder_path =
        cargo_target_dir.join(get_target_directory_for_compile_mode(compile_mode));

    fs::create_dir_all(output_directory_path.join("data").as_path())
        .context("Failed to create build output directories")?;

    if *compile_mode != CompileMode::HotReload || !hot_reload_child {
        let standalone_output_path =
            compilation_artifacts_folder_path.join(format!("pill_native{EXECUTABLE_SUFFIX}"));
        if !standalone_output_path.exists() {
            return Err(Error::msg(
                "Standalone executable was not built successfully",
            ));
        }

        let destination_executable_path =
            output_directory_path.join(format!("{project_title}{EXECUTABLE_SUFFIX}"));

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

    if *compile_mode == CompileMode::Release {
        stage_packaged_resource_files(project_directory_path, &data_directory)?;
    }

    let project_source =
        compilation_artifacts_folder_path.join(dynamic_library_name("pill_project"));
    let runtime_source =
        compilation_artifacts_folder_path.join(dynamic_library_name("pill_runtime"));

    if !project_source.exists() {
        return Err(Error::msg(format!(
            "Project dynamic library missing: {}",
            project_source.display()
        )));
    }
    if !runtime_source.exists() {
        return Err(Error::msg(format!(
            "Runtime dynamic library missing: {}",
            runtime_source.display()
        )));
    }

    if *compile_mode != CompileMode::HotReload || !hot_reload_child {
        if copy_file_if_newer(
            &project_source,
            &data_directory.join(dynamic_library_name("pill_project")),
        )? {
            println!("Copied project dynamic library");
            #[cfg(target_os = "macos")]
            codesign_ad_hoc(&data_directory.join(dynamic_library_name("pill_project")))?;
        } else {
            println!("Skipping copying of project dynamic library");
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

    if *compile_mode == CompileMode::HotReload {
        if copy_file_if_newer(
            &project_source,
            &data_directory.join(dynamic_library_name("pill_project_hot_reloaded")),
        )? {
            println!("Copied project hot-reload dynamic library");
        } else {
            println!("Skipping copying of project hot-reload dynamic library");
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
    let (open, close) = ansi_green();
    println!("{open}Project built successfully {time_str}{close}");

    Ok(())
}
