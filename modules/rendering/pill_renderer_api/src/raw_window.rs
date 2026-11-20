//! A window's platform identity as plain data, so a renderer built separately
//! from the frontend that owns the window can create surfaces on it.
//!
//! # Responsibilities
//!
//! - Define [`RawWindowData`], a `#[repr(C)]` value holding the handles a GPU
//!   surface is created from.
//! - Build it from any window that implements `raw-window-handle` 0.6 (winit,
//!   tao), and convert it back to the `raw-window-handle` values wgpu takes.
//!
//! # Design
//!
//! The frontend owns the window and its event loop; the renderer only needs to
//! create a surface on it. Passing a window *type* across that boundary would
//! tie the renderer to the frontend's windowing crate. Each DLL would then
//! carry its own copy of that crate, and a renderer could not accept a window
//! from a frontend built on a different one. The handles themselves are
//! integers and pointers, so they cross as data. Every field is a fixed-width
//! integer, which keeps the layout stable between the artifacts that exchange
//! it.
//!
//! The data does not keep the window alive. Whoever hands it out guarantees
//! the window outlives every surface created from it.

// External crates
pub use raw_window_handle;
use raw_window_handle::{
    AppKitDisplayHandle, AppKitWindowHandle, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WebCanvasWindowHandle,
    WebDisplayHandle, Win32WindowHandle, WindowsDisplayHandle, XcbDisplayHandle, XcbWindowHandle,
    XlibDisplayHandle, XlibWindowHandle,
};

// Current crate
use crate::error::RendererError;

// =============================================================================
// RawWindowKind
// =============================================================================

/// The platform whose handles a [`RawWindowData`] carries.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawWindowKind {
    /// Windows: `window` holds the `HWND`, and `window_aux` the `HINSTANCE`,
    /// or 0 when there is none.
    Win32 = 1,
    /// X11 through Xlib: `window` holds the window id and `window_aux` the
    /// visual id, or 0 when there is none. `display` holds the `Display*`, or
    /// 0, and `screen` the screen number.
    Xlib = 2,
    /// X11 through XCB: `window` holds the window id and `window_aux` the
    /// visual id, or 0 when there is none. `display` holds the connection, or
    /// 0, and `screen` the screen number.
    Xcb = 3,
    /// Wayland: `window` holds the `wl_surface*` and `display` the
    /// `wl_display*`.
    Wayland = 4,
    /// macOS: `window` is the `NSView*`.
    AppKit = 5,
    /// The web: `window` points at the `JsValue` of the page's
    /// `HtmlCanvasElement`, as winit hands it out. It is only meaningful
    /// inside the wasm module that owns that value.
    WebCanvas = 6,
}

// =============================================================================
// RawWindowData
// =============================================================================

/// A window's platform handles, as plain data.
///
/// Only the fields [`RawWindowKind`] names for `kind` are meaningful; every
/// other field is zero.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawWindowData {
    /// Which platform the handles belong to.
    pub kind: RawWindowKind,
    /// The window, as a handle, an id or a pointer, depending on `kind`.
    pub window: u64,
    /// A second window-side value, which only some platforms use.
    pub window_aux: u64,
    /// The display connection, on the platforms that have one.
    pub display: u64,
    /// The X11 screen number. Zero on every other platform.
    pub screen: i32,
}

impl RawWindowData {
    /// Reads the platform handles out of `window`.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::SurfaceCreation`] when the window cannot give
    /// its handles right now, when its platform is not one this type carries,
    /// or when its window and display handles come from different platforms.
    pub fn from_window(
        window: &(impl HasWindowHandle + HasDisplayHandle + ?Sized),
    ) -> Result<Self, RendererError> {
        let window_handle = window
            .window_handle()
            .map_err(surface_failure_from_handle_error)?
            .as_raw();
        let display_handle = window
            .display_handle()
            .map_err(surface_failure_from_handle_error)?
            .as_raw();
        Self::from_raw(window_handle, display_handle)
    }

