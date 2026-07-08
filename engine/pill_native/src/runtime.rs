//! This file implements the FFI bridge between the native host and pill_runtime.
//!
//! Provides RuntimeHost, which loads the runtime dynamic library via libloading
//! and exposes typed access to every function in the pill_abi FFI vtable.
//! Also contains dylib path resolution helpers used at startup to locate
//! runtime and project shared libraries across build output directories.
//!
//! Dependencies: paths (platform dylib naming), pill_abi (FFI types)

use crate::paths::{self, ProjectPaths, RuntimeLoadMode};
use anyhow::{bail, Context, Result};
use libloading::{Library, Symbol};
use pill_abi::*;
use std::ffi::{c_void, CString};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use winit::window::Window;

// ---------------------------------------------------------------------------
// FFI Bridge - RuntimeCreateContext
// ---------------------------------------------------------------------------

/// Holds the data needed to construct a PillEngineCreateArgsV1 each time
/// the runtime is (re)created. The window Arc is cloned so the runtime
/// receives its own counted reference.
pub(crate) struct RuntimeCreateContext {
    pub(crate) project_resources_dir: CString,
    pub(crate) config_path: CString,
    pub(crate) window: Arc<Window>,
}

impl RuntimeCreateContext {
    /// Builds the FFI argument struct for pill_runtime::create.
    /// Transfers one Arc<Window> reference to the runtime via Arc::into_raw.
    pub(crate) fn make_args(
        &self,
        project_dylib_path: &CString,
        window_size: winit::dpi::PhysicalSize<u32>,
    ) -> PillEngineCreateArgsV1 {
        // The runtime must reconstruct this with Arc::from_raw exactly once.
        let window_raw = Arc::into_raw(Arc::clone(&self.window)) as *const c_void;
        PillEngineCreateArgsV1 {
            struct_size: std::mem::size_of::<PillEngineCreateArgsV1>() as u32,
            window_ptr: window_raw,
            project_dylib_path: project_dylib_path.as_ptr(),
            project_resources_dir: self.project_resources_dir.as_ptr(),
            config_path: self.config_path.as_ptr(),
            initial_w: window_size.width,
            initial_h: window_size.height,
        }
    }
}

// ---------------------------------------------------------------------------
// FFI Bridge - RuntimeHost
// ---------------------------------------------------------------------------

/// Owns a loaded runtime dynamic library and provides typed access to every
/// function in the pill_abi FFI table. All FFI calls go through the `api`
/// vtable so that the host never links directly against pill_runtime symbols.
pub(crate) struct RuntimeHost {
    // Keeps the dynamic library loaded; dropped after `api` is no longer used.
    _lib: Option<Library>,
    // Pointer to the static PillEngineApiV1 exported by the runtime.
    api: *const PillEngineApiV1,
    // Opaque pointer the runtime uses to identify the engine instance.
    handle: EngineHandle,
}

impl RuntimeHost {
    /// Loads the runtime, either by dynamically opening the shared library
    /// or by calling the in-process symbol directly.
    pub(crate) fn load(runtime_dylib_path: &Path, load_mode: RuntimeLoadMode) -> Result<Self> {
        if load_mode == RuntimeLoadMode::InProcess {
            // When compiled in-process the vtable is a direct function call.
            let api = pill_runtime::get_pill_engine_api_v1();
            if api.is_null() {
                bail!("pill_runtime get_pill_engine_api_v1 returned null");
            }

            let runtime_api = unsafe { &*api };
            if runtime_api.abi_version != PILL_ENGINE_ABI_VERSION {
                bail!(
                    "Engine ABI version mismatch runtime {} host {}",
                    runtime_api.abi_version,
                    PILL_ENGINE_ABI_VERSION
                );
            }

            return Ok(Self {
                _lib: None,
                api,
                handle: std::ptr::null_mut(),
            });
        }

        // Dynamic-library path: use libloading to open the .dll/.so/.dylib.
        let lib = unsafe { Library::new(runtime_dylib_path) }.with_context(|| {
            format!(
                "Failed to load runtime dynamic library at {}",
                runtime_dylib_path.display()
            )
        })?;

        let get_api: Symbol<unsafe extern "C" fn() -> *const PillEngineApiV1> =
            unsafe { lib.get(PILL_ENGINE_API_SYMBOL) }
                .context("Missing symbol get_pill_engine_api_v1")?;

        let api = unsafe { get_api() };
        if api.is_null() {
            bail!("pill_engine get_pill_engine_api_v1 returned null");
        }

        let runtime_api = unsafe { &*api };
        if runtime_api.abi_version != PILL_ENGINE_ABI_VERSION {
            bail!(
                "Engine ABI version mismatch runtime {} host {}",
                runtime_api.abi_version,
                PILL_ENGINE_ABI_VERSION
            );
        }

        Ok(Self {
            _lib: Some(lib),
            api,
            handle: std::ptr::null_mut(),
        })
    }

