#![cfg(target_os = "linux")]

//! Window-level initialisation for the Linux backend.
//!
//! Called once during app startup. Sets up:
//! 1. A solid-colour SHM subsurface (the editor background)
//! 2. CSS→Wayland coordinate offset computation (accounts for CSD shadow + titlebar)
//! 3. Publication of the offset values into the shared `ViewportStore`

use super::linux_surface_utilities;
use raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use tauri::Manager;

/// Entry point — called from [`LinuxBackend::setup`].
///
/// Extracts the GTK window's `wl_surface*` and `wl_display*`, creates the
/// background SHM layer, computes coordinate offsets, and publishes the
/// `ViewportStore` for the viewport commands.
pub fn draw(app: tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let window = app
        .get_webview_window("main")
        .expect("main window must exist");

    // -- Step 1: extract the Wayland wl_surface* and wl_display* from GTK.
    let (parent_surface, display_ptr) = match get_surface_and_display(&window) {
        Some(pair) => pair,
        None => return Ok(()), // already logged inside
    };

    // -- Step 2: create the editor background SHM subsurface.
    let (shadow_x, shadow_y, bg_h) =
        setup_background(&window, parent_surface, display_ptr);

    // -- Step 3: compute CSS->Wayland offsets and publish to ViewportStore.
    calculate_viewport_offset(&app, &window, parent_surface, display_ptr, shadow_x, shadow_y, bg_h)?;

    Ok(())
}

/// Extract the raw `wl_surface*` and `wl_display*` from a Tauri window.
///
/// Uses `raw-window-handle` 0.6 (the version Tauri 2 uses) to access the
/// Wayland backend handles.  Returns `None` (after logging) on non-Wayland
/// platforms or if the handle extraction fails.
fn get_surface_and_display(
    window: &tauri::WebviewWindow,
) -> Option<(*mut std::ffi::c_void, *mut std::ffi::c_void)> {
    let parent_surface: *mut std::ffi::c_void = match window.window_handle() {
        Ok(h) => match h.as_raw() {
            RawWindowHandle::Wayland(wh) => wh.surface.as_ptr() as *mut _,
            _ => {
                eprintln!("[draw] Not a Wayland window - skipping");
                return None;
            }
        },
        Err(e) => {
            eprintln!("[draw] window_handle error: {e}");
            return None;
        }
    };
    let display_ptr: *mut std::ffi::c_void = match window.display_handle() {
        Ok(h) => match h.as_raw() {
            RawDisplayHandle::Wayland(dh) => dh.display.as_ptr() as *mut _,
            _ => {
                eprintln!("[draw] Not a Wayland display - skipping");
                return None;
            }
        },
        Err(e) => {
            eprintln!("[draw] display_handle error: {e}");
            return None;
        }
    };
    Some((parent_surface, display_ptr))
}

/// Create the editor background SHM subsurface and attach a resize handler.
///
/// The background is a solid `#1E1E1E` layer placed below all other
/// subsurfaces.  It prevents transparent gaps from flashing during WebKit
/// repaint lag on resize.
///
/// Returns `(shadow_x, shadow_y, bg_h)` — the CSD shadow margins and the
/// GTK window height — so the caller can derive CSS→Wayland offsets.
fn setup_background(
    window: &tauri::WebviewWindow,
    parent_surface: *mut std::ffi::c_void,
    display_ptr: *mut std::ffi::c_void,
) -> (i32, i32, i32) {
    use gtk::prelude::{GtkWindowExt, WidgetExt};
    let gtk_win = window.gtk_window().expect("GTK window");
    let (bg_w, bg_h) = gtk_win.size();
    // GDK size includes CSD shadow; subtract GTK size to get shadow margins.
    let (shadow_x, shadow_y) = gtk_win
        .window()
        .map(|gdk| {
            let sx = ((gdk.width() - bg_w) / 2).max(0);
            let sy = ((gdk.height() - bg_h) / 2).max(0);
            (sx, sy)
        })
        .unwrap_or((0, 0));

    // #1E1E1E: any WebKit repaint lag during resize shows as dark, not a flash.
    // Dark background: WebKit repaint lag during resize shows dark, not a flash.
    const BG_ARGB: u32 = 0xFF1E_1E1E;

    match linux_surface_utilities::create_shm_subsurface(
        parent_surface,
        display_ptr,
        BG_ARGB,
        bg_w, bg_h,
        shadow_x, shadow_y,
        parent_surface,
    ) {
        Err(e) => eprintln!("[bg] Failed: {e}"),
        Ok(shm_surf) => {
            let bg_isize   = shm_surf.surface as isize;
            let shm_isize  = shm_surf.shm     as isize;
            let disp_isize = display_ptr       as isize;

            // connect_size_allocate fires BEFORE GTK commits to Wayland.
            // This avoids the one-frame gap that on_window_event(Resized)
            // causes -- no flash of the old buffer during resize.
            gtk_win.connect_size_allocate(move |gtk_win, _alloc| {
                let (w, h) = gtk_win.size(); // gtk_window_get_size() -- no shadow
                if let Err(e) = linux_surface_utilities::resize_shm_surface(
                    bg_isize   as *mut std::ffi::c_void,
                    disp_isize as *mut std::ffi::c_void,
                    shm_isize  as *mut std::ffi::c_void,
                    BG_ARGB,
                    w, h,
                ) {
                    eprintln!("[bg] Resize failed: {e}");
                }
            });
        }
    }

    (shadow_x, shadow_y, bg_h)
}

/// Compute CSS→Wayland coordinate offsets and publish them to `ViewportStore`.
///
/// **Coordinate systems:**
/// - CSS `(0,0)` = webview content top-left
/// - Wayland `(0,0)` = GDK `wl_surface` origin (outside the CSD shadow)
///
/// **Published offsets:**
/// - `x_offset = shadow_x` (CSD shadow left margin)
/// - `y_offset = shadow_y + titlebar_height` (shadow top + GTK titlebar)
///
/// Adding these to a CSS coordinate gives the Wayland parent-surface
/// coordinate, aligning viewport subsurfaces with their React tab containers.
fn calculate_viewport_offset(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    parent_surface: *mut std::ffi::c_void,
    display_ptr: *mut std::ffi::c_void,
    shadow_x: i32,
    shadow_y: i32,
    bg_h: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    // Physical pixels -> logical CSS pixels.
    let scale = window.scale_factor().unwrap_or(1.0);
    let inner = window
        .inner_size()
        .unwrap_or_else(|_| tauri::PhysicalSize { width: 800, height: 600 });
    let inner_h_logical = (inner.height as f64 / scale).round() as i32;
    // GTK window height minus webview content height = titlebar height.
    let titlebar_h = (bg_h - inner_h_logical).max(0);
    // Wayland pos = CSS pos + shadow offset + titlebar.
    let x_offset   = shadow_x;
    let y_offset   = shadow_y + titlebar_h;
    eprintln!(
        "[draw] offsets: shadow=({shadow_x},{shadow_y}) titlebar={titlebar_h} => ({x_offset},{y_offset})"
    );

    let store_state = app
        .state::<std::sync::Mutex<Option<crate::viewport_store::ViewportStore>>>();
    *store_state.lock().unwrap() = Some(crate::viewport_store::ViewportStore::new(
        parent_surface as isize,
        display_ptr    as isize,
        x_offset,
        y_offset,
    ));

    Ok(())
}

