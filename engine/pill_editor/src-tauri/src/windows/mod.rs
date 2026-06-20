#![cfg(target_os = "windows")]

//! Windows backend implementation for the platform abstraction.
//!
//! Handles:
//! - Win32 child-window creation for wgpu viewports (via `native_viewport`)
//! - Viewport-offset computation (no CSD shadows on Windows, so offsets are 0)
//! - Delegates wgpu render-thread management to `renderer_windows`
//! - Popup window lifecycle (WebView2 popout windows)

pub mod native_viewport;

use crate::viewport_store::{ViewportEntry, ViewportStore};
use std::sync::Mutex;

/// Windows implementation of [`crate::platform::PlatformBackend`].
///
/// Manages Win32 child windows, wgpu render threads, and popup windows.
pub struct WindowsBackend {
    /// Shared wgpu renderer — one per app, creates threads per viewport.
    renderer: crate::renderer_windows::Renderer,
}

impl WindowsBackend {
    /// Create a new backend.
    pub fn new() -> Self {
        Self {
            renderer: crate::renderer_windows::Renderer::new(),
        }
    }
}

impl crate::platform::PlatformBackend for WindowsBackend {
    /// One-time initialisation: extracts the main window's HWND, computes
    /// coordinate offsets (0 on Windows — no CSD shadows), and publishes
    /// the ViewportStore. Must be called AFTER the main window has been built.
    fn setup(&self, app: &tauri::AppHandle) -> Result<(), String> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use tauri::Manager;

        let window = app
            .get_webview_window("main")
            .expect("main window must exist");

        // ── Extract the Win32 HWND from the Tauri window.
        let main_hwnd = match window.window_handle() {
            Ok(h) => match h.as_raw() {
                RawWindowHandle::Win32(wh) => wh.hwnd.get() as isize,
                _ => {
                    eprintln!("[WindowsBackend] Not a Win32 window — skipping setup");
                    return Ok(());
                }
            },
            Err(e) => {
                eprintln!("[WindowsBackend] window_handle error: {e}");
                return Ok(());
            }
        };

        // On Windows there's no CSD shadow or titlebar offset to worry about
        // because child-window coordinates are already relative to the
        // parent's client area.
        let x_offset: i32 = 0;
        let y_offset: i32 = 0;

        eprintln!(
            "[WindowsBackend] setup complete — main_hwnd={main_hwnd:#x}, offsets=({x_offset},{y_offset})"
        );

        // ── Publish the ViewportStore so Tauri commands can use it.
        let store_state = app.state::<Mutex<Option<ViewportStore>>>();
        *store_state.lock().unwrap() = Some(ViewportStore::new(
            main_hwnd, // parent_surface = main window HWND
            0,         // display = unused on Windows
            x_offset, y_offset,
        ));