    /// Initialises the engine inside the runtime with the given
    /// creation arguments.  Must be called after `load`.
    pub(crate) fn create(&mut self, args: &PillEngineCreateArgsV1) -> Result<()> {
        let runtime_api = unsafe { &*self.api };
        let ret = (runtime_api.create)(args as *const _, &mut self.handle as *mut _);
        if ret != PILL_OK {
            let error = unsafe { std::ffi::CStr::from_ptr((runtime_api.last_error_utf8)()) };
            bail!("engine create failed: {}", error.to_string_lossy());
        }
        Ok(())
    }

    /// Tears down the engine and releases the runtime's resources.
    /// Safe to call multiple times (no-op if already destroyed).
    fn destroy(&mut self) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.destroy)(self.handle);
        self.handle = std::ptr::null_mut();
    }

    /// Advances the engine by one frame. `delta_time` is the wall-clock
    /// duration since the previous call to `update`.
    pub(crate) fn update(&mut self, delta_time: std::time::Duration) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.update)(self.handle, delta_time.as_nanos() as u64);
    }

    // Notifies the engine that the window has been resized.
    pub(crate) fn resize(&mut self, width: u32, height: u32) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.resize)(self.handle, width, height);
    }

    /// Forwards a raw winit WindowEvent to the engine for egui input
    /// processing (no-op when the debug_ui feature is disabled).
    pub(crate) fn window_event(&mut self, window_event: &winit::event::WindowEvent) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.window_event)(self.handle, window_event as *const _ as *const c_void);
    }

    // Forwards a keyboard event to the engine.
    pub(crate) fn key_event(&mut self, key_event: &winit::event::KeyEvent) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.key_event)(self.handle, key_event as *const _ as *const c_void);
    }

    // Forwards a mouse button press/release.
    pub(crate) fn mouse_button(&mut self, button: u32, pressed: bool) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.mouse_button)(self.handle, button, pressed);
    }

    // Forwards raw mouse motion delta.
    pub(crate) fn mouse_delta(&mut self, delta_x: f64, delta_y: f64) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.mouse_delta)(self.handle, delta_x, delta_y);
    }

    // Forwards cursor position in physical pixels.
    pub(crate) fn cursor_position(&mut self, x: f64, y: f64) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.cursor_position)(self.handle, x, y);
    }

    // Forwards mouse wheel line-scroll deltas.
    pub(crate) fn mouse_wheel_line(&mut self, delta_x: f32, delta_y: f32) {
        if self.handle.is_null() {
            return;
        }

        let runtime_api = unsafe { &*self.api };
        (runtime_api.mouse_wheel_line)(self.handle, delta_x, delta_y);
    }

    /// Instructs the runtime to swap the currently loaded pill project
    /// for a newly compiled one (hot-reload).
    pub(crate) fn reload_project(&mut self, project_dylib_path: &Path) -> Result<()> {
        if self.handle.is_null() {
            bail!("Engine not initialized");
        }

        let runtime_api = unsafe { &*self.api };
        let path = CString::new(project_dylib_path.to_string_lossy().as_bytes())?;
        let ret = (runtime_api.reload_project)(self.handle, path.as_ptr());
        if ret != PILL_OK {
            let error = unsafe { std::ffi::CStr::from_ptr((runtime_api.last_error_utf8)()) };
            bail!("engine reload_project failed: {}", error.to_string_lossy());
        }
        Ok(())
    }

    /// Returns true when the engine has requested graceful shutdown
    /// (for example, a benchmark that finishes after N frames).
    pub(crate) fn should_exit(&self) -> bool {
        if self.handle.is_null() {
            return false;
        }
        let runtime_api = unsafe { &*self.api };
        (runtime_api.is_exit_requested)(self.handle) != 0
    }
}