    /// Builds the plain data from handles that were already taken out of a
    /// window.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::SurfaceCreation`] when the platform is not one
    /// this type carries, or when the window and display handles come from
    /// different platforms.
    pub fn from_raw(
        window: RawWindowHandle,
        display: RawDisplayHandle,
    ) -> Result<Self, RendererError> {
        let zeroed_handles = |kind| Self {
            kind,
            window: 0,
            window_aux: 0,
            display: 0,
            screen: 0,
        };
        let pointer_as_integer = |pointer: Option<std::ptr::NonNull<core::ffi::c_void>>| {
            pointer.map_or(0, |pointer| pointer.as_ptr() as usize as u64)
        };
        match (window, display) {
            (RawWindowHandle::Win32(handle), RawDisplayHandle::Windows(_)) => Ok(Self {
                window: handle.hwnd.get() as u64,
                window_aux: handle.hinstance.map_or(0, |instance| instance.get() as u64),
                ..zeroed_handles(RawWindowKind::Win32)
            }),
            (RawWindowHandle::Xlib(handle), RawDisplayHandle::Xlib(display)) => Ok(Self {
                window: u64::from(handle.window),
                window_aux: u64::from(handle.visual_id),
                display: pointer_as_integer(display.display),
                screen: display.screen,
                ..zeroed_handles(RawWindowKind::Xlib)
            }),
            (RawWindowHandle::Xcb(handle), RawDisplayHandle::Xcb(display)) => Ok(Self {
                window: u64::from(handle.window.get()),
                window_aux: handle.visual_id.map_or(0, |visual| u64::from(visual.get())),
                display: pointer_as_integer(display.connection),
                screen: display.screen,
                ..zeroed_handles(RawWindowKind::Xcb)
            }),
            (RawWindowHandle::Wayland(handle), RawDisplayHandle::Wayland(display)) => Ok(Self {
                window: pointer_as_integer(Some(handle.surface)),
                display: pointer_as_integer(Some(display.display)),
                ..zeroed_handles(RawWindowKind::Wayland)
            }),
            (RawWindowHandle::AppKit(handle), RawDisplayHandle::AppKit(_)) => Ok(Self {
                window: pointer_as_integer(Some(handle.ns_view)),
                ..zeroed_handles(RawWindowKind::AppKit)
            }),
            (RawWindowHandle::WebCanvas(handle), RawDisplayHandle::Web(_)) => Ok(Self {
                window: pointer_as_integer(Some(handle.obj)),
                ..zeroed_handles(RawWindowKind::WebCanvas)
            }),
            (window_handle, display_handle) => Err(RendererError::SurfaceCreation {
                detail: format!(
                    "window handles of this platform are not supported: {window_handle:?} on {display_handle:?}"
                ),
            }),
        }
    }

    /// Returns the `raw-window-handle` values a GPU surface is created from.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError::SurfaceCreation`] when a handle the platform
    /// needs is missing: the Win32 window handle, the XCB window id, the
    /// Wayland surface or display, the AppKit view, or the web canvas. An Xlib
    /// window id is not checked.
    pub fn to_raw(&self) -> Result<(RawWindowHandle, RawDisplayHandle), RendererError> {
        let missing_handle = |handle_name: &str| RendererError::SurfaceCreation {
            detail: format!("the window data carries no {handle_name}"),
        };
        let integer_as_pointer =
            |address: u64| std::ptr::NonNull::new(address as usize as *mut core::ffi::c_void);
        match self.kind {
            RawWindowKind::Win32 => {
                let hwnd = std::num::NonZeroIsize::new(self.window as isize)
                    .ok_or_else(|| missing_handle("window handle"))?;
                let mut window = Win32WindowHandle::new(hwnd);
                window.hinstance = std::num::NonZeroIsize::new(self.window_aux as isize);
                Ok((window.into(), WindowsDisplayHandle::new().into()))
            }
            RawWindowKind::Xlib => {
                let mut window = XlibWindowHandle::new(self.window as _);
                window.visual_id = self.window_aux as _;
                Ok((
                    window.into(),
                    XlibDisplayHandle::new(integer_as_pointer(self.display), self.screen).into(),
                ))
            }
            RawWindowKind::Xcb => {
                let id = std::num::NonZeroU32::new(self.window as u32)
                    .ok_or_else(|| missing_handle("window id"))?;
                let mut window = XcbWindowHandle::new(id);
                window.visual_id = std::num::NonZeroU32::new(self.window_aux as u32);
                Ok((
                    window.into(),
                    XcbDisplayHandle::new(integer_as_pointer(self.display), self.screen).into(),
                ))
            }
            RawWindowKind::Wayland => {
                let surface =
                    integer_as_pointer(self.window).ok_or_else(|| missing_handle("surface"))?;
                let display =
                    integer_as_pointer(self.display).ok_or_else(|| missing_handle("display"))?;
                Ok((
                    WaylandWindowHandle::new(surface).into(),
                    WaylandDisplayHandle::new(display).into(),
                ))
            }
            RawWindowKind::AppKit => {
                let view = integer_as_pointer(self.window).ok_or_else(|| missing_handle("view"))?;
                Ok((
                    AppKitWindowHandle::new(view).into(),
                    AppKitDisplayHandle::new().into(),
                ))
            }
            RawWindowKind::WebCanvas => {
                let canvas =
                    integer_as_pointer(self.window).ok_or_else(|| missing_handle("canvas"))?;
                Ok((
                    WebCanvasWindowHandle::new(canvas).into(),
                    WebDisplayHandle::new().into(),
                ))
            }
        }
    }
}

