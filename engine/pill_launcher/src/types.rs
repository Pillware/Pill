// This file defines the shared enums used by every other module in the launcher.
//
// Responsibilities:
// - Location: maps logical repo paths (engine crates, examples root, etc.) to absolute dirs.
// - CompileMode: selects debug, release, or hot-reload build profile.
// - BuildTarget: chooses between native standalone and WASM/WebGPU output.

use std::fmt;

/// Which part of the repository a path points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum Location {
    /// Repository root (parent of engine/).
    EngineProjectRoot,
    /// The engine/ workspace directory (contains pill_core, pill_engine, …).
    EngineCrates,
    /// The pill_engine crate.
    PillEngineCrate,
    /// The pill_core crate.
    PillCoreCrate,
    /// The pill_native crate (standalone host executable).
    PillNativeCrate,
    /// The pill_launcher crate itself.
    PillLauncherCrate,
    /// The pill_renderer crate.
    PillRendererCrate,
    /// The pill_abi crate.
    PillAbiCrate,
    /// The pill_assets crate.
    PillAssetsCrate,
    /// The pill_runtime crate.
    PillRuntimeCrate,
    /// The pill_web crate.
    PillWebCrate,
}

/// Build profile: debug, release, or hot-reload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CompileMode {
    Debug,
    Release,
    HotReload,
}

impl fmt::Display for CompileMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileMode::Debug => write!(f, "debug"),
            CompileMode::Release => write!(f, "release"),
            CompileMode::HotReload => write!(f, "hot-reload"),
        }
    }
}

/// Build target: native standalone executable or WASM+WebGPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BuildTarget {
    Native,
    Web,
}

impl fmt::Display for BuildTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildTarget::Native => write!(f, "native"),
            BuildTarget::Web => write!(f, "web"),
        }
    }
}
