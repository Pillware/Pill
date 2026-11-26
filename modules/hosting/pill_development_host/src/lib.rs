//! The development host: it builds a project and its extensions as loadable
//! modules and reloads them while the project runs.
//!
//! # Responsibilities
//!
//! - Builds and loads native or C# project modules and the extensions.
//! - Watches project sources and runs hot reloads at frame boundaries, just
//!   before the wrapped [`pill_player::Runtime`] runs its frame.
//! - Exposes [`setup`], [`run_frame`] and, when the host draws into a window,
//!   [`setup_rendering`] and [`attach_renderer`]. A frontend owns its event
//!   loop and drives the host through [`FrameDriver`].
//!
//! # Design
//!
//! The configuration types ([`ProjectModuleConfig`], [`HostConfig`]) keep
//! backend selection out of executable crates. Running the game is
//! `pill_player`'s job: the engine, the frame, the statically linked project
//! and the renderer's window. A shipping build links that crate without this
//! one. Everything this crate does is development work, so nothing here is
//! behind a feature: what a run watches is decided at startup by the switches
//! in [`switches`].

// =============================================================================
// Module Declarations
// =============================================================================

/// Collects build, load and reload measurements and prints them as console
/// reports.
///
/// Its measurements come from the build, load and reload pipeline, so a
/// statically linked build has nothing to report.
mod analytics;
/// The startup build plan: logs the modules a start will build, then counts
/// each build as it begins.
mod build_progress;
/// Build commands for the project and its extensions, artifact staging and
/// output-path resolution.
mod build_runner;
/// Project and extension configuration shared by every host frontend.
mod config;
/// C# development tooling on top of `pill_csharp_bridge`: generates the C#
/// mirrors, runs the in-process compiler, and re-exports the bridge items the
/// host names.
///
/// A shipped C# project needs none of it: it starts through the bridge's
/// `CSharpBackend`, run by the bridge's runtime.
mod csharp;
/// Per-function hot patching: decides which edits can be patched, then
/// generates, compiles and activates the patch.
mod hot_patch;

/// Builds, loads and reloads the extension modules.
mod extension;
/// What a module's resources declared their layouts are made of, and whether
/// that changed since the last load.
mod layout_record;
/// Reads a native image's dependencies, imports and exports from its bytes, so
/// a failed load can name the symbol and the dylib behind it.
mod native_binary;
/// Loads native module libraries from temporary copies that must outlive a
/// reload.
mod native_library;
/// Reloads, patches, failures and rollbacks, queued as notices for an attached
/// editor.
mod notices;
/// The loaded project module: its startup build and load, and each reload
/// after that.
mod project_module;
/// The project's settings: its declarations, its file pair and the resolution
/// every consumer asks.
mod project_settings;
/// The steps every reload runs once its replacement image is loaded.
mod reload;
/// The start-up and quit progress a frontend polls: one entry per module and
/// how far its build, load or release has got.
pub mod startup_progress;
/// What this run's watchers are allowed to do, read from the `PILL_*` switches.
mod switches;

/// The GPU renderer as a loaded module: builds and attaches it, then drives
/// its backend.
#[cfg(feature = "rendering")]
mod renderer_module;
/// The development host: `DevHost` and `RenderingHost`, with the functions
/// that set them up, run their frames and reload their modules.
mod runtime;
/// The file watchers: the project's source tree, its asset directory and a
/// shader source tree, each on its own worker thread.
mod watchers;

/// The project's assets as a tool sees them: the tree listing, import settings
/// and moves.
mod asset_browser;

/// Test-only helpers: temporary directories and explicit file dates.
#[cfg(test)]
mod scratch;

// =============================================================================
// Re-exports
// =============================================================================

/// The editor's project layer and its reader, used by the editor executable
/// and never by a game build.
pub use config::resolve_editor_theme;
/// The host's configuration: the project, the extensions to load and the
/// selected renderer.
pub use config::{ExtensionConfig, HostConfig, ProjectModuleBackend, ProjectModuleConfig};
/// The module ABI revision an extension must report, checked before the host
/// calls into it.
pub use extension::EXTENSION_ABI_VERSION;
/// The notice type an editor frontend reads, and the function that empties the
/// queue each frame.
pub use notices::{take as take_notices, HostNotice};
/// The error types frontends report through, with the message renderers and
/// the installed report handler.
pub use pill_core::error::{
    engine_report, install_engine_report_handler, BuildError, CSharpError, ConfigError,
    EngineMessage, EngineReportHandler, HostError, LibraryError, MessageRenderer, ModuleError,
    PlainMessageRenderer, SemanticRole, StyledDiagnosticProxy, TerminalMessageRenderer,
    WatcherError,
};
/// The assembly names and output directories of a managed project, as
/// [`ProjectModuleBackend`] stores them; defined by the C# bridge.
pub use pill_csharp_bridge::CSharpModuleConfig;
/// What a frontend's loop needs from whatever runs the game. The development
/// host implements it, as the shipping runtimes do.
pub use pill_player::FrameDriver;
/// The project's settings: the file pair, the declarations and the editor's
/// layer.
pub use project_settings::{
    declared_settings_schemas, read_editor_settings, EditorSettings, ProjectSettings,
    PROJECT_SETTINGS_FILE, PROJECT_SETTINGS_LOCAL_FILE,
};

/// The project's assets as an editor tool sees them, plus the standalone asset
/// types a new asset can be created for.
pub use asset_browser::{standalone_file_name, standalone_types, AssetEntry, StandaloneType};
/// One entry in a patched function's history, as returned by
/// [`DevHost::patch_generations`](runtime::DevHost::patch_generations).
pub use hot_patch::PatchGeneration;
/// The development host and the entry points that create and drive it without
/// a window.
pub use runtime::{run_frame, setup, DevHost};

/// The engine's composed error. It has no rendering variants, so headless
/// frontends use it too.
pub use pill_engine::EngineError;

// These come through the host so a frontend never names a renderer crate and
// needs no wgpu dependency of its own. A headless build has no renderer, so it
// also has no viewport to describe.

/// The bound a frontend's window type meets so a renderer can attach to it,
/// and the error a windowed runtime returns.
#[cfg(feature = "rendering")]
pub use pill_player::{RendererWindow, RenderingError};
/// The rectangle of a render target a renderer draws into, and the renderer's
/// error.
#[cfg(feature = "rendering")]
pub use pill_renderer_api::{RenderViewport, RendererError};

/// Rendering-only entry points: the windowed host, and attaching the renderer
/// to a frontend's window.
#[cfg(feature = "rendering")]
pub use runtime::{attach_renderer, setup_rendering, setup_rendering_with_tick, RenderingHost};
