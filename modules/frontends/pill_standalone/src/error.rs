//! Errors of the standalone frontend. [`FrontendError`] holds the `winit`
//! failures, and [`RunError`] is what a run can end with.
//!
//! # Responsibilities
//!
//! - Declares [`FrontendError`], the failures of creating or running the
//!   `winit` event loop and of creating the native window.
//! - Declares [`RunError`], what a run can end with: a setup failure, a
//!   frontend failure, or a renderer failure.
//!
//! # Design
//!
//! Both types live in the crate that owns the event loop. `pill_core` is the
//! shared dylib that every module, project and hot patch imports, so a
//! `winit` type placed there would add the windowing library to all of their
//! dependency graphs, for an error only this binary constructs.
//! `pill_player::RenderingError` already combines the windowed runtime's setup
//! and renderer failures. [`RunError`] adds the frontend arm, and a build
//! without the `rendering` feature has neither that arm nor the renderer arm.

// Standard library
use std::convert::Infallible;

// External crates
use pill_core_macros::engine_error;

// =============================================================================
// Frontend Errors
// =============================================================================

/// Failures of the windowed frontend, all reported by `winit`.
///
/// Only a build with the `rendering` feature has this type. It is raised while
/// the event loop is created or run, or while the native standalone window is
/// created.
#[cfg(feature = "rendering")]
#[engine_error(namespace = host::frontend, runtime = ::pill_core::error)]
pub enum FrontendError {
    /// The `winit` event loop could not be created, or failed while running.
    #[message("failed to create the event loop")]
    EventLoopCreation {
        #[source]
        source: winit::error::EventLoopError,
    },

    /// The native window could not be created.
    #[message("failed to create the standalone host window")]
    WindowCreation {
        #[source]
        source: winit::error::OsError,
    },
}

// =============================================================================
// Run Errors
// =============================================================================

/// Anything that can end a standalone run.
///
/// Every arm passes through to the error it carries, so `?` keeps the original
/// error's diagnostic code, message and source chain.
#[engine_error(namespace = standalone::run, runtime = ::pill_core::error)]
pub enum RunError {
    /// Setting up the host or runtime failed before any frame ran.
    #[transparent]
    Host(#[from] pill_core::error::HostError),

    /// The event loop could not be created or run, or the window could not be
    /// created.
    #[cfg(feature = "rendering")]
    #[transparent]
    Frontend(#[from] FrontendError),

    /// The renderer failed: it could not create a GPU surface or device, a
    /// resource it needed is missing, or it could not draw a frame.
    #[cfg(feature = "rendering")]
    #[transparent]
    Renderer(#[from] pill_player::RendererError),
}

#[cfg(feature = "rendering")]
impl From<pill_player::RenderingError> for RunError {
    /// `pill_player::RenderingError` is this enum without the frontend arm, so
    /// each of its arms goes into the matching arm here.
    fn from(error: pill_player::RenderingError) -> Self {
        match error {
            pill_player::RenderingError::Host(error) => Self::Host(error),
            pill_player::RenderingError::Renderer(error) => Self::Renderer(error),
        }
    }
}

impl From<Infallible> for RunError {
    /// The headless driver's frame cannot fail, so its error type is
    /// `Infallible`. The headless loop's `?` needs `RunError: From<Infallible>`,
    /// and this conversion provides it.
    fn from(never: Infallible) -> Self {
        match never {}
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    // `code()` is a method of the diagnostic trait that `#[engine_error]`
    // implements.
    use miette::Diagnostic as _;

    /// The `Host` arm passes through to the error it carries, so it reports
    /// that error's code rather than one of its own.
    #[test]
    fn a_composed_arm_reports_the_leaf_code() {
        let host_failure = RunError::from(pill_core::error::HostError::from(
            pill_core::error::ConfigError::EmptyModuleName,
        ));
        assert_eq!(
            host_failure.code().map(|code| code.to_string()).as_deref(),
            Some("host::config::empty_module_name")
        );
    }
}
