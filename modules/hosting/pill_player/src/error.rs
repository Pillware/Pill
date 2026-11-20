//! Errors of a windowed runtime.
//!
//! # Responsibilities
//!
//! - Declares [`RenderingError`], the failure that stops a windowed runtime
//!   from starting or running: host setup failed, or the renderer did.
//!
//! # Design
//!
//! The engine cannot name a renderer error: the renderer contract already
//! depends on the engine, so the engine cannot also depend on the contract.
//! This crate depends on both and defines the one error that names both
//! failure sources. Window and event-loop failures stay with the frontend
//! that owns the event loop; a frontend that reports a whole run as one
//! error composes this type with its own.

// External crates
use pill_core_macros::engine_error;

// =============================================================================
// Rendering Errors
// =============================================================================

/// Anything that can go wrong while a windowed runtime is set up or runs.
///
/// Both arms are transparent, so `?` keeps the original error's code, message
/// and source chain.
#[engine_error(namespace = runtime::rendering, runtime = ::pill_core::error)]
pub enum RenderingError {
    /// Host setup failed. In the development host this includes starting its
    /// renderer module, whose configuration or source watcher can fail. The
    /// error is a [`pill_core::error::HostError`].
    #[transparent]
    Host(#[from] pill_core::error::HostError),

    /// The renderer failed: registration or attach, the GPU's surface or
    /// device, a resource it needs, or a frame it could not draw.
    #[transparent]
    Renderer(#[from] pill_renderer_api::RendererError),
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    // `code()` comes from the `miette::Diagnostic` trait, which `#[engine_error]` implements.
    use miette::Diagnostic as _;

    /// Both arms are transparent, so a composed error reports the code of the
    /// error it carries, not one of its own.
    #[test]
    fn every_composed_arm_reports_the_leaf_code() {
        let renderer_failure =
            RenderingError::Renderer(pill_renderer_api::RendererError::RendererResourceNotFound);
        assert_eq!(
            renderer_failure
                .code()
                .map(|code| code.to_string())
                .as_deref(),
            Some("engine::renderer::renderer_resource_not_found")
        );

        let host_failure = RenderingError::from(pill_core::error::HostError::from(
            pill_core::error::ConfigError::EmptyModuleName,
        ));
        assert_eq!(
            host_failure.code().map(|code| code.to_string()).as_deref(),
            Some("host::config::empty_module_name")
        );
    }
}