        Ok(())
    }

    /// Create a Win32 child window at the given CSS coordinates and spawn a
    /// dedicated render thread. The `id` is the stable React tab ID.
    fn create_viewport(
        &self,
        store: &Mutex<Option<ViewportStore>>,
        id: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        // ── Read parent HWND and offsets from ViewportStore.
        let (parent_hwnd, wx, wy) = {
            let g = store.lock().map_err(|e| e.to_string())?;
            let s = g.as_ref().ok_or("ViewportStore not ready")?;
            (s.parent_surface, s.x_offset + x, s.y_offset + y)
        };

        // ── Create the Win32 child window parented to the main window.
        let child_hwnd = native_viewport::create_child_window(parent_hwnd)?;

        // ── Determine overlay kind from the viewport ID.
        let _overlay_kind = if id.starts_with("game") {
            native_viewport::OverlayKind::Game
        } else {
            native_viewport::OverlayKind::Scene
        };

        // TODO: Overlay windows (WS_EX_LAYERED + LWA_COLORKEY) interfere
        // with viewport rendering when the parent window is also layered.
        // Disabled until a working approach is found (e.g. single top-level
        // owned overlay window per the Layer 3 design in WINDOWS_IMPL.md).
        let overlay_hwnd: isize = 0;
        // let overlay_hwnd = native_viewport::create_overlay(parent_hwnd, overlay_kind)?;

        // ── Position the viewport at the given CSS coordinates.
        native_viewport::set_window_rect(child_hwnd, wx, wy, width as i32, height as i32);

        // ── Position the overlay at the same rectangle (called AFTER
        //     set_window_rect so the overlay lands on top of the viewport).
        if overlay_hwnd != 0 {
            native_viewport::set_overlay_rect(overlay_hwnd, wx, wy, width as i32, height as i32);
        }

        // ── Start the render thread.
        let resize_tx = self.renderer.register_viewport(child_hwnd, width, height);

        // ── Store the entry in ViewportStore keyed by the React tab ID.
        let mut g = store.lock().map_err(|e| e.to_string())?;
        let s = g.as_mut().ok_or("ViewportStore not ready")?;
        let id_dbg = id.clone();
        s.viewports.insert(
            id,
            ViewportEntry {
                surface: child_hwnd,
                subsurface: 0, // not used on Windows
                overlay: overlay_hwnd,
                resize_tx,
            },
        );

        eprintln!(
            "[WindowsBackend] viewport created: id={id_dbg}, hwnd={child_hwnd:#x}, \
             pos=({wx},{wy}), size={width}x{height}"
        );
        Ok(())
    }

    /// Move and resize a child window when its tab is repositioned, and
    /// signal the render thread to reconfigure the swapchain.
    fn viewport_resize(
        &self,
        store: &Mutex<Option<ViewportStore>>,
        id: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        let (child_hwnd, overlay_hwnd, wx, wy, resize_tx) = {
            let g = store.lock().map_err(|e| e.to_string())?;
            let s = g.as_ref().ok_or("ViewportStore not ready")?;
            let e = s
                .viewports
                .get(&id)
                .ok_or_else(|| format!("unknown viewport: {id}"))?;
            (
                e.surface,
                e.overlay,
                s.x_offset + x,
                s.y_offset + y,
                e.resize_tx.clone(),
            )
        };

        // ── Move the child window (SetWindowPos is direct, no thread concerns).
        native_viewport::set_window_rect(child_hwnd, wx, wy, width as i32, height as i32);

        // ── Move the overlay (called AFTER so it stays on top of the viewport).
        if overlay_hwnd != 0 {
            native_viewport::set_overlay_rect(overlay_hwnd, wx, wy, width as i32, height as i32);
        }

        // ── Signal the render thread to reconfigure its swapchain.
        let _ = resize_tx.send(Some((width.max(1), height.max(1))));
        Ok(())
    }

    /// Send a shutdown signal to the render thread, then destroy the Win32
    /// child window.
    fn delete_viewport(
        &self,
        store: &Mutex<Option<ViewportStore>>,
        id: String,
    ) -> Result<(), String> {
        // ── Remove the entry and signal the render thread to stop.
        let (child_hwnd, overlay_hwnd) = {
            let mut g = store.lock().map_err(|e| e.to_string())?;
            let s = g.as_mut().ok_or("ViewportStore not ready")?;
            let e = s
                .viewports
                .remove(&id)
                .ok_or_else(|| format!("unknown viewport: {id}"))?;
            let _ = e.resize_tx.send(None); // None = shutdown
            (e.surface, e.overlay)
        };

        // ── Give the render thread one frame (~16ms) to notice the shutdown
        //     signal before we destroy its HWND.
        std::thread::sleep(std::time::Duration::from_millis(32));

        // ── Destroy the overlay window first (no render-thread dependency).
        if overlay_hwnd != 0 {
            native_viewport::destroy_overlay(overlay_hwnd);
        }

        // ── Destroy the child window.
        native_viewport::destroy_window(child_hwnd);
        eprintln!("[WindowsBackend] viewport destroyed: hwnd={child_hwnd:#x}");

        Ok(())
    }

    /// Called when flexlayout opens a popup window via `window.open`.
    ///
    /// On Windows, WebView2 handles popups natively — we don't need to
    /// manually rename or track them. Stub for API compatibility.
    fn on_popup_created(&self, url: &str) {
        eprintln!("[WindowsBackend] popup created: {url}");
    }

    /// Close a popup window identified by its flexlayout layout ID.
    ///
    /// Windows/WebView2 popups are browser-managed. Stub for API compatibility.
    fn close_popup_window(&self, layout_id: &str) -> Result<(), String> {
        eprintln!("[WindowsBackend] close_popup_window({layout_id}) — stub");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Popup window creation (called from lib.rs via cfg)
// ---------------------------------------------------------------------------

use std::sync::atomic::AtomicU32;

/// Build a proper popup window with title "Pill" and no URL bar.
///
/// Called by the main window's `on_new_window` handler on Windows.
/// Uses `NewWindowResponse::Create` to construct a clean Tauri window
/// instead of the default browser-style popup (which shows the URL).
pub fn handle_popup<R: tauri::Runtime>(
    url: tauri::Url,
    app_handle: &tauri::AppHandle<R>,
) -> tauri::webview::NewWindowResponse<R> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};

    static POPUP_COUNT: AtomicU32 = AtomicU32::new(0);
    let n = POPUP_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let label = format!("popup_{n}");

    match WebviewWindowBuilder::new(app_handle, &label, WebviewUrl::External(url))
        .title("Pill")
        .inner_size(800.0, 600.0)
        .build()
    {
        Ok(window) => tauri::webview::NewWindowResponse::Create { window },
        Err(e) => {
            eprintln!("[WindowsBackend] popup build failed: {e}");
            tauri::webview::NewWindowResponse::Deny
        }
    }
}
