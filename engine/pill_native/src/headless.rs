//! This file implements the headless runner, activated by the `headless` feature.
//!
//! Replaces the winit-based run_app() with a bare update loop — no window,
//! no event loop, no GPU surface. Loads the runtime DLL directly and drives
//! the engine frame-by-frame until the project signals exit via is_exit_requested().
//!
//! Hot-reload is fully supported: file watchers are polled each frame and
//! engine/project DLLs are hot-swapped using the same check_and_reload path.

#[cfg(feature = "headless")]
use {
    crate::hot_reload,
    crate::hot_reload::{check_and_reload, create_file_watchers},
    crate::paths::{self, *},
    crate::runtime::{self, RuntimeHost},
    anyhow::{bail, Context, Result},
    config::Config,
    pill_abi::*,
    pill_core::{info, LogContext, PillStyle},
    std::ffi::CString,
};

/// Headless runner — replaces run_app() when compiled with the headless feature.
///
/// Skips winit entirely: no window, no event loop. Loads the runtime directly
/// and drives the engine in a tight update loop. Hot-reload is supported via
/// file watchers polled each frame.
///
/// The benchmark (or any headless PillProject) controls its own exit via
/// is_exit_requested().
#[cfg(feature = "headless")]
pub(crate) fn run_app_headless() -> Result<()> {
    // --- 1. Detect project and resolve initial paths ---
    let mut hot_reload_enabled =
        std::env::var("PILL_COMPILE_MODE").ok().as_deref() == Some("hot-reload");

    // Determine the project directory relative to the executable.
    let current_directory_path = std::env::current_exe()
        .context("Failed to get current executable path")?
        .parent()
        .context("Executable has no parent directory")?
        .to_path_buf();

    let project_directory_path = infer_project_directory(&current_directory_path)?;
    let run_layout = resolve_run_layout(&project_directory_path);

    if hot_reload_enabled && run_layout != RunLayout::Development {
        bail!("Hot reload requires development layout paths");
    }

    // Build all the path information the app will need.
    let build_data_directory_path = current_directory_path.join("data");
    let project_resources_directory_path = project_directory_path.join("res");
    let build_resources_directory_path = build_data_directory_path.join("res");

    let project_resources_directory_path = match run_layout {
        RunLayout::Development => project_resources_directory_path,
        RunLayout::Packaged if build_resources_directory_path.exists() => {
            build_resources_directory_path
        }
        RunLayout::Packaged => project_resources_directory_path,
    };

    let project_source_directory_path = project_directory_path.join("src");
    let config_path = project_resources_directory_path.join("config.ini");

    let runtime_load_mode = RuntimeLoadMode::Dylib;

    // --- 2. Resolve engine workspace (only needed for hot-reload) ---
    let engine_source_directory_path = if hot_reload_enabled {
        Some(resolve_engine_workspace_dir(
            &current_directory_path,
            &project_directory_path,
            true,
        )?)
    } else {
        None
    };

    // --- 3. Resolve DLL paths (runtime + project, including hot-reload variants) ---
    let runtime_dynamic_library_path = runtime::resolve_runtime_dylib(
        &build_data_directory_path,
        engine_source_directory_path.as_deref(),
        "pill_runtime",
    )?;

    let runtime_dynamic_library_hot_reloaded_path = if hot_reload_enabled {
        runtime::resolve_runtime_dylib_optional(
            &build_data_directory_path,
            engine_source_directory_path.as_deref(),
            "pill_runtime_hot_reloaded",
        )
        .unwrap_or_else(|| {
            hot_reload_enabled = false;
            runtime_dynamic_library_path.clone()
        })
    } else {
        runtime_dynamic_library_path.clone()
    };

    let project_dynamic_library_path = build_data_directory_path.join(dylib("project"));
    let project_dynamic_library_hot_reloaded_path =
        build_data_directory_path.join(dylib("project_hot_reloaded"));

    // --- 4. Assemble ProjectPaths and clean up stale artifacts from previous runs ---
    let project_paths = ProjectPaths {
        build_data_directory_path,
        engine_source_directory_path,
        project_directory_path,
        project_resources_directory_path,
        project_source_directory_path,
        config_path,
        runtime_dynamic_library_path,
        runtime_dynamic_library_hot_reloaded_path,
        project_dynamic_library_path,
        project_dynamic_library_hot_reloaded_path,
    };

    // Clean up stale hot-reloaded libraries from previous runs.
    if hot_reload_enabled {
        hot_reload::try_remove_files_starting_with(
            &project_paths.build_data_directory_path,
            &format!("{DYLIB_PREFIX}pill_runtime_loaded"),
        );
        hot_reload::try_remove_files_starting_with(
            &project_paths.build_data_directory_path,
            &format!("{DYLIB_PREFIX}project_loaded"),
        );
    }

    // --- 5. Load project configuration and set up logging ---
    let mut config = Config::default();
    config
        .merge(config::File::with_name(
            project_paths.config_path.to_str().unwrap(),
        ))
        .with_context(|| {
            format!(
                "Failed to load config from {}",
                project_paths.config_path.display()
            )
        })?;

    crate::configure_logging(&config);

    // Log hot-reload status; only include watch paths when enabled.
    if hot_reload_enabled {
        info!(
            LogContext::HotReload => "Hot-reload enabled (headless, watching src: {}, res: {})",
            project_paths.project_source_directory_path.display(),
            project_paths.project_resources_directory_path.display()
        );
    } else {
        info!(LogContext::HotReload => "Hot-reload disabled (headless)");
    }
    info!(
        "Initializing {} ({:?} layout, headless)",
        "Standalone".module_object_style(),
        run_layout,
    );

    // --- 6. Initialize file watchers if hot-reload is active ---
    // Set up file watchers for hot-reload.
    let mut file_watchers = if hot_reload_enabled {
        Some(create_file_watchers(&project_paths))
    } else {
        None
    };

    // --- 7. Load runtime DLL and create the engine with null window (headless) ---
    // Load the runtime and create the engine.
    let mut runtime_host = Some(RuntimeHost::load(
        &project_paths.runtime_dynamic_library_path,
        runtime_load_mode,
    )?);

    let project_dylib_cstr = CString::new(
        project_paths
            .project_dynamic_library_path
            .to_string_lossy()
            .as_bytes(),
    )
    .context("Failed to create project dylib path CString")?;
    let resources_cstr = CString::new(
        project_paths
            .project_resources_directory_path
            .to_string_lossy()
            .as_bytes(),
    )
    .context("Failed to create resources path CString")?;
    let config_cstr = CString::new(project_paths.config_path.to_string_lossy().as_bytes())
        .context("Failed to create config path CString")?;

    // Build the FFI args with a null window_ptr — the runtime detects
    // headless mode via its own compile-time feature and uses DummyRenderer.
    let create_args = PillEngineCreateArgsV1 {
        struct_size: std::mem::size_of::<PillEngineCreateArgsV1>() as u32,
        window_ptr: std::ptr::null(), // headless: no window
        project_dylib_path: project_dylib_cstr.as_ptr(),
        project_resources_dir: resources_cstr.as_ptr(),
        config_path: config_cstr.as_ptr(),
        initial_w: 0,
        initial_h: 0,
    };

    runtime_host
        .as_mut()
        .unwrap()
        .create(&create_args)
        .context("RuntimeHost.create failed")?;

    info!("Headless engine initialized — entering update loop");

    // --- 8. Main update loop: advance engine, check exit, poll hot-reload ---
    // Tight update loop — runs until the engine requests exit.
    let mut last_update = std::time::Instant::now();
    let mut last_reload_poll = std::time::Instant::now();
    let window_size = winit::dpi::PhysicalSize::new(0, 0); // dummy for headless
    loop {
        let now = std::time::Instant::now();
        let delta = now.duration_since(last_update);
        last_update = now;

        // --- 8a. Compute delta time and advance the engine by one frame ---
        // Advance the engine by one frame.
        if let Some(host) = runtime_host.as_mut() {
            host.update(delta);

            // Check if the engine has requested graceful exit
            // (e.g. benchmark finished after N frames).
            if host.should_exit() {
                info!("Engine requested exit — shutting down");
                break;
            }
        }

        // --- 8b. Poll file watchers and trigger hot-reload if source changed ---
        // Poll for hot-reload changes after each frame.
        if hot_reload_enabled {
            if let Some(watchers) = file_watchers.as_mut() {
                check_and_reload(
                    &mut runtime_host,
                    None, // headless: no window context
                    &project_paths,
                    &mut last_reload_poll,
                    window_size,
                    watchers,
                    runtime_load_mode,
                )
                .unwrap();
            }
        }

        // Yield to avoid a busy-wait spin at 100% CPU.
        // The engine controls its own frame pacing via the
        // benchmark_headless feature or project-level logic.
        std::thread::yield_now();
    }

    // Explicitly destroy the runtime host before the dylib unloads.
    // RuntimeHost::Drop calls engine.destroy() automatically.
    drop(runtime_host);

    info!("Headless run complete");
    Ok(())
}
