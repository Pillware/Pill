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
    ansi_green, dynamic_library_name, extract_json_bool, extract_json_str, format_build_error,
    format_elapsed_time, parse_cargo_stderr, use_experimental_logs_parser, use_verbose_timing,
    EXECUTABLE_SUFFIX,
};
#[cfg(target_os = "macos")]
use crate::utils::files::codesign_ad_hoc;
use crate::utils::files::{artifacts_up_to_date, copy_file_if_newer, stage_packaged_resource_files};
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
    // Mtime pre-check BEFORE prepare_workspace_for_project touches any files.
    // The WorkspaceGuard writes back originals on drop (touching project/Cargo.toml
    // and engine/Cargo.toml), so checking after would always show sources as changed.
    let skip_cargo = compute_skip_cargo(project_directory_path, compile_mode);

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
        skip_cargo,
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
    // Mtime pre-check BEFORE prepare_workspace_for_project touches any files.
    let skip_cargo = compute_skip_cargo(project_directory_path, compile_mode);

    let (engine_workspace_directory_path, _guard) =
        prepare_workspace_for_project(project_directory_path, compile_mode)?;
    build_project_in_workspace(
        project_directory_path,
        output_directory_path,
        compile_mode,
        features,
        &engine_workspace_directory_path,
        skip_cargo,
    )
}

/// Compute whether to skip `cargo build` by checking if all output artifacts
/// are already newer than every source file.
/// Must be called BEFORE `prepare_workspace_for_project` because the workspace
/// guard writes back originals on drop, touching project/Cargo.toml mtime.
///
/// Each artifact is checked only against its own crate's sources:
///   project.dll      ← project/src/
///   pill_runtime.dll ← pill_runtime/src/ + pill_runtime/Cargo.toml
///   pill_native.exe  ← pill_native/src/  + pill_native/Cargo.toml
///
/// project/Cargo.toml and engine/Cargo.toml are excluded because the
/// WorkspaceGuard writes them back on every run drop, bumping their mtime.
fn compute_skip_cargo(project_directory_path: &PathBuf, compile_mode: &CompileMode) -> bool {
    let hot_reload_child = *compile_mode == CompileMode::HotReload
        && std::env::var("PILL_HOT_RELOAD_CHILD").ok().as_deref() == Some("1");

    let project_title = match get_project_title(project_directory_path) {
        Ok(t) => t,
        Err(_) => return false,
    };

    let cargo_target_dir = if let Ok(shared) = std::env::var("PILL_TARGET_DIR") {
        PathBuf::from(shared)
    } else {
        get_path(Location::EngineCrates)
            .join("target_projects")
            .join(&project_title)
    };

    let pill_native_dir = get_path(Location::PillNativeCrate);
    let pill_runtime_dir = get_path(Location::PillRuntimeCrate);
    let artifact_dir = cargo_target_dir.join(get_target_directory_for_compile_mode(compile_mode));

    // project.dll must be newer than project/src/**
    // (project/Cargo.toml excluded — WorkspaceGuard rewrites it on drop)
    let project_src = project_directory_path.join("src");
    let project_dll = artifact_dir.join(dynamic_library_name("project"));
    if !artifacts_up_to_date(&[project_src.as_path()], &[], &[project_dll.as_path()]) {
        return false;
    }

    // pill_runtime.dll must be newer than pill_runtime/src/** + pill_runtime/Cargo.toml
    let runtime_src = pill_runtime_dir.join("src");
    let runtime_cargo = pill_runtime_dir.join("Cargo.toml");
    let runtime_dll = artifact_dir.join(dynamic_library_name("pill_runtime"));
    if !artifacts_up_to_date(
        &[runtime_src.as_path()],
        &[runtime_cargo.as_path()],
        &[runtime_dll.as_path()],
    ) {
        return false;
    }

    // pill_native.exe must be newer than pill_native/src/** + pill_native/Cargo.toml
    // (only required when we are responsible for building it)
    if *compile_mode != CompileMode::HotReload || !hot_reload_child {
        let native_src = pill_native_dir.join("src");
        let native_cargo = pill_native_dir.join("Cargo.toml");
        let native_exe = artifact_dir.join(format!("pill_native{EXECUTABLE_SUFFIX}"));
        if !artifacts_up_to_date(
            &[native_src.as_path()],
            &[native_cargo.as_path()],
            &[native_exe.as_path()],
        ) {
            return false;
        }
    }

    true
}

