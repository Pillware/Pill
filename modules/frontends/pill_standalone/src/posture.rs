//! What this binary runs: the development host, or a shipped project.
//!
//! # Responsibilities
//!
//! - Name the project type, the headless and windowed drivers, and the setup
//!   and renderer-attach steps for the posture this binary was built with.
//!
//! # Design
//!
//! The build mode picks what this binary runs: `development` compiles the
//! development host, `shipping` compiles the linked project, and the other
//! mode's code is not in the binary at all. "Posture" is the crate's word for
//! what a mode compiles to.
//!
//! The loops in `headless_standalone` and `windowed_standalone` are written
//! once, against [`pill_player::FrameDriver`]. Only setting a driver up
//! differs, and that difference lives here: the file defines the same private
//! module twice, one definition per mode, and the `development` and `shipping`
//! features pick which one compiles. `main.rs` refuses a build that has
//! neither mode or both. A shipping build does not compile
//! `pill_development_host` at all, and each posture's windowed half exists
//! only under the `rendering` feature.

// =============================================================================
// Development Posture
// =============================================================================

/// The development posture: `pill_development_host` builds, loads and reloads
/// the project.
#[cfg(feature = "development")]
mod compiled_posture {
    // External crates
    use pill_core::error::HostError;
    #[cfg(feature = "rendering")]
    use pill_player::{RendererWindow, RenderingError};

    /// The development host's configuration: the project and the extensions
    /// it builds, watches and reloads.
    pub type ProjectSource = pill_development_host::HostConfig;
    /// The development host before a renderer window is attached.
    pub type HeadlessDriver = pill_development_host::DevHost;
    /// The development host with the renderer attached to one window.
    #[cfg(feature = "rendering")]
    pub type WindowedDriver = pill_development_host::RenderingHost;

    /// Builds and loads the project and its extensions, then starts their
    /// source watchers.
    ///
    /// # Errors
    ///
    /// Returns a [`HostError`] naming the failing subsystem: configuration,
    /// build, library loading, watcher startup or managed backend startup.
    pub fn setup_driver(project: ProjectSource) -> Result<HeadlessDriver, HostError> {
        pill_development_host::setup(project)
    }

    /// Loads the renderer module and attaches it to `window`.
    ///
    /// Call it after [`setup_driver`], once a window exists, so a slow first build
    /// never shows a blank surface. `window_width_pixels` and
    /// `window_height_pixels` hold the window's size in physical pixels.
    ///
    /// # Errors
    ///
    /// Returns a [`RenderingError`]: a [`HostError`] when the renderer module
    /// cannot be built, loaded or initialized, or a `RendererError` when
    /// attaching it to the window fails.
    #[cfg(feature = "rendering")]
    pub fn attach_renderer<W: RendererWindow>(
        headless_driver: HeadlessDriver,
        window: W,
        window_width_pixels: u32,
        window_height_pixels: u32,
    ) -> Result<WindowedDriver, RenderingError> {
        pill_development_host::attach_renderer(
            headless_driver,
            window,
            window_width_pixels,
            window_height_pixels,
        )
    }
}

// =============================================================================
// Shipping Posture
// =============================================================================

/// The shipping posture: `pill_player` runs the project that the bundle links
/// into this binary.
#[cfg(feature = "shipping")]
mod compiled_posture {
    // External crates
    use pill_core::error::HostError;
    #[cfg(feature = "rendering")]
    use pill_player::{RendererWindow, RenderingError};

    /// What the shipping runtime runs: the project and extensions linked into
    /// this binary.
    pub type ProjectSource = pill_player::StaticProject;
    /// The engine and its frame, before a renderer window is attached.
    pub type HeadlessDriver = pill_player::Runtime;
    /// The linked project running in a window.
    #[cfg(feature = "rendering")]
    pub type WindowedDriver = pill_player::RenderingRuntime;

    /// Mounts the asset pack the project embeds, then registers the linked
    /// extensions and the project, in that order.
    ///
    /// # Errors
    ///
    /// Returns a [`HostError`] when the embedded pack cannot be parsed as an
    /// asset pack, when an extension's or the project's entry point reports a
    /// non-zero initialization status, or when an external backend fails to
    /// start.
    pub fn setup_driver(project: ProjectSource) -> Result<HeadlessDriver, HostError> {
        pill_player::setup(project)
    }

    /// Registers the linked renderer and attaches it to `window`.
    ///
    /// The attach is asynchronous because building the renderer is
    /// asynchronous. This frontend waits for it once, before the first frame.
    /// `window_width_pixels` and `window_height_pixels` hold the window's size
    /// in physical pixels.
    ///
    /// # Errors
    ///
    /// Returns a [`RenderingError`] when the renderer's registration fails,
    /// when it cannot attach to the window, or when this build links no
    /// renderer.
    #[cfg(feature = "rendering")]
    pub fn attach_renderer<W: RendererWindow>(
        headless_driver: HeadlessDriver,
        window: W,
        window_width_pixels: u32,
        window_height_pixels: u32,
    ) -> Result<WindowedDriver, RenderingError> {
        pill_core::platform::futures::block_on(pill_player::attach_renderer(
            headless_driver,
            window,
            window_width_pixels,
            window_height_pixels,
        ))
    }
}

// =============================================================================
// Re-exports
// =============================================================================

/// The compiled posture's project type, drivers and setup steps.
pub use compiled_posture::*;
