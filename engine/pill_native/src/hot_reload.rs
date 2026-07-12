//! This file implements the hot-reload system for both windowed and headless modes.
//!
//! Watches engine crate sources, project sources, resources, and output dylib
//! directories for changes. On detection, invokes PillLauncher to rebuild and
//! hot-swaps the runtime/project DLLs in-place without restarting the game.
//!
//! Supports: windowed mode (polled via winit event loop), headless mode (polled
//! via tight update loop). Full runtime reload on engine changes; project-only
//! reload on game-code changes.

use crate::file_watcher::FileWatcher;
use crate::paths::{dylib, ProjectPaths};
use crate::runtime::{self, RuntimeCreateContext, RuntimeHost};
use anyhow::{bail, Context, Result};
use pill_core::{info, warn, LogContext};
use std::ffi::{CString, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

// Minimum time between successive hot-reload attempts to avoid
// triggering rebuilds on every file-system event.
const RELOAD_COOLDOWN: Duration = Duration::from_millis(1000);

// ---------------------------------------------------------------------------
// FileWatchers
// ---------------------------------------------------------------------------

/// Groups together all file watchers used during hot-reload.
/// Each watcher tracks a different directory:
///   - Engine source crates (pill_core, pill_engine, pill_renderer)
///   - Dynamic libraries output folder
///   - Project source and resources folders
pub(crate) struct FileWatchers {
    pub(crate) engine_core_source_files_watcher: FileWatcher,
    pub(crate) engine_engine_source_files_watcher: FileWatcher,
    pub(crate) engine_renderer_source_files_watcher: FileWatcher,
    pub(crate) dynamic_libraries_files_watcher: FileWatcher,
    pub(crate) project_source_files_watcher: FileWatcher,
    pub(crate) project_resources_files_watcher: FileWatcher,
}

/// Creates a full set of file watchers for all directories relevant
/// to hot-reload: engine crates, output dylibs, and project source/resources.
pub(crate) fn create_file_watchers(project_paths: &ProjectPaths) -> FileWatchers {
    let engine_workspace_directory_path = project_paths
        .engine_source_directory_path
        .as_ref()
        .expect("engine_source_directory_path missing for hot reload");

    let core_source_path = engine_workspace_directory_path.join("pill_core/src");
    let engine_core_source_files_watcher = FileWatcher::new(core_source_path).set_recursive(true);

    let engine_source_path = engine_workspace_directory_path.join("pill_engine/src");
    let engine_engine_source_files_watcher =
        FileWatcher::new(engine_source_path).set_recursive(true);

    let renderer_source_path = engine_workspace_directory_path.join("pill_renderer/src");
    let engine_renderer_source_files_watcher =
        FileWatcher::new(renderer_source_path).set_recursive(true);

    let dynamic_libraries_files_watcher =
        FileWatcher::new(project_paths.build_data_directory_path.clone());
    let project_source_files_watcher =
        FileWatcher::new(project_paths.project_source_directory_path.clone()).set_recursive(true);
    let project_resources_files_watcher =
        FileWatcher::new(project_paths.project_resources_directory_path.clone())
            .set_recursive(true);

    FileWatchers {
        engine_core_source_files_watcher,
        engine_engine_source_files_watcher,
        engine_renderer_source_files_watcher,
        dynamic_libraries_files_watcher,
        project_source_files_watcher,
        project_resources_files_watcher,
    }
}

// ---------------------------------------------------------------------------
// Launcher Command Resolution
// ---------------------------------------------------------------------------

// Resolves the path (or command) to the PillLauncher binary used for
// hot-reload builds.  Checks, in order:
//   1. PILL_LAUNCHER_BIN env var (explicit path)
//   2. PILL_LAUNCHER_CMD env var (shell command)
//   3. Standard target directories under pill_launcher/
// Falls back to "PillLauncherUpstream" on PATH if nothing is found.
fn resolve_launcher_command(engine_source_directory_path: &Path) -> Result<OsString> {
    if let Ok(value) = std::env::var("PILL_LAUNCHER_BIN") {
        let path = PathBuf::from(value);
        if !path.exists() {
            bail!(
                "PILL_LAUNCHER_BIN points to missing file: {}",
                path.display()
            );
        }
        return Ok(path.into_os_string());
    }

    if let Ok(value) = std::env::var("PILL_LAUNCHER_CMD") {
        return Ok(OsString::from(value));
    }

    let launcher_candidates = [
        engine_source_directory_path
            .join("pill_launcher")
            .join("target")
            .join("debug")
            .join("PillLauncher"),
        engine_source_directory_path
            .join("pill_launcher")
            .join("target")
            .join("release")
            .join("PillLauncher"),
        engine_source_directory_path
            .join("pill_launcher")
            .join("target")
            .join("debug")
            .join("PillLauncherUpstream"),
        engine_source_directory_path
            .join("pill_launcher")
            .join("target")
            .join("release")
            .join("PillLauncherUpstream"),
    ];

    for candidate in launcher_candidates {
        if candidate.exists() {
            return Ok(candidate.into_os_string());
        }
    }

    Ok(OsString::from("PillLauncherUpstream"))
}

// ---------------------------------------------------------------------------
// Hot-Reload Build
// ---------------------------------------------------------------------------

/// Invokes PillLauncher to perform a hot-reload build of the project.
/// Sets PILL_HOT_RELOAD_STATUS so external tooling can react to
/// success / warning / failure.
fn build_hot_reload_via_launcher(project_paths: &ProjectPaths) -> Result<()> {
    let engine_source_directory_path = project_paths
        .engine_source_directory_path
        .as_ref()
        .context("engine_source_directory_path missing for hot reload")?;

    let launcher_command = resolve_launcher_command(engine_source_directory_path)?;
    let output_directory = project_paths
        .build_data_directory_path
        .parent()
        .context("build_data_directory_path has no parent")?;

    // When pill_native was compiled in headless mode, the hot-reload
    // rebuild must also enable headless so the new runtime DLL accepts
    // a null window_ptr. The `mut` is only needed with that feature;
    // suppress the clippy lint when building without it.
    #[cfg_attr(not(feature = "headless"), allow(unused_mut))]
    let mut arguments = vec![
        "build",
        "-p",
        project_paths.project_directory_path.to_str().unwrap(),
        "-c",
        "hot-reload",
        "-o",
        output_directory.to_str().unwrap(),
    ];

    #[cfg(feature = "headless")]
    arguments.push("--headless");

    // Attempt to run the pre-built launcher binary.
    // If the binary doesn't exist (NotFound), fall back to `cargo run`
    // which compiles and launches it on-the-fly.
    let output = std::process::Command::new(&launcher_command)
        .args(&arguments)
        .env("PILL_HOT_RELOAD_CHILD", "1")
        .env("PILL_ENGINE_WORKSPACE_DIR", engine_source_directory_path)
        .output();

    let output = match output {
        Ok(output) => output,
        // Launcher binary not found — compile and run via cargo as fallback.
        // This handles the case where the developer hasn't pre-built PillLauncher.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let manifest = engine_source_directory_path
                .join("pill_launcher")
                .join("Cargo.toml");
            std::process::Command::new("cargo")
                .args(["run", "--manifest-path", manifest.to_str().unwrap(), "--"])
                .args(&arguments)
                .env("PILL_HOT_RELOAD_CHILD", "1")
                .env("PILL_ENGINE_WORKSPACE_DIR", engine_source_directory_path)
                .output()
                .context("Failed to invoke pill_launcher via cargo for hot reload")?
        }
        Err(error) => return Err(error).context("Failed to invoke pill_launcher for hot reload"),
    };

    // Forward launcher stdout/stderr to our own output streams
    // so the user sees build progress and compiler diagnostics.
    let standard_output = String::from_utf8_lossy(&output.stdout);
    let standard_error = String::from_utf8_lossy(&output.stderr);

    print!("{standard_output}");
    eprint!("{standard_error}");

    // Set PILL_HOT_RELOAD_STATUS so external tooling can react.
    // Values: "pass" (clean), "warn" (warnings), "fail" (build error).
    if !output.status.success() {
        std::env::set_var("PILL_HOT_RELOAD_STATUS", "fail");
        bail!("pill_launcher build hot-reload failed");
    }

    // Check for compiler warnings in the launcher output and set the
    // status accordingly. Even a successful build may have warnings.
    let has_warnings = standard_output.contains("warning:") || standard_error.contains("warning:");
    if has_warnings {
        std::env::set_var("PILL_HOT_RELOAD_STATUS", "warn");
    } else {
        std::env::set_var("PILL_HOT_RELOAD_STATUS", "pass");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// check_and_reload
// ---------------------------------------------------------------------------

/// Polls all file watchers for changes and triggers a hot-reload build
/// followed by runtime / project reload when needed.
///
/// High-level flow:
///   1. Check that the cooldown period has elapsed.
///   2. Collect changed paths from every file watcher.
///   3. If only resource files changed, skip the build.
///   4. If source files changed, invoke pill_launcher to rebuild.
///   5. If new dylibs appeared, reload the runtime and/or project in-place.
pub(crate) fn check_and_reload(
    runtime_host: &mut Option<RuntimeHost>,
    runtime_context: Option<&RuntimeCreateContext>,
    project_paths: &ProjectPaths,
    last_reload_poll: &mut Instant,
    window_size: winit::dpi::PhysicalSize<u32>,
    file_watchers: &mut FileWatchers,
    runtime_load_mode: crate::paths::RuntimeLoadMode,
) -> Result<()> {
    // --- 1. Cooldown gate ---
    let now = Instant::now();
    if now.duration_since(*last_reload_poll) < RELOAD_COOLDOWN {
        return Ok(());
    }
    *last_reload_poll = now;

    // --- 2. Collect file changes ---
    let mut engine_source_changes = Vec::<PathBuf>::new();
    let mut project_source_changes = Vec::<PathBuf>::new();
    let mut project_resources_changes = Vec::<PathBuf>::new();

    if let Some(paths) = file_watchers.engine_core_source_files_watcher.get_changes() {
        info!(LogContext::HotReload => "Engine pill_core source file change detected: {:?}", paths);
        engine_source_changes.extend(paths);
    }
    if let Some(paths) = file_watchers
        .engine_engine_source_files_watcher
        .get_changes()
    {
        info!(LogContext::HotReload => "Engine pill_engine source file change detected: {:?}", paths);
        engine_source_changes.extend(paths);
    }
    if let Some(paths) = file_watchers
        .engine_renderer_source_files_watcher
        .get_changes()
    {
        info!(LogContext::HotReload => "Engine pill_renderer source file change detected: {:?}", paths);
        engine_source_changes.extend(paths);
    }

    if let Some(paths) = file_watchers.project_resources_files_watcher.get_changes() {
        info!(LogContext::HotReload => "Project resources file change detected: {:?}", paths);
        project_resources_changes.extend(paths);
    }
    if let Some(paths) = file_watchers.project_source_files_watcher.get_changes() {
        info!(LogContext::HotReload => "Project source file change detected: {:?}", paths);
        project_source_changes.extend(paths);
    }

    // --- 3. Resource-only changes: no rebuild needed ---
    if !project_resources_changes.is_empty()
        && project_source_changes.is_empty()
        && engine_source_changes.is_empty()
    {
        info!(LogContext::HotReload => "Project resources changed; No code rebuild needed: {:?}", project_resources_changes);
        return Ok(());
    }

    // --- 4. Build via launcher ---
    let build_start = Instant::now();
    if !project_source_changes.is_empty() || !engine_source_changes.is_empty() {
        if let Err(error) = build_hot_reload_via_launcher(project_paths) {
            warn!(
                LogContext::HotReload =>
                "Hot-reload failed; Keeping currently loaded runtime project. Error: {error:?}"
            );

            // Drain the dylib watcher so stale events don't trigger
            // another build immediately.
            let _ = file_watchers.dynamic_libraries_files_watcher.get_changes();

            return Ok(());
        }
        info!(LogContext::HotReload => "Hot-reload build completed; Took: {:.3}s", build_start.elapsed().as_secs_f64());
    }

    // --- 5. Detect which dylibs were (re)built ---
    let mut runtime_hot_reload = false;
    let mut project_hot_reload = false;
    if let Some(paths) = file_watchers.dynamic_libraries_files_watcher.get_changes() {
        let project_hot_name = dylib("project_hot_reloaded");
        let runtime_hot_name = dylib("pill_runtime_hot_reloaded");

        for path in paths {
            let filename = path.file_name().and_then(|value| value.to_str());
            if filename == Some(&runtime_hot_name) {
                runtime_hot_reload = true;
            } else if filename == Some(&project_hot_name) {
                project_hot_reload = true;
            }
        }
    }

    // In-process runtime is statically linked — it cannot be unloaded
    // and reloaded at runtime.  Skip with a warning rather than failing.
    if runtime_hot_reload && runtime_load_mode == crate::paths::RuntimeLoadMode::InProcess {
        warn!(LogContext::HotReload => "Runtime hot-reload; Skipped for in-process runtime");
        runtime_hot_reload = false;
    }

    // --- 5a. Full runtime reload (engine code changed) ---
    //
    // When any engine crate (pill_core, pill_engine, pill_renderer) is
    // rebuilt, we must tear down the entire runtime and load the new one
    // from the freshly compiled dylib.
    //
    // Steps:
    //   1. Drop the old runtime (destroys engine, renderer, wgpu stack).
    //   2. Copy the new runtime dylib to a unique path so the OS loader
    //      does not lock the original build artifact.
    //   3. If the project dylib was also rebuilt, copy it to a unique
    //      path as well; otherwise reuse the existing project dylib.
    //   4. Load the new runtime dylib via libloading.
    //   5. Create a fresh engine inside the new runtime, passing the
    //      project dylib path and window handle.
    if runtime_hot_reload {
        info!(LogContext::HotReload => "Runtime hot-reload; Reloading runtime...");

        // 1. Destroy the old runtime.  This drops the Engine, Renderer,
        //    wgpu Surface/Device/Queue, and unloads the old project dylib.
        drop(runtime_host.take());

        // 2. Copy the freshly built runtime dylib to a generation-unique
        //    path.  This avoids Windows file-locking the compiler output
        //    and lets us keep the old dylib loaded until we're ready.
        let loaded_runtime_path = runtime::next_loaded_runtime_dylib_path(project_paths);
        fs::copy(
            &project_paths.runtime_dynamic_library_hot_reloaded_path,
            &loaded_runtime_path,
        )
        .context("Failed to copy hot-reloaded runtime dylib to unique loaded path")?;

        // 3. Decide which project dylib to load into the new runtime.
        //    If the project was also rebuilt, use the hot-reloaded copy;
        //    otherwise keep using the original project dylib.
        let project_path_for_create = if project_hot_reload {
            let loaded_project_path = runtime::next_loaded_project_dylib_path(project_paths);
            fs::copy(
                &project_paths.project_dynamic_library_hot_reloaded_path,
                &loaded_project_path,
            )
            .context("Failed to copy hot-reloaded project dylib to unique loaded path")?;
            loaded_project_path
        } else {
            project_paths.project_dynamic_library_path.clone()
        };

        // 4. Open the new runtime dylib and obtain its FFI vtable.
        let mut new_runtime = RuntimeHost::load(&loaded_runtime_path, runtime_load_mode)?;

        // 5. Build the create-args (window pointer, paths, size) and
        //    initialise a fresh engine inside the new runtime.
        let project_dylib_path =
            CString::new(project_path_for_create.to_string_lossy().as_bytes())?;
        let args = if let Some(ctx) = runtime_context {
            ctx.make_args(&project_dylib_path, window_size)
        } else {
            // Headless: no window, build create-args from project_paths.
            let resources_cstr = CString::new(
                project_paths
                    .project_resources_directory_path
                    .to_string_lossy()
                    .as_bytes(),
            )?;
            let config_cstr = CString::new(project_paths.config_path.to_string_lossy().as_bytes())?;
            pill_abi::PillEngineCreateArgsV1 {
                struct_size: std::mem::size_of::<pill_abi::PillEngineCreateArgsV1>() as u32,
                window_ptr: std::ptr::null(),
                project_dylib_path: project_dylib_path.as_ptr(),
                project_resources_dir: resources_cstr.as_ptr(),
                config_path: config_cstr.as_ptr(),
                initial_w: 0,
                initial_h: 0,
            }
        };
        new_runtime.create(&args)?;

        // Swap the old runtime (already dropped) with the new one.
        *runtime_host = Some(new_runtime);

        info!(LogContext::HotReload =>
            "Hot-reload completed (runtime + project); Took: {:.3}s",
            build_start.elapsed().as_secs_f64()
        );
    }
    // --- 5b. Project-only reload (game code changed, engine unchanged) ---
    //
    // When only the project crate is rebuilt, we can hot-swap the project
    // dylib without tearing down the engine.  This is much faster because
    // the renderer, GPU resources, and all engine state survive.
    //
    // Steps:
    //   1. Copy the new project dylib to a unique path.
    //   2. Ask the runtime to call reload_project(), which:
    //        - Drops the old Box<dyn PillProject> and unloads the old dylib.
    //        - Loads the new dylib and constructs a fresh PillProject.
    //        - Calls project.start() on the new project to reinitialise.
    else if project_hot_reload {
        info!(LogContext::HotReload => "Project hot-reload; Reloading project...");

        // 1. Copy the rebuilt project dylib to avoid OS file locks.
        let loaded_project_path = runtime::next_loaded_project_dylib_path(project_paths);
        fs::copy(
            &project_paths.project_dynamic_library_hot_reloaded_path,
            &loaded_project_path,
        )
        .context("Failed to copy hot-reloaded project dylib to unique loaded path")?;

        // 2. In-place project swap — the runtime unloads the old project
        //    dylib, loads the new one, and calls start() on it.
        if let Some(runtime) = runtime_host.as_mut() {
            runtime.reload_project(&loaded_project_path)?;
        } else {
            bail!("Engine not initialized");
        }

        info!(LogContext::HotReload =>
            "Hot-reload completed (project only); Took: {:.3}s",
            build_start.elapsed().as_secs_f64()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

/// Removes files whose names start with the given prefix from a directory.
/// Used during startup to clean up stale hot-reloaded libraries from
/// previous runs. Failures are logged but otherwise ignored.
pub(crate) fn try_remove_files_starting_with(directory_path: &Path, file_name_prefix: &str) {
    if !directory_path.exists() || !directory_path.is_dir() {
        return;
    }

    let entries = match fs::read_dir(directory_path) {
        Ok(entries) => entries,
        Err(error) => {
            warn!(
                LogContext::HotReload => "Failed to read directory {} during cleanup: {}",
                directory_path.display(),
                error
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };

        // Only remove regular files with the expected prefix.
        if !path.is_file() || !name.starts_with(file_name_prefix) {
            continue;
        }

        if let Err(error) = fs::remove_file(&path) {
            warn!(
                LogContext::HotReload => "Ignoring cleanup failure for {}: {}",
                path.display(),
                error
            );
        }
    }
}
