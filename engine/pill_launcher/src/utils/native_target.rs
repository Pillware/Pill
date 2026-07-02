// This file provides shared build/run utilities consumed by the "build" and
// "run" actions.
//
// Responsibilities:
// - CLI flag registration for build-related actions.
// - Cargo stderr parsing for user-friendly error messages.
// - ANSI terminal detection (cached).
// - build_project(): compile project + pill_native + pill_runtime.
// - run_project(): build then launch the standalone executable.

use anyhow::{bail, Context, Error, Result};
use std::{
    fs,
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    time::Instant,
};

use crate::types::*;
use crate::utils::common::{
    ansi_green, dynamic_library_name, format_build_error, format_elapsed_time, parse_cargo_stderr,
    use_experimental_logs_parser, EXECUTABLE_SUFFIX,
};
#[cfg(target_os = "macos")]
use crate::utils::files::codesign_ad_hoc;
use crate::utils::files::{copy_file_if_newer, stage_packaged_resource_files};
use crate::utils::paths::{
    get_path, get_project_title, get_standalone_layout_for_compile_mode,
    get_target_directory_for_compile_mode,
};
use crate::utils::plantuml::render_puml_for_crate;
use crate::utils::workspace::prepare_workspace_for_project;

// ---------------------------------------------------------------------------
// Build & run logic
// ---------------------------------------------------------------------------

/// Build and then launch the native standalone executable for a project.
/// Supports optional stdout capture (for benchmarks) and --features passthrough.
/// Prepares the workspace once and holds the guard through both build and run
/// so that hot-reload child processes can find the project as a workspace member.
pub(crate) fn run_project(
    project_directory_path: &PathBuf,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    project_args: &[String],
    features: Option<&str>,
    capture_stdout: bool,
) -> Result<Option<String>> {
    // Prepare the workspace once and hold the guard through build AND execution.
    // Without this, the guard would drop after build_project() returns, unlinking
    // the project from engine/Cargo.toml before the child process starts.  In
    // hot-reload mode the child checks workspace membership and would fail.
    let (engine_workspace_directory_path, _workspace_guard) =
        prepare_workspace_for_project(project_directory_path, compile_mode)?;

    build_project_in_workspace(
        project_directory_path,
        output_directory_path,
        compile_mode,
        features,
        &engine_workspace_directory_path,
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

    let mut cmd = Command::new(&standalone_executable_path);
    cmd.current_dir(output_directory_path)
        .env("PILL_LAUNCHER_BIN", &launcher_bin)
        .env(
            "PILL_ENGINE_WORKSPACE_DIR",
            &engine_workspace_directory_path,
        )
        .env("PROJECT_DIR", project_directory_path)
        .env("PILL_COMPILE_MODE", compile_mode.to_string())
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
            // Child crashed or was terminated (e.g. user closed the window).
            // Don't bail - the launcher did its job; just report the code.
            let code = output
                .status
                .code()
                .map_or("unknown".into(), |c| c.to_string());
            eprintln!("Project exited with code: {code}");
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
            // Child crashed or was terminated (e.g. user closed the window).
            // Don't bail - the launcher did its job; just report the code once.
            let code = status.code().map_or("unknown".into(), |c| c.to_string());
            eprintln!("Project exited with code: {code}");
        }
        Ok(None)
    }
}

/// Build project + pill_native + pill_runtime via cargo in the engine workspace.
/// Copies the standalone executable and dynamic libraries into the output directory.
/// Supports --features, hot-reload, PlantUML pre-rendering, and per-project target dirs.
pub(crate) fn build_project(
    project_directory_path: &PathBuf,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    features: Option<&str>,
) -> Result<()> {
    let (engine_workspace_directory_path, _guard) =
        prepare_workspace_for_project(project_directory_path, compile_mode)?;
    build_project_in_workspace(
        project_directory_path,
        output_directory_path,
        compile_mode,
        features,
        &engine_workspace_directory_path,
    )
}