// =============================================================================
// Free Functions
// =============================================================================

/// Turns a failure to read a window's handles into a surface creation error.
fn surface_failure_from_handle_error(error: raw_window_handle::HandleError) -> RendererError {
    RendererError::SurfaceCreation {
        detail: format!("the window's handles are unavailable: {error}"),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Win32 handles come back unchanged after a round trip through the plain
    /// data.
    #[test]
    fn win32_handles_round_trip() {
        let mut window = Win32WindowHandle::new(std::num::NonZeroIsize::new(0x1234).unwrap());
        window.hinstance = std::num::NonZeroIsize::new(0x5678);
        let window_data =
            RawWindowData::from_raw(window.into(), WindowsDisplayHandle::new().into()).unwrap();

        assert_eq!(window_data.kind, RawWindowKind::Win32);
        let (window_back, display_back) = window_data.to_raw().unwrap();
        assert_eq!(window_back, RawWindowHandle::Win32(window));
        assert_eq!(
            display_back,
            RawDisplayHandle::Windows(WindowsDisplayHandle::new())
        );
    }

    /// A web canvas handle comes back unchanged after a round trip.
    #[test]
    fn web_canvas_handles_round_trip() {
        let canvas = std::ptr::NonNull::<core::ffi::c_void>::dangling();
        let window = WebCanvasWindowHandle::new(canvas);
        let window_data =
            RawWindowData::from_raw(window.into(), WebDisplayHandle::new().into()).unwrap();

        assert_eq!(window_data.kind, RawWindowKind::WebCanvas);
        let (window_back, display_back) = window_data.to_raw().unwrap();
        assert_eq!(window_back, RawWindowHandle::WebCanvas(window));
        assert_eq!(display_back, RawDisplayHandle::Web(WebDisplayHandle::new()));
    }

    /// Xcb handles come back unchanged after a round trip, the screen
    /// included.
    #[test]
    fn xcb_handles_round_trip() {
        let mut window = XcbWindowHandle::new(std::num::NonZeroU32::new(42).unwrap());
        window.visual_id = std::num::NonZeroU32::new(7);
        let display = XcbDisplayHandle::new(None, 2);
        let window_data = RawWindowData::from_raw(window.into(), display.into()).unwrap();

        let (window_back, display_back) = window_data.to_raw().unwrap();
        assert_eq!(window_back, RawWindowHandle::Xcb(window));
        assert_eq!(display_back, RawDisplayHandle::Xcb(display));
    }

    /// A zero window handle is refused rather than turned into a surface.
    #[test]
    fn a_zero_window_handle_is_refused() {
        let window_data = RawWindowData {
            kind: RawWindowKind::Win32,
            window: 0,
            window_aux: 0,
            display: 0,
            screen: 0,
        };

        assert!(window_data.to_raw().is_err());
    }

    /// Handles of different platforms are refused rather than mixed.
    #[test]
    fn mismatched_platforms_are_refused() {
        let window = Win32WindowHandle::new(std::num::NonZeroIsize::new(1).unwrap());

        assert!(RawWindowData::from_raw(window.into(), AppKitDisplayHandle::new().into()).is_err());
    }
}
