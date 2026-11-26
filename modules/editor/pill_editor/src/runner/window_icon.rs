//! The editor window's icon: the Pill mark, decoded from the PNG it embeds.
//!
//! # Responsibilities
//!
//! - [`window_icon`]: the icon the main window is created with.
//! - [`decode`]: one PNG in the shape winit takes - square, 8-bit RGBA.
//!
//! # Design
//!
//! The PNG beside this crate is embedded in the binary, so the editor has its
//! icon wherever it runs: no asset store, no project and no file beside the
//! executable. It is `res/pill_editor_window_icon_black.png` (the white mark
//! on the black disc), rendered from the same mark the menu bar draws by
//! `devops/tools/generate_app_icons.py`, so the window's icon and the menu
//! bar's cannot disagree.
//!
//! An icon that will not decode is a broken build asset, not a reason to
//! refuse to open: the failure is logged and the window opens without one.
//! On Windows the window's icon is the small one winit sets; the taskbar's
//! own icon comes from the resource the build script links into the
//! executable.

// Standard library
use std::io::Cursor;

// External crates
use winit::window::Icon;

// Current crate
use pill_core::telemetry::telemetry_target;
use pill_core::warn;

/// The window icon every editor build renders: 64 pixels square.
const WINDOW_ICON_PNG: &[u8] = include_bytes!("../../res/pill_editor_window_icon_black.png");

// =============================================================================
// Free Functions
// =============================================================================

/// The icon the editor's main window is created with.
///
/// Returns `None` when the embedded PNG does not decode, having logged why: a
/// missing icon costs the window its picture, not the editor its window.
#[must_use]
pub(super) fn window_icon() -> Option<Icon> {
    match decode(WINDOW_ICON_PNG) {
        Ok(icon) => Some(icon),
        Err(reason) => {
            warn!(
                target: telemetry_target::EDITOR,
                reason = %reason,
                "[editor] the window icon did not load"
            );
            None
        }
    }
}

/// Decodes one PNG into a window icon.
///
/// # Errors
///
/// Returns a message naming what the file was when it is not a square,
/// 8-bit-per-channel RGBA PNG, or when winit refuses the pixel data.
fn decode(bytes: &[u8]) -> Result<Icon, String> {
    let mut reader = png::Decoder::new(Cursor::new(bytes))
        .read_info()
        .map_err(|error| format!("the PNG header did not read: {error}"))?;
    let capacity = reader
        .output_buffer_size()
        .ok_or_else(|| "the PNG reports no decoded size".to_owned())?;
    let mut pixels = vec![0_u8; capacity];
    let frame = reader
        .next_frame(&mut pixels)
        .map_err(|error| format!("the PNG data did not decode: {error}"))?;
    pixels.truncate(frame.buffer_size());
    if frame.color_type != png::ColorType::Rgba || frame.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "the PNG is {:?} at {:?}, and a window icon is 8-bit RGBA",
            frame.color_type, frame.bit_depth
        ));
    }
    if frame.width != frame.height {
        return Err(format!(
            "the PNG is {}x{}, and a window icon is square",
            frame.width, frame.height
        ));
    }
    Icon::from_rgba(pixels, frame.width, frame.height)
        .map_err(|error| format!("winit refused the pixels: {error}"))
}