/// Core build logic.  The caller is responsible for preparing the workspace
/// (adding the project to engine/Cargo.toml members) before calling this.
fn build_project_in_workspace(
    project_directory_path: &PathBuf,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    features: Option<&str>,
    engine_workspace_directory_path: &PathBuf,
) -> Result<()> {
    println!(
        "Building project from {}...",
        project_directory_path.display()
    );

    let hot_reload_child = *compile_mode == CompileMode::HotReload
        && std::env::var("PILL_HOT_RELOAD_CHILD").ok().as_deref() == Some("1");

    let project_title =
        get_project_title(project_directory_path).context("Failed to get project title")?;

    let cargo_target_dir = if let Ok(shared) = std::env::var("PILL_TARGET_DIR") {
        PathBuf::from(shared)
    } else {
        engine_workspace_directory_path
            .join("target_projects")
            .join(&project_title)
    };

    let pill_engine_dir = get_path(Location::PillEngineCrate);
    if *compile_mode != CompileMode::HotReload {
        if let Err(e) = render_puml_for_crate(&pill_engine_dir) {
            eprintln!("Warning: skipping PlantUML render ({})", e);
        }
    }

    let mut arguments = vec![
        "build",
        "-p",
        "project",
        "-p",
        "pill_native",
        "-p",
        "pill_runtime",
    ];
    if *compile_mode == CompileMode::HotReload {
        arguments.push("--profile");
        arguments.push("hot-reload");
        // Suppress cargo's per-crate progress only during background
        // hot-reload builds triggered by pill_native.  When the user
        // runs `PillLauncher run -c hot-reload` directly, they should
        // see the full compilation output.
        if hot_reload_child {
            arguments.push("--quiet");
        }
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
    let mut cargo_command = Command::new("cargo");
    cargo_command
        .args(&arguments)
        .current_dir(&engine_workspace_directory_path)
        .env("CARGO_TARGET_DIR", &cargo_target_dir);

    // When pill_native captures our output via pipes, cargo detects
    // no TTY and strips ANSI colors from error messages.  Force color
    // in hot-reload child builds so compiler diagnostics remain readable.
    if hot_reload_child {
        cargo_command.env("CARGO_TERM_COLOR", "always");
    }

    let mut cargo_child = if use_experimental_logs_parser() {
        cargo_command
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn cargo build")?
    } else {
        cargo_command
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

    let project_source = compilation_artifacts_folder_path.join(dynamic_library_name("project"));
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
            &data_directory.join(dynamic_library_name("project")),
        )? {
            println!("Copied project dynamic library");
            #[cfg(target_os = "macos")]
            codesign_ad_hoc(&data_directory.join(dynamic_library_name("project")))?;
        }
        if copy_file_if_newer(
            &runtime_source,
            &data_directory.join(dynamic_library_name("pill_runtime")),
        )? {
            println!("Copied runtime dynamic library");
            #[cfg(target_os = "macos")]
            codesign_ad_hoc(&data_directory.join(dynamic_library_name("pill_runtime")))?;
        }
    }

    if *compile_mode == CompileMode::HotReload {
        if copy_file_if_newer(
            &project_source,
            &data_directory.join(dynamic_library_name("project_hot_reloaded")),
        )? {
            println!("Copied project hot-reload dynamic library");
        }
        if copy_file_if_newer(
            &runtime_source,
            &data_directory.join(dynamic_library_name("pill_runtime_hot_reloaded")),
        )? {
            println!("Copied runtime hot-reload dynamic library");
        }
    }

    let time_str = format_elapsed_time(start.elapsed());
    let (open, close) = ansi_green();

    let standalone_path = output_directory_path.join(format!("{project_title}{EXECUTABLE_SUFFIX}"));
    let size_str = match fs::metadata(&standalone_path) {
        Ok(meta) => {
            let bytes = meta.len();
            format!("{:.3} MB", bytes as f64 / (1024.0 * 1024.0))
        }
        Err(_) => "unknown".to_string(),
    };

    println!("{open}Build completed successfully {time_str}{close}");
    println!("Build size: {size_str}");

    Ok(())
}
