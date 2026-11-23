//! The C ABI a host uses to drive this renderer after loading it as a module.
//!
//! # Responsibilities
//!
//! - Export `pill_renderer_attach`, which builds a renderer on a window that
//!   the host describes as [`RawWindowData`], and `pill_renderer_detach`,
//!   which drops that renderer inside this image.
//! - Keep panics and failures from crossing the C boundary: both exports catch
//!   unwinds, and an attach that fails is logged here and returns null.
//!
//! # Design
//!
//! The two exports are the same names the GPU renderer exports, and carry the
//! same contract, so the host loads and reloads either renderer with no
//! change: which one a run gets is the only difference.
//!
//! The renderer crosses the boundary as a `Box<Box<dyn PillRenderer>>` turned
//! into a raw pointer, so the image that allocated it is the one that frees it.
//! Calling a trait object across the boundary only works when the host and the
//! module compiled the same `pill_renderer_api`.
//!
//! The functions here are plain library code. The `#[no_mangle]` symbols come
//! from `__pill_renderer_entry_points!`, which the renderer's wrapper crate
//! expands: they cannot be defined here, because this library is also linked
//! into the host's own dependency graph and every renderer declares the same
//! two names.

// Standard library
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};

// External crates
use pill_core::telemetry::telemetry_target;
use pill_renderer_api::{PillRenderer, RawWindowData};

// Current crate
use crate::renderer::{panel_from_environment, EmbeddedRenderer};

/// Builds a renderer for the window `window` describes.
///
/// Returns a `Box<Box<dyn PillRenderer>>` as a raw pointer. The return is null
/// when `window` is null or when attaching panicked; every failure is logged
/// here.
///
/// The renderer draws its frames at the panel's resolution but does not show
/// them in the window yet: the window blit is the piece this stage still owes,
/// and it is why the pointer's size is accepted and ignored for now.
///
/// # Safety
///
/// `window`, when non-null, points to a valid [`RawWindowData`] that names a
/// live window. The caller keeps that window alive until it has passed the
/// returned pointer to [`detach_backend`].
pub unsafe extern "C" fn attach_to_window(
    window: *const RawWindowData,
    width: u32,
    height: u32,
) -> *mut c_void {
    // Step 1: A null window leaves nothing to attach to.
    if window.is_null() {
        pill_core::error!(
            target: telemetry_target::ENGINE,
            "renderer attach was given no window"
        );
        return std::ptr::null_mut();
    }
    // Step 2: Read the handles out and build the renderer, inside
    // `catch_unwind` so a panic cannot unwind into the host.
    // SAFETY: the pointer is not null and the contract above says it points at
    // a valid `RawWindowData`. The value is `Copy`, so the read copies plain
    // handle numbers.
    let window = unsafe { *window };
    let attach_attempt = catch_unwind(AssertUnwindSafe(|| {
        let renderer = EmbeddedRenderer::new(panel_from_environment());
        let _ = (width, height, window);
        Ok::<EmbeddedRenderer, pill_renderer_api::RendererError>(renderer)
    }));
    // Step 3: Hand the renderer back as an opaque pointer, or log and return
    // null.
    match attach_attempt {
        Ok(Ok(renderer)) => {
            let renderer_backend: Box<dyn PillRenderer> = Box::new(renderer);
            Box::into_raw(Box::new(renderer_backend)).cast()
        }
        Ok(Err(error)) => {
            pill_core::error!(
                target: telemetry_target::ENGINE,
                "the embedded renderer could not attach to the window: {error}"
            );
            std::ptr::null_mut()
        }
        Err(_) => {
            pill_core::error!(
                target: telemetry_target::ENGINE,
                "the embedded renderer panicked while attaching to the window"
            );
            std::ptr::null_mut()
        }
    }
}

/// Drops a renderer returned by [`attach_to_window`], inside this image.
///
/// A null `backend` is ignored.
///
/// # Safety
///
/// `backend` is null, or a pointer that [`attach_to_window`] of this same image
/// returned. It must not have been detached already, and the caller must not
/// use it again afterwards.
pub unsafe extern "C" fn detach_backend(backend: *mut c_void) {
    // Step 1: A null pointer is ignored.
    if backend.is_null() {
        return;
    }
    // Step 2: Rebuild the box here and drop it, so the renderer's own drop runs
    // inside the image that allocated it.
    // SAFETY: the contract above says the pointer came from `Box::into_raw` in
    // `attach_to_window` and that the caller does not use it again, so it is
    // released here exactly once.
    let renderer_backend = unsafe { Box::from_raw(backend.cast::<Box<dyn PillRenderer>>()) };
    if catch_unwind(AssertUnwindSafe(|| drop(renderer_backend))).is_err() {
        pill_core::error!(
            target: telemetry_target::ENGINE,
            "the embedded renderer panicked while detaching"
        );
    }
}

/// Expands to this crate's renderer ABI exports, `pill_renderer_attach` and
/// `pill_renderer_detach`.
///
/// The symbols must be defined in the wrapper crate, because that crate is the
/// artifact the host loads.
#[macro_export]
macro_rules! __pill_renderer_entry_points {
    () => {
        /// Builds a renderer on the window the host describes.
        ///
        /// # Safety
        ///
        /// `window`, when non-null, points to a valid `RawWindowData` that
        /// names a live window. The caller keeps that window alive until it
        /// has passed the returned pointer to `pill_renderer_detach`.
        #[no_mangle]
        pub unsafe extern "C" fn pill_renderer_attach(
            window: *const $crate::RawWindowData,
            width: u32,
            height: u32,
        ) -> *mut ::core::ffi::c_void {
            // SAFETY: the caller promises exactly what `attach_to_window`
            // requires, and the call passes the window pointer and the size on
            // unchanged.
            unsafe { $crate::module_entry::attach_to_window(window, width, height) }
        }

        /// Drops a renderer built by `pill_renderer_attach`.
        ///
        /// # Safety
        ///
        /// `backend` is null, or a pointer that `pill_renderer_attach` of this
        /// same image returned. It must not have been detached already.
        #[no_mangle]
        pub unsafe extern "C" fn pill_renderer_detach(backend: *mut ::core::ffi::c_void) {
            // SAFETY: the caller promises exactly what `detach_backend`
            // requires, and the call passes the pointer on unchanged.
            unsafe { $crate::module_entry::detach_backend(backend) }
        }
    };
}