/// Ensures the runtime engine is always torn down, even if the App
/// is dropped abnormally (e.g. during a panic unwind). The Drop impl
/// guarantees wgpu surface cleanup and DLL unloading in all code paths.
impl Drop for RuntimeHost {
    fn drop(&mut self) {
        self.destroy();
    }
}

// ---------------------------------------------------------------------------
// Dynamic Library Resolution
// ---------------------------------------------------------------------------

// Monotonic counter used to generate unique suffixes for
// hot-reloaded dynamic libraries so that old copies can coexist
// with the newly loaded ones.
static RELOAD_GEN: AtomicU64 = AtomicU64::new(0);

/// Builds a list of candidate paths where a runtime dynamic library
/// might be found, covering both the build data directory and the
/// engine workspace target directories (debug and release).
fn resolve_runtime_dylib_candidates(
    build_data_directory_path: &Path,
    engine_source_directory_path: Option<&Path>,
    name: &str,
) -> Vec<PathBuf> {
    let mut candidates = vec![build_data_directory_path.join(paths::dylib(name))];

    if let Some(engine_source_directory_path) = engine_source_directory_path {
        // Also search the workspace-level target directories.
        if let Some(engine_workspace_root) = engine_source_directory_path.parent() {
            candidates.extend([
                engine_workspace_root
                    .join("target")
                    .join("debug")
                    .join(paths::dylib(name)),
                engine_workspace_root
                    .join("target")
                    .join("release")
                    .join(paths::dylib(name)),
            ]);
        }

        candidates.extend([
            engine_source_directory_path
                .join("target")
                .join("debug")
                .join(paths::dylib(name)),
            engine_source_directory_path
                .join("target")
                .join("release")
                .join(paths::dylib(name)),
        ]);
    }

    candidates
}

/// Finds the first existing candidate for a runtime dynamic library.
/// Errors if no candidate exists on disk.
pub(crate) fn resolve_runtime_dylib(
    build_data_directory_path: &Path,
    engine_source_directory_path: Option<&Path>,
    name: &str,
) -> Result<PathBuf> {
    let candidates = resolve_runtime_dylib_candidates(
        build_data_directory_path,
        engine_source_directory_path,
        name,
    );

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    // Format all checked paths into the error message so the developer
    // can see exactly where the loader looked and why it failed.
    let candidates_display = candidates
        .iter()
        .map(|c| c.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");

    bail!("Failed to find {name} runtime dylib. Checked: {candidates_display}")
}

/// Same search as resolve_runtime_dylib but returns None instead of
/// an error when the library cannot be found.
pub(crate) fn resolve_runtime_dylib_optional(
    build_data_directory_path: &Path,
    engine_source_directory_path: Option<&Path>,
    name: &str,
) -> Option<PathBuf> {
    resolve_runtime_dylib_candidates(
        build_data_directory_path,
        engine_source_directory_path,
        name,
    )
    .into_iter()
    .find(|candidate| candidate.exists())
}

/// Generates a unique file path for a hot-reloaded runtime library by
/// appending a monotonically increasing generation number.
pub(crate) fn next_loaded_runtime_dylib_path(project_paths: &ProjectPaths) -> PathBuf {
    let generation = RELOAD_GEN.fetch_add(1, Ordering::Relaxed);
    project_paths
        .build_data_directory_path
        .join(paths::dylib(&format!("pill_runtime_loaded_{generation}")))
}

/// Generates a unique file path for a hot-reloaded project library.
pub(crate) fn next_loaded_project_dylib_path(project_paths: &ProjectPaths) -> PathBuf {
    let generation = RELOAD_GEN.fetch_add(1, Ordering::Relaxed);
    project_paths
        .build_data_directory_path
        .join(paths::dylib(&format!("project_loaded_{generation}")))
}