/// Core build logic.  The caller is responsible for preparing the workspace
/// (adding the project to engine/Cargo.toml members) before calling this.
/// `skip_cargo` must be pre-computed by the caller via `compute_skip_cargo`
/// (before `prepare_workspace_for_project` is called).
fn build_project_in_workspace(
    project_directory_path: &PathBuf,
    output_directory_path: &PathBuf,
    compile_mode: &CompileMode,
    features: Option<&str>,
    engine_workspace_directory_path: &PathBuf,
    skip_cargo: bool,
) -> Result<()> {
    println!(
        "Building project from {}...",
        project_directory_path.display()
    );

    let t_build_total = Instant::now();

    let hot_reload_child = *compile_mode == CompileMode::HotReload
        && std::env::var("PILL_HOT_RELOAD_CHILD").ok().as_deref() == Some("1");

    let t = Instant::now();
    let project_title =
        get_project_title(project_directory_path).context("Failed to get project title")?;
    println!("[TIMING] get_project_title: {:.3}s", t.elapsed().as_secs_f64());

    let cargo_target_dir = if let Ok(shared) = std::env::var("PILL_TARGET_DIR") {
        PathBuf::from(shared)
    } else {
        engine_workspace_directory_path
            .join("target_projects")
            .join(&project_title)
    };

    let pill_engine_dir = get_path(Location::PillEngineCrate);
    if *compile_mode != CompileMode::HotReload {
        let t = Instant::now();
        if let Err(e) = render_puml_for_crate(&pill_engine_dir) {
            eprintln!("Warning: skipping PlantUML render ({})", e);
        }
        println!("[TIMING] render_puml: {:.3}s", t.elapsed().as_secs_f64());
    }

    println!("[TIMING] mtime pre-check result: skip_cargo={}", skip_cargo);

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
    println!("[TIMING] pre-cargo setup: {:.3}s", t_build_total.elapsed().as_secs_f64());
    let start = Instant::now();

    if skip_cargo {
        println!("Sources unchanged, skipping cargo build.");
    } else if use_verbose_timing() {
        // ---------------------------------------------------------------
        // Verbose timing path: pipe stdout as --message-format=json and
        // print a timestamped line for every cargo event (compiler-artifact,
        // build-script-executed, build-finished).  Stderr (the human-readable
        // "Compiling foo..." progress lines) is still inherited so the user
        // sees normal output alongside the timing breakdown.
        // ---------------------------------------------------------------
        let mut json_arguments = arguments.clone();
        // Remove --quiet so we still get JSON events for hot-reload builds.
        json_arguments.retain(|a| *a != "--quiet");
        json_arguments.push("--message-format=json");

        let mut cargo_child = Command::new("cargo")
            .args(&json_arguments)
            .current_dir(&engine_workspace_directory_path)
            .env("CARGO_TARGET_DIR", &cargo_target_dir)
            .stdout(Stdio::piped()) // JSON events
            .stderr(Stdio::inherit()) // human-readable progress
            .spawn()
            .context("failed to spawn cargo build (verbose timing)")?;

        let stdout_pipe = cargo_child
            .stdout
            .take()
            .context("failed to capture cargo stdout")?;

        let mut stderr_for_error = String::new();
        let mut compiled_crates: Vec<(f64, String)> = Vec::new();
        let mut build_scripts: Vec<(f64, String)> = Vec::new();

        {
            let reader = BufReader::new(stdout_pipe);
            for line in reader.lines() {
                let line = line.unwrap_or_default();
                let elapsed = start.elapsed().as_secs_f64();

                let reason = extract_json_str(&line, "reason").unwrap_or("").to_string();

                match reason.as_str() {
                    "compiler-artifact" => {
                        let name = extract_json_str(&line, "name")
                            .unwrap_or("?")
                            .to_string();
                        let fresh = extract_json_bool(&line, "fresh").unwrap_or(false);
                        let state = if fresh { "fresh (skipped)" } else { "compiled" };
                        println!("[TIMING] +{elapsed:.3}s cargo: {state} '{name}'");
                        if !fresh {
                            compiled_crates.push((elapsed, name));
                        }
                    }
                    "build-script-executed" => {
                        // package_id looks like "foo 0.1.0 (path+file:///...)" – grab first token
                        let pkg = extract_json_str(&line, "package_id").unwrap_or("?");
                        let crate_name = pkg.split_whitespace().next().unwrap_or(pkg).to_string();
                        println!("[TIMING] +{elapsed:.3}s cargo: build-script '{crate_name}'");
                        build_scripts.push((elapsed, crate_name));
                    }
                    "build-finished" => {
                        let success = extract_json_bool(&line, "success").unwrap_or(true);
                        println!(
                            "[TIMING] +{elapsed:.3}s cargo: build-finished \
                             (success={success}, compiled={}, build-scripts={})",
                            compiled_crates.len(),
                            build_scripts.len()
                        );
                    }
                    "compiler-message" => {
                        // Forward compiler diagnostics (errors/warnings) to stderr so they
                        // remain visible in verbose timing mode.
                        if let Some(rendered) = extract_json_str(&line, "rendered") {
                            eprint!("{}", rendered.replace("\\n", "\n").replace("\\t", "\t"));
                        }
                        stderr_for_error.push_str(&line);
                        stderr_for_error.push('\n');
                    }
                    _ => {}
                }
            }
        }

        let cargo_status = cargo_child
            .wait()
            .context("failed to wait on cargo build")?;

        if !cargo_status.success() {
            let detail = parse_cargo_stderr(&stderr_for_error);
            let elapsed = start.elapsed();
            bail!(format_build_error(&detail, elapsed));
        }
    } else if use_experimental_logs_parser() {
        let mut cargo_child = Command::new("cargo")
            .args(&arguments)
            .current_dir(&engine_workspace_directory_path)
            .env("CARGO_TARGET_DIR", &cargo_target_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to spawn cargo build")?;

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
        let mut cargo_child = Command::new("cargo")
            .args(&arguments)
            .current_dir(&engine_workspace_directory_path)
            .env("CARGO_TARGET_DIR", &cargo_target_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .context("failed to spawn cargo build")?;

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
    println!("[TIMING] cargo build: {:.3}s", start.elapsed().as_secs_f64());

    let t_post = Instant::now();
    let t = Instant::now();
    fs::create_dir_all(output_directory_path.join("data").as_path())
        .context("Failed to create build output directories")?;
    println!("[TIMING] create_output_dirs: {:.3}s", t.elapsed().as_secs_f64());

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

        let t = Instant::now();
        let _copied = copy_file_if_newer(&standalone_output_path, &destination_executable_path)?;
        println!("[TIMING] copy_executable: {:.3}s", t.elapsed().as_secs_f64());

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
        let t = Instant::now();
        stage_packaged_resource_files(project_directory_path, &data_directory)?;
        println!("[TIMING] stage_resource_files: {:.3}s", t.elapsed().as_secs_f64());
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
        let t = Instant::now();
        if copy_file_if_newer(
            &project_source,
            &data_directory.join(dynamic_library_name("project")),
        )? {
            println!("Copied project dynamic library");
            #[cfg(target_os = "macos")]
            codesign_ad_hoc(&data_directory.join(dynamic_library_name("project")))?;
        } else {
            println!("Skipping copying of project dynamic library");
        }
        println!("[TIMING] copy_project_dylib: {:.3}s", t.elapsed().as_secs_f64());

        let t = Instant::now();
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
        println!("[TIMING] copy_runtime_dylib: {:.3}s", t.elapsed().as_secs_f64());
    }

    if *compile_mode == CompileMode::HotReload {
        let t = Instant::now();
        if copy_file_if_newer(
            &project_source,
            &data_directory.join(dynamic_library_name("project_hot_reloaded")),
        )? {
            println!("Copied project hot-reload dynamic library");
        } else {
            println!("Skipping copying of project hot-reload dynamic library");
        }
        println!("[TIMING] copy_project_hotreload_dylib: {:.3}s", t.elapsed().as_secs_f64());

        let t = Instant::now();
        if copy_file_if_newer(
            &runtime_source,
            &data_directory.join(dynamic_library_name("pill_runtime_hot_reloaded")),
        )? {
            println!("Copied runtime hot-reload dynamic library");
        } else {
            println!("Skipping copying of runtime hot-reload dynamic library");
        }
        println!("[TIMING] copy_runtime_hotreload_dylib: {:.3}s", t.elapsed().as_secs_f64());
    }

    println!("[TIMING] post-cargo steps TOTAL: {:.3}s", t_post.elapsed().as_secs_f64());
    let time_str = format_elapsed_time(start.elapsed());
    let (open, close) = ansi_green();
    println!("{open}Project built successfully {time_str}{close}");

    Ok(())
}
