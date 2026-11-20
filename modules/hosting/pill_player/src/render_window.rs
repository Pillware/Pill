//! Attaching a renderer to a frontend's window.
//!
//! # Responsibilities
//!
//! - Define [`RendererWindow`], the trait a frontend's window type must
//!   implement: any window that hands out `raw-window-handle` 0.6 handles,
//!   such as a winit or tao window.
//! - Attach a renderer to such a window using its handles as plain data
//!   ([`attach_renderer_to_window`], or
//!   [`attach_renderer_to_window_async`] when the renderer is built by a
//!   future), and keep the window alive beside it.
//!
//! # Design
//!
//! The renderer never sees a window type, only [`RawWindowData`]. This keeps
//! a windowing crate out of the renderer, so every frontend attaches its own
//! window type the same way.
//!
//! The handles do not keep the window alive. The window is therefore
//! returned with the renderer as an [`AttachedWindow`], and whoever stores
//! the two must drop the renderer before the window.

// Standard library
use std::any::Any;
use std::future::Future;

// External crates
use pill_renderer_api::raw_window::raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use pill_renderer_api::{PillRenderer, RawWindowData, RendererError};

// =============================================================================
// Renderer Window
// =============================================================================

/// A window the renderer can be attached to.
///
/// Every type that hands out `raw-window-handle` 0.6 window and display
/// handles implements this trait automatically. That includes winit's and
/// tao's windows, and an `Arc` of either, so a frontend can pass its own
/// window type.
pub trait RendererWindow: HasWindowHandle + HasDisplayHandle + 'static {}
impl<T> RendererWindow for T where T: HasWindowHandle + HasDisplayHandle + 'static {}

/// A frontend window held for the lifetime of the renderer drawn on it.
///
/// The window type is erased, so the value is only meant to be stored. Keep
/// it in a field declared after the renderer's field, so the renderer is
/// dropped first.
pub(crate) type AttachedWindow = Box<dyn Any>;

// =============================================================================
// Window Attachment
// =============================================================================

/// Attaches a renderer to `window` through `attach_renderer`, and returns the
/// renderer, the window the renderer must not outlive, and the window's
/// handles as data. A caller keeps the handles to attach another renderer to
/// the same window later.
///
/// `attach_renderer` builds the renderer from the window's handles. The
/// caller must store the renderer and the window so that the renderer is
/// dropped first: in a struct, declare the renderer field before the window
/// field. That order is what makes it sound for `attach_renderer` to create
/// a surface from the handles.
///
/// # Errors
///
/// Returns a [`RendererError`] if the window does not give its handles, if
/// its platform is not one [`RawWindowData`] carries, if its window and
/// display handles come from different platforms, or if `attach_renderer`
/// fails.
pub(crate) fn attach_renderer_to_window<W: RendererWindow>(
    window: W,
    attach_renderer: impl FnOnce(RawWindowData) -> Result<Box<dyn PillRenderer>, RendererError>,
) -> Result<(Box<dyn PillRenderer>, AttachedWindow, RawWindowData), RendererError> {
    let window_data = RawWindowData::from_window(&window)?;
    let renderer = attach_renderer(window_data)?;
    Ok((renderer, Box::new(window), window_data))
}

/// Attaches a renderer that is built asynchronously to `window`, like
/// [`attach_renderer_to_window`], except that `attach_renderer` returns a
/// future. The window is kept while the future runs.
///
/// # Errors
///
/// Returns a [`RendererError`] if the window does not give its handles, if
/// its platform is not one [`RawWindowData`] carries, if its window and
/// display handles come from different platforms, or if the future that
/// `attach_renderer` returns fails.
pub(crate) async fn attach_renderer_to_window_async<W, F>(
    window: W,
    attach_renderer: impl FnOnce(RawWindowData) -> F,
) -> Result<(Box<dyn PillRenderer>, AttachedWindow, RawWindowData), RendererError>
where
    W: RendererWindow,
    F: Future<Output = Result<Box<dyn PillRenderer>, RendererError>>,
{
    let window_data = RawWindowData::from_window(&window)?;
    let renderer = attach_renderer(window_data).await?;
    Ok((renderer, Box::new(window), window_data))
}
