//! Showing a processor renderer's frame in the frontend's window.
//!
//! # Responsibilities
//!
//! - Widen one RGB565 panel frame into 32-bit colour pixels and stretch them
//!   over the window ([`PanelBlit::present`]).
//! - Keep the widened pixels between frames, so showing one allocates nothing.
//!
//! # Design
//!
//! A renderer with no graphics device draws into its own pixel buffer and hands
//! it over through [`PillRenderer::panel_frame`]. Putting those pixels on screen
//! belongs to the frontend: it owns the window, and a windowing or graphics
//! *crate* inside a renderer module - or anywhere the host's graph gains it -
//! changes the dependency graph that module builds resolve against. The engine's
//! shared dylibs then differ between the host and the modules, and a module
//! cannot load.
//!
//! The blit therefore calls the platform directly, through declarations written
//! here: GDI's `StretchDIBits` on Windows. That adds no crate to any graph, only
//! a link to system libraries every Windows build already has. The panel keeps
//! its resolution; the stretch fills the window, so a 240x240 frame appears as a
//! crisp enlargement of what a device panel would show.
//!
//! On a platform this module has no blit for, `new` answers `None`: the frames
//! are still drawn and reported as presented, and the window stays blank.

// External crates
use pill_renderer_api::PanelFrame;

// =============================================================================
// Platform declarations
// =============================================================================

/// The Windows calls the blit makes, declared here rather than taken from a
/// crate: a crate would join the host's dependency graph, which every module
/// build has to match.
#[cfg(windows)]
mod platform {
    use std::ffi::c_void;

    /// A window handle (`HWND`).
    pub type WindowHandle = *mut c_void;

    /// A device context (`HDC`).
    pub type DeviceContext = *mut c_void;

    /// The header of a device-independent bitmap, laid out as `BITMAPINFOHEADER`
    /// is.
    #[repr(C)]
    pub struct BitmapInfoHeader {
        pub size: u32,
        pub width: i32,
        pub height: i32,
        pub planes: u16,
        pub bit_count: u16,
        pub compression: u32,
        pub image_size: u32,
        pub x_pixels_per_meter: i32,
        pub y_pixels_per_meter: i32,
        pub colours_used: u32,
        pub colours_important: u32,
    }

    /// A device-independent bitmap's description (`BITMAPINFO`): the header and
    /// room for the colour table a 32-bit image does not use.
    #[repr(C)]
    pub struct BitmapInfo {
        pub header: BitmapInfoHeader,
        pub colours: [u32; 3],
    }

    /// Uncompressed colour.
    pub const BI_RGB: u32 = 0;

    /// A colour table of literal RGB values.
    pub const DIB_RGB_COLORS: u32 = 0;

    /// Copy the source pixels to the destination.
    pub const SRCCOPY: u32 = 0x00CC_0020;

    /// Stretch by dropping or doubling pixels, which keeps a scaled panel crisp.
    pub const COLORONCOLOR: i32 = 3;

    #[link(name = "user32")]
    extern "system" {
        /// Borrows the window's device context.
        pub fn GetDC(window: WindowHandle) -> DeviceContext;
        /// Gives the borrowed device context back.
        pub fn ReleaseDC(window: WindowHandle, device_context: DeviceContext) -> i32;
    }

    #[link(name = "gdi32")]
    extern "system" {
        /// Sets how a stretch resamples.
        pub fn SetStretchBltMode(device_context: DeviceContext, mode: i32) -> i32;
        /// Copies a device-independent bitmap into a device context, scaling it.
        #[allow(clippy::too_many_arguments)]
        pub fn StretchDIBits(
            device_context: DeviceContext,
            destination_x: i32,
            destination_y: i32,
            destination_width: i32,
            destination_height: i32,
            source_x: i32,
            source_y: i32,
            source_width: i32,
            source_height: i32,
            pixels: *const c_void,
            bitmap_info: *const BitmapInfo,
            usage: u32,
            raster_operation: u32,
        ) -> i32;
    }
}

// =============================================================================
// PanelBlit
// =============================================================================

/// The window a processor renderer's frames are shown in.
pub(crate) struct PanelBlit {
    /// The window's platform handle, as the frontend handed it over: an `HWND`
    /// on Windows.
    #[cfg(windows)]
    window: u64,
    /// The panel frame widened to 32-bit colour, kept between frames.
    #[cfg(windows)]
    pixels: Vec<u32>,
    /// The panel's size the widened pixels were built for.
    panel_size: (u32, u32),
}

impl PanelBlit {
    /// Builds a blit for the window whose platform handle is `window`.
    ///
    /// Returns `None` on a platform this module has no blit for, where the
    /// window stays blank.
    pub(crate) fn new(window: u64) -> Option<Self> {
        if window == 0 {
            return None;
        }
        #[cfg(windows)]
        {
            Some(Self {
                window,
                pixels: Vec::new(),
                panel_size: (0, 0),
            })
        }
        #[cfg(not(windows))]
        {
            None
        }
    }

