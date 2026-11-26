//! The editor window's own frame: rounded corners and a one-pixel border.
//!
//! # Responsibilities
//!
//! - [`apply`]: round the window's corners and give it a border, through the
//!   desktop compositor.
//! - [`border_for`]: the border colour, taken from the theme's line colour.
//!
//! # Design
//!
//! The window has no system title bar (`with_decorations(false)`), so nothing
//! draws its frame unless this does. Windows 11's compositor draws both: the
//! corner preference rounds the window - including the composition overlay
//! above it - and the border colour paints the one-pixel line along its edge,
//! antialiased, with the shadow and the square corners a maximized window has.
//!
//! The corner radius is the compositor's, not ours: `DwmSetWindowAttribute`
//! accepts only `DWMWCP_ROUND` (8px at 100% DPI) or `DWMWCP_ROUNDSMALL` (4px),
//! so an exact 6px would need a window region on the outside, which loses the
//! antialiasing, the shadow and this border in one go.
//!
//! The border follows the theme, so [`apply`] runs again whenever the editor's
//! theme resolves to something new.

// Current crate
use pill_editor_core::egui::Color32;
use pill_editor_core::style::Palette;
use winit::window::Window;

// =============================================================================
// Free Functions
// =============================================================================

/// Returns the colour of the window's one-pixel border.
///
/// The theme's line colour is what every separator and panel edge already
/// uses, so the window's own edge reads as one more of them: a step lighter
/// than the window's background without competing with the content.
#[must_use]
pub(super) fn border_for(palette: &Palette) -> Color32 {
    palette.line
}

/// Rounds the window's corners and draws its one-pixel border in `border`.
///
/// Both are the desktop compositor's work: on a Windows 11 desktop the window
/// gains the rounded corners every other window has, and the border is drawn
/// along that rounded edge. A platform or a Windows version without the
/// attributes keeps a square, borderless window - the look is cosmetic and
/// nothing else depends on it - so a refusal is logged at debug level only.
pub(super) fn apply(window: &Window, border: Color32) {
    #[cfg(windows)]
    platform::apply(window, border);
    #[cfg(not(windows))]
    {
        let _ = (window, border);
    }
}

// =============================================================================
// Windows
// =============================================================================

/// The `DwmSetWindowAttribute` calls behind [`apply`].
#[cfg(windows)]
mod platform {
    // Standard library
    use std::mem::size_of;

    // External crates
    use pill_core::{debug, telemetry::telemetry_target};
    use pill_editor_core::egui::Color32;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        DWM_WINDOW_CORNER_PREFERENCE,
    };
    use winit::window::Window;

    /// The corner the compositor rounds the window to.
    ///
    /// `DWMWCP_ROUND` is the 8px radius every Windows 11 window has;
    /// `DWMWCP_ROUNDSMALL` is the 4px one. There is nothing between them and no
    /// attribute takes a radius of the app's own.
    const CORNER: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;

    /// Rounds the window's corners and draws its border in `border`.
    pub(super) fn apply(window: &Window, border: Color32) {
        let Some(hwnd) = super::hwnd(window) else {
            debug!(
                target: telemetry_target::EDITOR,
                "[editor] the window's frame was skipped: it has no HWND"
            );
            return;
        };
        // Step 1: the rounded corners. A maximized window squares them itself,
        // which is what the platform's own windows do.
        let corner = CORNER;
        if let Err(error) = set(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const DWM_WINDOW_CORNER_PREFERENCE,
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>(),
        ) {
            debug!(
                target: telemetry_target::EDITOR,
                error = %error,
                "[editor] the window's corners stay square"
            );
        }
        // Step 2: the one-pixel border, in the theme's own line colour.
        let colour = color_ref(border);
        if let Err(error) = set(hwnd, DWMWA_BORDER_COLOR, &colour, size_of::<u32>()) {
            debug!(
                target: telemetry_target::EDITOR,
                error = %error,
                "[editor] the window keeps no border"
            );
        }
        debug!(
            target: telemetry_target::EDITOR,
            border = %format!("#{:02x}{:02x}{:02x}", border.r(), border.g(), border.b()),
            "[editor] window frame applied"
        );
    }

    /// Sets one DWM attribute from a value of `len` bytes.
    fn set<T>(
        hwnd: HWND,
        attribute: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE,
        value: *const T,
        len: usize,
    ) -> windows::core::Result<()> {
        // SAFETY: `value` points at a live value of `len` bytes that outlives
        // the call, and the attribute expects exactly that type and size.
        unsafe {
            DwmSetWindowAttribute(hwnd, attribute, value.cast(), len as u32)?;
        }
        Ok(())
    }

    /// Returns `colour` as the `COLORREF` a DWM attribute takes: `0x00BBGGRR`.
    fn color_ref(colour: Color32) -> u32 {
        u32::from(colour.b()) << 16 | u32::from(colour.g()) << 8 | u32::from(colour.r())
    }
}

/// Returns the window's `HWND`, when it has one.
#[cfg(windows)]
fn hwnd(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    crate::overlay::hwnd_of(window)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The border is a step lighter than the window's background, which is what
    /// makes it read as an edge rather than a seam.
    #[test]
    fn the_border_is_lighter_than_the_window_background() {
        let palette = pill_editor_core::style::palette();
        let border = border_for(&palette);
        let background = palette.frame;
        let luminance =
            |colour: Color32| u32::from(colour.r()) + u32::from(colour.g()) + u32::from(colour.b());
        assert!(
            luminance(border) > luminance(background),
            "the border {border:?} is not lighter than the frame {background:?}"
        );
    }
}
