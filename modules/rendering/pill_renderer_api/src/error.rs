//! Public renderer failures at the host boundary.
//!
//! # Responsibilities
//!
//! - Declare [`RendererError`], the failures a renderer reports while it is
//!   built, when a surface is attached and while frames are presented.
//! - Provide the [`Result`] alias and [`ErrorContext`], so a call site can
//!   attach a description of what it was doing to any failure beneath it.
//!
//! # Design
//!
//! The variants carry plain strings rather than `wgpu` error values. The host
//! links this crate in every build and reports renderer failures without
//! naming a graphics type, while the renderer module turns its wgpu errors
//! into these. The `engine::renderer` namespace is also used by the 2D
//! renderer's error enum, so a code such as
//! `engine::renderer::surface_creation` means the same condition whichever of
//! the two reports it.

// External crates
use pill_core_macros::engine_error;

// =============================================================================
// Renderer Errors
// =============================================================================

/// Failures the renderer reports to its host.
///
/// Only failures a host or frontend acts on get a variant of their own. The
/// backend's startup and swapchain failures all arrive as [`Self::Backend`],
/// with the text the GPU module wrote, so a new way for a device to fail needs
/// no change here.
#[engine_error(namespace = engine::renderer, runtime = ::pill_core::error)]
pub enum RendererError {
    /// The GPU surface could not be created for the supplied window.
    #[message("failed to create the GPU surface: ", value(detail))]
    SurfaceCreation { detail: String },
    /// The GPU backend failed: no adapter or no device, no format or alpha
    /// mode to present with, a configuration the driver refused, or a
    /// swapchain that stayed lost or gave no texture.
    ///
    /// wgpu's error values stay inside the renderer module, which writes their
    /// text into `detail`. The host acts on the one thing the variant says: the
    /// backend did not start, or did not hand this frame a surface.
    #[message("the GPU backend failed: ", value(detail))]
    Backend { detail: String },
    /// The renderer cannot draw into the surface the caller asked for. It may
    /// draw a single surface only, the id may name the surface it was built
    /// on, or the surface cannot use the format the renderer draws with.
    #[message("the renderer cannot draw into this surface: ", value(detail))]
    UnsupportedSurface { detail: String },

    /// A resource the call referenced - a camera, a shader - is not in
    /// storage.
    #[message("renderer resource was not found")]
    RendererResourceNotFound,
    /// A failure the caller described in its own words: the pass that reads a
    /// target no pass writes, the texture that is not loaded, the shader wgpu
    /// refused.
    ///
    /// The message has no generic prefix, because the caller already says what
    /// it was doing - `Pass <name> is not drawn: <this>`. Repeating it would
    /// read as a stutter.
    #[message(value(detail))]
    Other { detail: String },
}

// =============================================================================
// Result Alias
// =============================================================================

/// The result type used across this crate: the value on success,
/// [`RendererError`] on failure.
pub type Result<T> = std::result::Result<T, RendererError>;

// =============================================================================
// Error Context
// =============================================================================

/// Turns any lower-level failure into a [`RendererError::Other`] carrying a
/// description of what the caller was doing.
///
/// Implemented for every `Result<T, E>` whose error implements `Display`, and
/// for `Option<T>`. A call site can then name its own operation - loading this
/// texture, building this pass - and the enum needs no variant per operation.
pub trait ErrorContext<T> {
    /// Returns the value, or [`RendererError::Other`] describing the failure.
    ///
    /// `message` says what the caller was doing.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::Other`] when the value is absent. The detail
    /// holds `message`, followed by the original error's text when the value
    /// was a `Result`.
    fn context(self, message: impl Into<String>) -> Result<T>;
}

impl<T, E: std::fmt::Display> ErrorContext<T> for std::result::Result<T, E> {
    fn context(self, message: impl Into<String>) -> Result<T> {
        let message = message.into();
        self.map_err(|error| RendererError::Other {
            detail: format!("{message}: {error}"),
        })
    }
}

impl<T> ErrorContext<T> for Option<T> {
    fn context(self, message: impl Into<String>) -> Result<T> {
        self.ok_or_else(|| RendererError::Other {
            detail: message.into(),
        })
    }
}