    /// Shows `frame` stretched over the window, sized `size` physical pixels.
    pub(crate) fn present(&mut self, frame: PanelFrame<'_>, size: (u32, u32)) {
        if size.0 == 0 || size.1 == 0 || frame.width == 0 || frame.height == 0 {
            return;
        }
        self.widen(frame);
        #[cfg(windows)]
        // SAFETY: `window` is the live window handle the frontend attached the
        // renderer with and keeps alive while this blit exists; the pixel
        // buffer and the bitmap description outlive the call.
        unsafe {
            self.stretch_to_window(size);
        }
    }

    /// Widens the panel's RGB565 pixels into the 32-bit buffer, reusing it when
    /// the panel's size did not change.
    fn widen(&mut self, frame: PanelFrame<'_>) {
        let panel_size = (frame.width, frame.height);
        #[cfg(windows)]
        {
            let pixel_count = frame.width as usize * frame.height as usize;
            if self.panel_size != panel_size || self.pixels.len() != pixel_count {
                self.pixels.clear();
                self.pixels.resize(pixel_count, 0);
            }
            for (destination, pixel) in self.pixels.iter_mut().zip(frame.pixels.iter().copied()) {
                *destination = widen_pixel(pixel);
            }
        }
        #[cfg(not(windows))]
        let _ = frame;
        self.panel_size = panel_size;
    }

    /// Stretches the widened panel over the window with GDI.
    ///
    /// # Safety
    ///
    /// `self.window` must be a live window handle, which the caller keeps
    /// alive.
    #[cfg(windows)]
    unsafe fn stretch_to_window(&self, size: (u32, u32)) {
        use platform::{
            BitmapInfo, BitmapInfoHeader, BI_RGB, COLORONCOLOR, DIB_RGB_COLORS, SRCCOPY,
        };

        let (width, height) = self.panel_size;
        if self.pixels.len() != width as usize * height as usize {
            return;
        }
        let window = self.window as platform::WindowHandle;
        // SAFETY: the caller's contract. `GetDC` only borrows the window's
        // device context, which is released below.
        let device_context = unsafe { platform::GetDC(window) };
        if device_context.is_null() {
            return;
        }
        // A top-down 32-bit bitmap whose rows are already in 0x00RRGGBB order.
        let bitmap_info = BitmapInfo {
            header: BitmapInfoHeader {
                size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                width: width as i32,
                height: -(height as i32),
                planes: 1,
                bit_count: 32,
                compression: BI_RGB,
                image_size: 0,
                x_pixels_per_meter: 0,
                y_pixels_per_meter: 0,
                colours_used: 0,
                colours_important: 0,
            },
            colours: [0; 3],
        };
        // SAFETY: the buffer holds `width * height` pixels, the description
        // matches that image exactly, and both outlive the call.
        unsafe {
            platform::SetStretchBltMode(device_context, COLORONCOLOR);
            platform::StretchDIBits(
                device_context,
                0,
                0,
                size.0 as i32,
                size.1 as i32,
                0,
                0,
                width as i32,
                height as i32,
                self.pixels.as_ptr().cast(),
                &bitmap_info,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
            platform::ReleaseDC(window, device_context);
        }
    }
}

/// Converts one packed RGB565 pixel into the window's 32-bit pixel.
#[cfg(windows)]
fn widen_pixel(pixel: u16) -> u32 {
    let widen = |value: u32, max: u32| value * 255 / max;
    let red = widen(u32::from((pixel >> 11) & 0x1F), 31);
    let green = widen(u32::from((pixel >> 5) & 0x3F), 63);
    let blue = widen(u32::from(pixel & 0x1F), 31);
    (red << 16) | (green << 8) | blue
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// A packed pixel widens to the window's channel order.
    #[test]
    fn a_packed_pixel_widens_into_the_window_pixel() {
        assert_eq!(widen_pixel(0x0000), 0x00_00_00);
        assert_eq!(widen_pixel(0xF800), 0x00_FF_00_00);
        assert_eq!(widen_pixel(0x07E0), 0x00_00_FF_00);
        assert_eq!(widen_pixel(0x001F), 0x00_00_00_FF);
        assert_eq!(widen_pixel(0xFFFF), 0x00_FF_FF_FF);
    }

    /// Widening keeps the buffer's length and reuses it across frames.
    #[test]
    fn widening_reuses_the_buffer_between_frames() {
        let pixels = [0xF800u16, 0x001F];
        let frame = PanelFrame {
            width: 2,
            height: 1,
            pixels: &pixels,
        };
        let mut blit = PanelBlit {
            window: 0,
            pixels: Vec::new(),
            panel_size: (0, 0),
        };

        blit.widen(frame);

        assert_eq!(blit.pixels, [0x00_FF_00_00, 0x00_00_00_FF]);
        assert_eq!(blit.panel_size, (2, 1));
    }
}
