//! Rendering failures reported by the wgpu backend.
//!
//! # Responsibilities
//!
//! - Declares [`RendererError`], covering surface, adapter, device and
//!   frame-acquisition failures.
//!
//! # Design
//!
//! The type lives here rather than in `pill_engine` because most variants wrap
//! a `wgpu` error. `pill_engine` is compiled into every loaded module and every
//! hot patch, so one wgpu type reachable from it would pull the whole graphics
//! stack into all of them.
//!
//! Diagnostic codes use the `engine::renderer` namespace, for example
//! `engine::renderer::no_texture_formats`. A report built from the error shows
//! that code beside the message.

// External crates
use pill_core_macros::engine_error;

// =============================================================================
// Renderer Errors
// =============================================================================

/// Rendering initialization or presentation failures of the wgpu backend.
#[engine_error(namespace = engine::renderer, runtime = ::pill_core::error)]
pub enum RendererError {
    /// The GPU surface could not be created for the supplied window.
    #[message("failed to create the GPU surface")]
    SurfaceCreation {
        #[source]
        source: wgpu::CreateSurfaceError,
    },

    /// No compatible GPU adapter could be found.
    #[message("failed to find a compatible GPU adapter")]
    AdapterRequest {
        #[source]
        source: wgpu::RequestAdapterError,
    },

    /// The GPU device could not be created from the adapter.
    #[message("failed to create the GPU device")]
    DeviceCreation {
        #[source]
        source: wgpu::RequestDeviceError,
    },

    /// The surface exposes no texture formats.
    #[message("GPU surface exposes no texture formats")]
    NoTextureFormats,

    /// The surface exposes no alpha modes.
    #[message("GPU surface exposes no alpha modes")]
    NoAlphaModes,

    /// The surface refused every presentation configuration it was offered.
    ///
    /// `Surface::configure` returns nothing. A refusal goes to the device's
    /// uncaptured-error path (where wgpu sends errors that no error scope
    /// captured) and panics by default. So the renderer tries each fallback
    /// configuration inside its own error scope, and returns this variant only
    /// when all of them fail. The backend error cannot be carried out of
    /// `configure`, so each refusal is printed where it happens.
    #[message("failed to configure the GPU surface for presentation")]
    SurfaceConfigure,

    /// The frame texture could not be acquired, for a reason the renderer
    /// cannot recover from.
    #[message("failed to acquire the GPU surface texture")]
    SurfaceTextureFailed {
        #[source]
        source: wgpu::SurfaceError,
    },
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    // `code()` comes from the `miette::Diagnostic` trait, which
    // `#[engine_error]` implements.
    use miette::Diagnostic as _;

    /// Renderer unit variants produce their diagnostic code.
    ///
    /// The code is derived from the namespace and the variant name, so the
    /// variant alone decides the suffix a user sees.
    #[test]
    fn renderer_error_code_derives_from_namespace_and_variant() {
        assert_eq!(
            RendererError::NoTextureFormats
                .code()
                .map(|code| code.to_string())
                .as_deref(),
            Some("engine::renderer::no_texture_formats")
        );
    }
}
