//! The native editor's errors.
//!
//! # Responsibilities
//!
//! - Declares [`EditorError`], what an editor run can end with: event-loop,
//!   window and GPU device creation, host setup, or a frame through the
//!   project's renderer.
//!
//! # Design
//!
//! The error lives in the crate that owns the event loop, like
//! `pill_standalone`'s. The `wgpu` and `winit` errors are carried unchanged,
//! so the report printed when the run ends shows the driver's own message.

// External crates
use pill_core_macros::engine_error;

/// Anything that can end an editor run.
#[engine_error(namespace = editor::run, runtime = ::pill_core::error)]
pub enum EditorError {
    /// The `winit` event loop could not be created.
    #[message("failed to create the editor event loop")]
    EventLoopCreation {
        #[source]
        source: winit::error::EventLoopError,
    },

    /// The event loop's `run_app` reported a failure.
    #[message("the editor event loop failed")]
    EventLoopRun {
        #[source]
        source: winit::error::EventLoopError,
    },

    /// Setting up the host failed before any frame ran.
    #[transparent]
    Host(#[from] pill_core::error::HostError),

    /// A frame through the project's renderer failed.
    #[transparent]
    Renderer(#[from] pill_player::RendererError),

    /// The native window could not be created.
    #[message("failed to create the editor window")]
    WindowCreation {
        #[source]
        source: winit::error::OsError,
    },

    /// No GPU adapter could be found for the editor.
    #[message("failed to find a GPU adapter for the editor")]
    AdapterRequest {
        #[source]
        source: wgpu::RequestAdapterError,
    },

    /// The editor's GPU device could not be created.
    #[message("failed to create the editor GPU device")]
    DeviceRequest {
        #[source]
        source: wgpu::RequestDeviceError,
    },
}

impl From<pill_player::RenderingError> for EditorError {
    /// `pill_player::RenderingError` is this enum without the frontend arms,
    /// so each of its two arms maps onto the one of the same name here.
    fn from(error: pill_player::RenderingError) -> Self {
        match error {
            pill_player::RenderingError::Host(error) => Self::Host(error),
            pill_player::RenderingError::Renderer(error) => Self::Renderer(error),
        }
    }
}
